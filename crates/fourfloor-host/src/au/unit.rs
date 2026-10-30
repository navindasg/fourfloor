//! Finding Audio Units, and the main-thread half of a hosted one.
//!
//! Everything on [`AuMain`] runs on the plugin host thread, inside a
//! [`MainBound`](crate::thread::MainBound). It may allocate and block.

use std::ffi::c_void;
use std::mem::size_of;
use std::ptr;
use std::sync::Arc;

use fourfloor_core::{DeviceCategory, ParamId, ParamInfo, ParamUnit};

use super::ffi::{self, AudioComponentDescription, AudioUnit, OSStatus};
use super::render::{self, InputContext, TransportShared};
use crate::error::HostError;
use crate::info::{PluginFormat, PluginInfo};
use crate::thread::MainIdle;

/// An installed Audio Unit, before it is instantiated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuComponent {
    /// `type.subtype.manufacturer`, e.g. `aufx.dely.appl`.
    pub id: String,
    /// Display name without the manufacturer prefix.
    pub name: String,
    /// Manufacturer name; empty when the component does not give one.
    pub vendor: String,
    /// Version as `major.minor.bugfix`.
    pub version: String,
}

/// Writes a four-character code as text: its four characters when they are
/// printable ASCII, otherwise `#` and eight hex digits.
fn code_to_text(code: u32) -> String {
    let bytes = code.to_be_bytes();
    let plain = bytes
        .iter()
        .all(|&b| (b == b' ' || b.is_ascii_graphic()) && b != b'.' && b != b'#');
    if plain {
        bytes.iter().map(|&b| char::from(b)).collect()
    } else {
        format!("#{code:08X}")
    }
}

fn text_to_code(text: &str) -> Option<u32> {
    if let Some(hex) = text.strip_prefix('#') {
        return (hex.len() == 8)
            .then(|| u32::from_str_radix(hex, 16).ok())
            .flatten();
    }
    let bytes: [u8; 4] = text.as_bytes().try_into().ok()?;
    Some(u32::from_be_bytes(bytes))
}

/// The id fourfloor stores for a component: `type.subtype.manufacturer`.
pub fn component_id(component_type: u32, sub_type: u32, manufacturer: u32) -> String {
    format!(
        "{}.{}.{}",
        code_to_text(component_type),
        code_to_text(sub_type),
        code_to_text(manufacturer)
    )
}

/// Parses an id written by [`component_id`] into the three codes.
pub fn parse_component_id(id: &str) -> Option<(u32, u32, u32)> {
    let mut parts = id.split('.');
    let codes = (
        text_to_code(parts.next()?)?,
        text_to_code(parts.next()?)?,
        text_to_code(parts.next()?)?,
    );
    parts.next().is_none().then_some(codes)
}

fn is_hosted_type(component_type: u32) -> bool {
    matches!(
        component_type,
        ffi::TYPE_EFFECT | ffi::TYPE_MUSIC_EFFECT | ffi::TYPE_MUSIC_DEVICE
    )
}

/// Lists the installed Audio Unit v2 effects (`aufx`), music effects
/// (`aumf`) and instruments (`aumu`), sorted by id.
///
/// No unit is instantiated. Left out: units that need asynchronous
/// instantiation (Audio Unit v3 app extensions that cannot be bridged to
/// the synchronous v2 call), and the other component types (generators, format converters, mixers, panners, MIDI processors,
/// output units), which do not fit an insert slot.
pub fn list_components() -> Vec<AuComponent> {
    let wildcard = AudioComponentDescription::default();
    let mut found = Vec::new();
    let mut component = ptr::null_mut();
    loop {
        // SAFETY: a null or previously returned component and a valid
        // description are what the call expects.
        component = unsafe { ffi::AudioComponentFindNext(component, &wildcard) };
        if component.is_null() {
            break;
        }
        let mut description = AudioComponentDescription::default();
        // SAFETY: `component` was just returned by the system.
        if unsafe { ffi::AudioComponentGetDescription(component, &mut description) } != 0 {
            continue;
        }
        if !is_hosted_type(description.componentType)
            || description.componentFlags & ffi::FLAG_REQUIRES_ASYNC != 0
        {
            continue;
        }
        let mut name_ref = ptr::null();
        // SAFETY: valid component; on success we own `name_ref`.
        let full_name = unsafe {
            if ffi::AudioComponentCopyName(component, &mut name_ref) == 0 && !name_ref.is_null() {
                let text = ffi::cf_string(name_ref);
                ffi::CFRelease(name_ref);
                text
            } else {
                String::new()
            }
        };
        let (vendor, name) = match full_name.split_once(": ") {
            Some((vendor, name)) => (vendor.to_owned(), name.to_owned()),
            None => (String::new(), full_name),
        };
        let mut version = 0u32;
        // SAFETY: valid component and out pointer.
        let _ = unsafe { ffi::AudioComponentGetVersion(component, &mut version) };
        let id = component_id(
            description.componentType,
            description.componentSubType,
            description.componentManufacturer,
        );
        found.push(AuComponent {
            name: if name.is_empty() { id.clone() } else { name },
            id,
            vendor,
            version: format!(
                "{}.{}.{}",
                version >> 16,
                (version >> 8) & 0xFF,
                version & 0xFF
            ),
        });
    }
    found.sort_by(|a, b| a.id.cmp(&b.id));
    found.dedup_by(|a, b| a.id == b.id);
    found
}

/// What the device asks for when it activates the unit.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Activation {
    pub sample_rate: f64,
    pub max_block: u32,
    /// Channels of the graph bus the device sits on.
    pub bus_channels: u32,
}

/// What the unit agreed to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Activated {
    /// Channels of the main input; 0 for an instrument.
    pub inputs: u32,
    pub outputs: u32,
    /// Channels of the sidechain input; 0 when there is none.
    pub sidechain: u32,
    pub latency: u32,
}

/// The unit's pointers, handed to the device for use on the audio thread.
#[derive(Clone, Copy)]
pub(crate) struct AudioHandles {
    pub unit: AudioUnit,
    pub input: *mut InputContext,
}

// SAFETY: the pointers are only carried between threads here. The device
// that receives them is the one audio-thread user, and it keeps the
// `AuMain` that owns both alive.
unsafe impl Send for AudioHandles {}

/// One Audio Unit instance and everything whose address it has been given.
pub(crate) struct AuMain {
    unit: AudioUnit,
    component_type: u32,
    initialized: bool,
    /// Refcon of the input render callbacks. Owned here so that it lives
    /// exactly as long as the unit; filled by the device on the audio
    /// thread.
    input: *mut InputContext,
    /// User data of the host transport callbacks.
    transport: Arc<TransportShared>,
}

impl MainIdle for AuMain {}

fn status_text(what: &str, status: OSStatus) -> String {
    format!("{what} failed with OSStatus {status}")
}

impl AuMain {
    /// Instantiates the component `id`. Host thread only.
    pub fn create(id: &str, transport: Arc<TransportShared>) -> Result<Self, HostError> {
        let (component_type, sub_type, manufacturer) =
            parse_component_id(id).ok_or_else(|| HostError::InvalidId(id.to_owned()))?;
        if !is_hosted_type(component_type) {
            return Err(HostError::Unsupported(format!(
                "Audio Unit type of \"{id}\" is not an effect or an instrument"
            )));
        }
        let description = AudioComponentDescription {
            componentType: component_type,
            componentSubType: sub_type,
            componentManufacturer: manufacturer,
            componentFlags: 0,
            componentFlagsMask: 0,
        };
        // SAFETY: null starts the search; the description is valid.
        let component = unsafe { ffi::AudioComponentFindNext(ptr::null_mut(), &description) };
        if component.is_null() {
            return Err(HostError::NotFound { id: id.to_owned() });
        }
        let mut unit: AudioUnit = ptr::null_mut();
        // SAFETY: valid component and out pointer.
        let status = unsafe { ffi::AudioComponentInstanceNew(component, &mut unit) };
        if status != 0 || unit.is_null() {
            return Err(HostError::Instantiate(status_text(
                "AudioComponentInstanceNew",
                status,
            )));
        }
        Ok(Self {
            unit,
            component_type,
            initialized: false,
            input: Box::into_raw(Box::new(InputContext::default())),
            transport,
        })
    }

    pub fn handles(&self) -> AudioHandles {
        AudioHandles {
            unit: self.unit,
            input: self.input,
        }
    }

    pub fn is_instrument(&self) -> bool {
        self.component_type == ffi::TYPE_MUSIC_DEVICE
    }

    /// True when the unit takes MIDI through `MusicDeviceMIDIEvent`.
    pub fn takes_midi(&self) -> bool {
        self.component_type != ffi::TYPE_EFFECT
    }

    fn get<T: Copy>(&self, property: u32, scope: u32, element: u32, value: &mut T) -> OSStatus {
        let mut size = size_of::<T>() as u32;
        // SAFETY: `value` is a valid, writable `T` and `size` is its size.
        unsafe {
            ffi::AudioUnitGetProperty(
                self.unit,
                property,
                scope,
                element,
                (value as *mut T).cast(),
                &mut size,
            )
        }
    }

    fn set<T>(&self, property: u32, scope: u32, element: u32, value: &T) -> OSStatus {
        // SAFETY: `value` is a valid `T` of the size passed.
        unsafe {
            ffi::AudioUnitSetProperty(
                self.unit,
                property,
                scope,
                element,
                (value as *const T).cast(),
                size_of::<T>() as u32,
            )
        }
    }

    /// How many buses the unit has on `scope`.
    fn element_count(&self, scope: u32) -> u32 {
        let mut count = 0u32;
        if self.get(ffi::PROP_ELEMENT_COUNT, scope, 0, &mut count) == 0 {
            count
        } else {
            0
        }
    }

    fn input_buses(&self) -> u32 {
        if self.is_instrument() {
            0
        } else {
            self.element_count(ffi::SCOPE_INPUT)
        }
    }

    fn channels(&self, scope: u32, element: u32) -> u32 {
        let mut format = ffi::AudioStreamBasicDescription::default();
        if self.get(ffi::PROP_STREAM_FORMAT, scope, element, &mut format) == 0 {
            format.mChannelsPerFrame
        } else {
            0
        }
    }

    /// Reads the unit's writable global parameters.
    pub fn params(&self) -> Vec<ParamInfo> {
        let mut size = 0u32;
        // SAFETY: valid unit and out pointer; a null `outWritable` is allowed.
        let status = unsafe {
            ffi::AudioUnitGetPropertyInfo(
                self.unit,
                ffi::PROP_PARAMETER_LIST,
                ffi::SCOPE_GLOBAL,
                0,
                &mut size,
                ptr::null_mut(),
            )
        };
        let count = size as usize / size_of::<u32>();
        if status != 0 || count == 0 {
            return Vec::new();
        }
        let mut ids = vec![0u32; count];
        // SAFETY: `ids` has room for `size` bytes.
        let status = unsafe {
            ffi::AudioUnitGetProperty(
                self.unit,
                ffi::PROP_PARAMETER_LIST,
                ffi::SCOPE_GLOBAL,
                0,
                ids.as_mut_ptr().cast(),
                &mut size,
            )
        };
        if status != 0 {
            return Vec::new();
        }
        ids.truncate(size as usize / size_of::<u32>());
        ids.into_iter().filter_map(|id| self.param(id)).collect()
    }

    fn param(&self, id: u32) -> Option<ParamInfo> {
        // SAFETY: the struct is plain data; all-zero is a valid value.
        let mut info: ffi::AudioUnitParameterInfo = unsafe { std::mem::zeroed() };
        let mut size = size_of::<ffi::AudioUnitParameterInfo>() as u32;
        // SAFETY: `info` is writable and `size` is its size.
        let status = unsafe {
            ffi::AudioUnitGetProperty(
                self.unit,
                ffi::PROP_PARAMETER_INFO,
                ffi::SCOPE_GLOBAL,
                id,
                (&mut info as *mut ffi::AudioUnitParameterInfo).cast(),
                &mut size,
            )
        };
        if status != 0 {
            return None;
        }
        let mut name = String::new();
        if info.flags & ffi::PARAM_FLAG_HAS_CF_NAME != 0 && !info.cfNameString.is_null() {
            // SAFETY: the flag says the field holds a valid CFString, and
            // the release flag says the host owns it.
            unsafe {
                name = ffi::cf_string(info.cfNameString);
                if info.flags & ffi::PARAM_FLAG_CF_NAME_RELEASE != 0 {
                    ffi::CFRelease(info.cfNameString);
                }
            }
        }
        if name.is_empty() {
            let end = info
                .name
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(info.name.len());
            name = String::from_utf8_lossy(&info.name[..end]).into_owned();
        }
        if name.is_empty() {
            name = format!("Parameter {id}");
        }
        let (min, max) = (info.minValue, info.maxValue);
        if info.flags & ffi::PARAM_FLAG_IS_WRITABLE == 0
            || !(min.is_finite() && max.is_finite() && min <= max)
        {
            return None;
        }
        let unit = match info.unit {
            ffi::UNIT_BOOLEAN => ParamUnit::Toggle,
            ffi::UNIT_INDEXED | ffi::UNIT_MIDI_NOTE | ffi::UNIT_MIDI_CONTROLLER => {
                ParamUnit::Integer
            }
            ffi::UNIT_PERCENT => ParamUnit::Percent,
            ffi::UNIT_SECONDS => ParamUnit::Seconds,
            ffi::UNIT_SAMPLE_FRAMES => ParamUnit::Samples,
            ffi::UNIT_HERTZ => ParamUnit::Hertz,
            ffi::UNIT_CENTS | ffi::UNIT_ABSOLUTE_CENTS => ParamUnit::Cents,
            ffi::UNIT_RELATIVE_SEMITONES => ParamUnit::Semitones,
            ffi::UNIT_DECIBELS => ParamUnit::Decibels,
            ffi::UNIT_BEATS => ParamUnit::Beats,
            ffi::UNIT_MILLISECONDS => ParamUnit::Milliseconds,
            ffi::UNIT_RATIO => ParamUnit::Ratio,
            _ => ParamUnit::Generic,
        };
        let default = if info.defaultValue.is_finite() {
            info.defaultValue.clamp(min, max)
        } else {
            min
        };
        let param = ParamInfo::new(ParamId(id), "", min, max, default, unit).with_name(name);
        Some(
            if info.flags & ffi::PARAM_FLAG_DISPLAY_LOGARITHMIC != 0 && min > 0.0 {
                param.logarithmic()
            } else {
                param
            },
        )
    }

    /// Describes the unit for the plugin database.
    pub fn info(&self, component: &AuComponent) -> PluginInfo {
        let inputs = self.input_buses();
        PluginInfo {
            format: PluginFormat::Au,
            id: component.id.clone(),
            name: component.name.clone(),
            vendor: component.vendor.clone(),
            version: component.version.clone(),
            category: if self.is_instrument() {
                DeviceCategory::Instrument
            } else {
                DeviceCategory::Effect
            },
            path: std::path::PathBuf::new(),
            features: component
                .id
                .split('.')
                .next()
                .map(str::to_owned)
                .into_iter()
                .collect(),
            params: self.params(),
            audio_inputs: if inputs > 0 {
                self.channels(ffi::SCOPE_INPUT, 0)
            } else {
                0
            },
            audio_outputs: self.channels(ffi::SCOPE_OUTPUT, 0),
            sidechain: inputs >= 2,
            note_input: self.takes_midi(),
            note_output: false,
            loadable: true,
        }
    }

    /// Sets the stream formats for one channel layout and initialises the
    /// unit. Returns the sidechain channel count on success.
    fn try_layout(&mut self, sample_rate: f64, inputs: u32, outputs: u32) -> Option<u32> {
        let format =
            |channels| ffi::AudioStreamBasicDescription::float_planar(sample_rate, channels);
        let input_buses = self.input_buses();
        if (inputs > 0) != (input_buses > 0) {
            return None;
        }
        if self.set(
            ffi::PROP_STREAM_FORMAT,
            ffi::SCOPE_OUTPUT,
            0,
            &format(outputs),
        ) != 0
        {
            return None;
        }
        if inputs > 0
            && self.set(
                ffi::PROP_STREAM_FORMAT,
                ffi::SCOPE_INPUT,
                0,
                &format(inputs),
            ) != 0
        {
            return None;
        }
        let mut sidechain = 0;
        if input_buses >= 2 {
            // A second input bus is taken to be a sidechain. A unit that
            // refuses the format keeps its own and simply gets no sidechain.
            for channels in [inputs, 1, 2] {
                if self.set(
                    ffi::PROP_STREAM_FORMAT,
                    ffi::SCOPE_INPUT,
                    1,
                    &format(channels),
                ) == 0
                {
                    sidechain = channels;
                    break;
                }
            }
        }
        // SAFETY: valid unit, not initialised.
        if unsafe { ffi::AudioUnitInitialize(self.unit) } != 0 {
            return None;
        }
        self.initialized = true;
        Some(sidechain)
    }

    /// Initialises the unit for rendering. The input scratch buffers are
    /// (re)allocated here, so the audio thread must not be rendering.
    pub fn activate(&mut self, request: &Activation) -> Result<Activated, HostError> {
        self.deactivate();
        let max = ffi::MAX_CHANNELS as u32;
        let bus = request.bus_channels.clamp(1, max);

        let status = self.set(
            ffi::PROP_MAX_FRAMES_PER_SLICE,
            ffi::SCOPE_GLOBAL,
            0,
            &request.max_block,
        );
        if status != 0 {
            return Err(HostError::Activate(status_text(
                "setting the maximum block size",
                status,
            )));
        }

        let has_inputs = self.input_buses() > 0;
        if has_inputs {
            for element in 0..self.input_buses().min(2) {
                let callback = ffi::AURenderCallbackStruct {
                    inputProc: Some(render::render_input),
                    inputProcRefCon: self.input.cast(),
                };
                let status = self.set(
                    ffi::PROP_SET_RENDER_CALLBACK,
                    ffi::SCOPE_INPUT,
                    element,
                    &callback,
                );
                if status != 0 && element == 0 {
                    return Err(HostError::Activate(status_text(
                        "setting the input callback",
                        status,
                    )));
                }
            }
        }
        // Tempo and position for units that ask. Many do not support the
        // property, which is fine.
        let callbacks = ffi::HostCallbackInfo {
            hostUserData: Arc::as_ptr(&self.transport).cast_mut().cast::<c_void>(),
            beatAndTempoProc: Some(render::host_beat_and_tempo),
            musicalTimeLocationProc: Some(render::host_musical_time),
            transportStateProc: Some(render::host_transport_state),
            transportStateProc2: Some(render::host_transport_state2),
        };
        let _ = self.set(ffi::PROP_HOST_CALLBACKS, ffi::SCOPE_GLOBAL, 0, &callbacks);

        // Preferred layout first, then the usual fallbacks.
        let layouts: &[(u32, u32)] = if has_inputs {
            &[(bus, bus), (1, bus), (1, 1), (2, 2), (1, 2)]
        } else {
            &[(0, bus), (0, 2), (0, 1)]
        };
        let mut chosen = None;
        for &(inputs, outputs) in layouts {
            if let Some(sidechain) = self.try_layout(request.sample_rate, inputs, outputs) {
                chosen = Some((inputs, outputs, sidechain));
                break;
            }
        }
        let Some((inputs, outputs, sidechain)) = chosen else {
            return Err(HostError::Activate(format!(
                "the Audio Unit accepts no mono or stereo layout at {} Hz",
                request.sample_rate
            )));
        };

        let mut seconds = 0.0f64;
        let latency = if self.get(ffi::PROP_LATENCY, ffi::SCOPE_GLOBAL, 0, &mut seconds) == 0
            && seconds.is_finite()
            && seconds > 0.0
        {
            (seconds * request.sample_rate).round() as u32
        } else {
            0
        };

        let max_block = request.max_block as usize;
        let buffers = |channels: u32| vec![vec![0.0f32; max_block]; channels as usize];
        // SAFETY: the pointer came from `Box::into_raw` in `create` and is
        // freed only in `drop`. The audio thread is not rendering while
        // `activate` runs (it is called from the device's `prepare`), so
        // this is the only reference.
        let input = unsafe { &mut *self.input };
        input.ports = [buffers(inputs), buffers(sidechain)];
        input.max_block = max_block;

        Ok(Activated {
            inputs,
            outputs,
            sidechain,
            latency,
        })
    }

    /// Uninitialises the unit if it is initialised.
    pub fn deactivate(&mut self) {
        if self.initialized {
            // SAFETY: valid, initialised unit.
            let _ = unsafe { ffi::AudioUnitUninitialize(self.unit) };
            self.initialized = false;
        }
    }

    /// Serialises the unit's `ClassInfo` property list as a binary plist.
    pub fn save_state(&self) -> Result<Vec<u8>, HostError> {
        let mut plist: ffi::CFPropertyListRef = ptr::null();
        let status = self.get(ffi::PROP_CLASS_INFO, ffi::SCOPE_GLOBAL, 0, &mut plist);
        if status != 0 || plist.is_null() {
            return Err(HostError::State(status_text(
                "reading the class info",
                status,
            )));
        }
        // SAFETY: `plist` is a property list we own; the data, when not
        // null, is ours too and is read within its length.
        unsafe {
            let data = ffi::CFPropertyListCreateData(
                ptr::null(),
                plist,
                ffi::CF_PLIST_BINARY,
                0,
                ptr::null_mut(),
            );
            ffi::CFRelease(plist);
            if data.is_null() {
                return Err(HostError::State(
                    "the class info could not be serialised".into(),
                ));
            }
            let length = usize::try_from(ffi::CFDataGetLength(data)).unwrap_or(0);
            let bytes = std::slice::from_raw_parts(ffi::CFDataGetBytePtr(data), length).to_vec();
            ffi::CFRelease(data);
            Ok(bytes)
        }
    }

    /// Restores a blob written by [`save_state`](Self::save_state).
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), HostError> {
        let length = isize::try_from(bytes.len())
            .map_err(|_| HostError::State("the state is too large".into()))?;
        // SAFETY: `bytes` is valid for `length`; every object created here
        // is released here.
        let plist = unsafe {
            let data = ffi::CFDataCreate(ptr::null(), bytes.as_ptr(), length);
            if data.is_null() {
                return Err(HostError::State("out of memory".into()));
            }
            let plist = ffi::CFPropertyListCreateWithData(
                ptr::null(),
                data,
                0,
                ptr::null_mut(),
                ptr::null_mut(),
            );
            ffi::CFRelease(data);
            plist
        };
        if plist.is_null() {
            return Err(HostError::State(
                "the state is not an Audio Unit property list".into(),
            ));
        }
        let status = self.set(ffi::PROP_CLASS_INFO, ffi::SCOPE_GLOBAL, 0, &plist);
        // SAFETY: we own `plist`.
        unsafe { ffi::CFRelease(plist) };
        if status != 0 {
            return Err(HostError::State(status_text(
                "restoring the class info",
                status,
            )));
        }
        Ok(())
    }

    pub fn param_value(&self, id: u32) -> Option<f64> {
        let mut value = 0.0f32;
        // SAFETY: valid unit and out pointer.
        let status =
            unsafe { ffi::AudioUnitGetParameter(self.unit, id, ffi::SCOPE_GLOBAL, 0, &mut value) };
        (status == 0).then_some(f64::from(value))
    }
}

impl Drop for AuMain {
    fn drop(&mut self) {
        self.deactivate();
        // SAFETY: the unit was created in `create` and is disposed once.
        // After that nothing can call the input callback, so its context,
        // from `Box::into_raw`, can be freed.
        unsafe {
            ffi::AudioComponentInstanceDispose(self.unit);
            drop(Box::from_raw(self.input));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_ids_round_trip() {
        let id = component_id(
            ffi::fourcc(b"aufx"),
            ffi::fourcc(b"dely"),
            ffi::fourcc(b"appl"),
        );
        assert_eq!(id, "aufx.dely.appl");
        assert_eq!(
            parse_component_id(&id),
            Some((
                ffi::fourcc(b"aufx"),
                ffi::fourcc(b"dely"),
                ffi::fourcc(b"appl")
            ))
        );
        // Spaces are kept; dots and unprintable bytes are written as hex.
        let odd = component_id(ffi::fourcc(b"aumu"), ffi::fourcc(b"dls "), 0x0102_2E04);
        assert_eq!(odd, "aumu.dls .#01022E04");
        assert_eq!(
            parse_component_id(&odd),
            Some((ffi::fourcc(b"aumu"), ffi::fourcc(b"dls "), 0x0102_2E04))
        );
    }

    #[test]
    fn malformed_ids_are_refused() {
        for id in [
            "",
            "aufx",
            "aufx.dely",
            "aufx.dely.appl.more",
            "aufx.del.appl",
            "aufx.#12.appl",
        ] {
            assert_eq!(parse_component_id(id), None, "{id}");
        }
    }
}
