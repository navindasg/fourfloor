//! MIDI in and out: message parsing, controller mapping with MIDI learn,
//! and MIDI clock.
//!
//! Everything in this module is plain data handling with no device access
//! and no feature flag. The optional [`ports`] module (feature `midi`)
//! connects it to real MIDI ports through `midir`.
//!
//! - [`parse`] turns raw MIDI bytes into a [`MidiMessage`]; [`encode`]
//!   does the reverse for channel messages.
//! - [`ControllerMap`] turns controller messages into parameter changes.
//! - [`MidiLearn`] captures the next controller that moves and binds it to
//!   a parameter.
//! - [`MidiRouter`] turns incoming messages into [`AudioMessage`]s for the
//!   engine: notes to an instrument, mapped controllers to parameters.
//! - [`clock`] works out where MIDI clock pulses fall.

pub mod clock;
#[cfg(feature = "midi")]
pub mod ports;

use fourfloor_core::{AudioMessage, DeviceId, EventKind, ParamId};

pub use clock::{ClockOut, CLOCK, CONTINUE, PULSES_PER_BEAT, START, STOP};

/// One parsed MIDI message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiMessage {
    /// A note, controller or pitch-bend message.
    Event(EventKind),
    /// Timing clock: 24 per quarter note.
    Clock,
    /// Start playing from the beginning.
    Start,
    /// Continue from the current position.
    Continue,
    /// Stop.
    Stop,
}

/// Parses one complete MIDI message. Returns `None` for anything this
/// engine has no use for (aftertouch, program change, sysex, active
/// sensing) and for truncated messages.
///
/// A note-on with velocity zero is a note-off, as the MIDI standard says.
/// Running status is not handled: every message must start with its
/// status byte, which is how OS MIDI APIs deliver them.
pub fn parse(bytes: &[u8]) -> Option<MidiMessage> {
    let status = *bytes.first()?;
    match status {
        clock::CLOCK => return Some(MidiMessage::Clock),
        clock::START => return Some(MidiMessage::Start),
        clock::CONTINUE => return Some(MidiMessage::Continue),
        clock::STOP => return Some(MidiMessage::Stop),
        _ => {}
    }
    let channel = status & 0x0F;
    let data = |index: usize| bytes.get(index).copied().filter(|byte| *byte < 0x80);
    let kind = match status & 0xF0 {
        0x80 => EventKind::NoteOff {
            pitch: data(1)?,
            velocity: data(2)?,
            channel,
        },
        0x90 => {
            let (pitch, velocity) = (data(1)?, data(2)?);
            if velocity == 0 {
                EventKind::NoteOff {
                    pitch,
                    velocity: 0,
                    channel,
                }
            } else {
                EventKind::NoteOn {
                    pitch,
                    velocity,
                    channel,
                }
            }
        }
        0xB0 => {
            let (controller, value) = (data(1)?, data(2)?);
            // Controllers 120 and 123 are "all sound off" and "all notes off".
            if controller == 120 || controller == 123 {
                EventKind::AllNotesOff
            } else {
                EventKind::ControlChange {
                    controller,
                    value,
                    channel,
                }
            }
        }
        0xE0 => {
            let raw = i16::from(data(1)?) | (i16::from(data(2)?) << 7);
            EventKind::PitchBend {
                value: raw - 8_192,
                channel,
            }
        }
        _ => return None,
    };
    Some(MidiMessage::Event(kind))
}

/// Encodes an event as a three-byte MIDI channel message.
/// [`EventKind::AllNotesOff`] becomes controller 123 on channel 0.
pub fn encode(kind: &EventKind) -> [u8; 3] {
    match *kind {
        EventKind::NoteOn {
            pitch,
            velocity,
            channel,
        } => [0x90 | (channel & 15), pitch & 127, velocity & 127],
        EventKind::NoteOff {
            pitch,
            velocity,
            channel,
        } => [0x80 | (channel & 15), pitch & 127, velocity & 127],
        EventKind::ControlChange {
            controller,
            value,
            channel,
        } => [0xB0 | (channel & 15), controller & 127, value & 127],
        EventKind::PitchBend { value, channel } => {
            let raw = (i32::from(value) + 8_192).clamp(0, 16_383);
            [0xE0 | (channel & 15), (raw & 127) as u8, (raw >> 7) as u8]
        }
        EventKind::AllNotesOff => [0xB0, 123, 0],
    }
}

/// A MIDI controller bound to a device parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControllerBinding {
    /// MIDI channel 0 to 15, or `None` to answer on every channel.
    pub channel: Option<u8>,
    /// Controller number, 0 to 127.
    pub controller: u8,
    /// The device that owns the parameter.
    pub device: DeviceId,
    /// The parameter.
    pub param: ParamId,
    /// Parameter value when the controller is at 0.
    pub min: f32,
    /// Parameter value when the controller is at 127.
    pub max: f32,
}

impl ControllerBinding {
    /// Whether this binding answers to a controller message.
    pub fn matches(&self, channel: u8, controller: u8) -> bool {
        self.controller == controller && self.channel.is_none_or(|own| own == channel)
    }

    /// The parameter value for a controller value from 0 to 127.
    pub fn value_at(&self, value: u8) -> f32 {
        self.min + (self.max - self.min) * (f32::from(value.min(127)) / 127.0)
    }
}

/// The controller bindings of a project. A binding for one channel wins
/// over an any-channel binding for the same controller.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ControllerMap {
    bindings: Vec<ControllerBinding>,
}

impl ControllerMap {
    /// A map with the given bindings.
    pub fn new(bindings: Vec<ControllerBinding>) -> Self {
        Self { bindings }
    }

    /// The bindings.
    pub fn bindings(&self) -> &[ControllerBinding] {
        &self.bindings
    }

    /// The binding that answers to a controller message.
    pub fn binding(&self, channel: u8, controller: u8) -> Option<&ControllerBinding> {
        let matching = |exact: bool| {
            self.bindings.iter().find(move |binding| {
                binding.matches(channel, controller) && binding.channel.is_some() == exact
            })
        };
        matching(true).or_else(|| matching(false))
    }

    /// The parameter change a controller message causes, if it is bound.
    /// Does not allocate.
    pub fn resolve(&self, channel: u8, controller: u8, value: u8) -> Option<AudioMessage> {
        self.binding(channel, controller)
            .map(|binding| AudioMessage::SetParam {
                device: binding.device,
                param: binding.param,
                value: binding.value_at(value),
            })
    }
}

/// MIDI learn: arm it with a parameter, then feed it incoming messages;
/// the first controller that moves becomes that parameter's binding.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct MidiLearn {
    armed: Option<(DeviceId, ParamId, f32, f32)>,
    /// Bind to the controller's channel (`true`) or to any channel.
    channel_specific: bool,
}

impl MidiLearn {
    /// A learner that is not armed. `channel_specific` decides whether a
    /// learned binding answers only on the channel it was learned from.
    pub fn new(channel_specific: bool) -> Self {
        Self {
            armed: None,
            channel_specific,
        }
    }

    /// Waits for a controller to bind to `param` of `device`, which it
    /// will sweep from `min` (controller at 0) to `max` (at 127).
    pub fn arm(&mut self, device: DeviceId, param: ParamId, min: f32, max: f32) {
        self.armed = Some((device, param, min, max));
    }

    /// Stops waiting.
    pub fn cancel(&mut self) {
        self.armed = None;
    }

    /// Whether a parameter is waiting for a controller.
    pub fn is_armed(&self) -> bool {
        self.armed.is_some()
    }

    /// Looks at one incoming message. Returns the new binding when the
    /// message is a controller and the learner was armed; the learner is
    /// then disarmed. Notes and clock never bind.
    pub fn feed(&mut self, message: &MidiMessage) -> Option<ControllerBinding> {
        let MidiMessage::Event(EventKind::ControlChange {
            controller,
            channel,
            ..
        }) = *message
        else {
            return None;
        };
        let (device, param, min, max) = self.armed.take()?;
        Some(ControllerBinding {
            channel: self.channel_specific.then_some(channel),
            controller,
            device,
            param,
            min,
            max,
        })
    }
}

/// Turns incoming MIDI into engine messages.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MidiRouter {
    /// Controller bindings.
    pub map: ControllerMap,
    /// The instrument that receives notes, pitch bend and unbound
    /// controllers; `None` drops them.
    pub target: Option<DeviceId>,
}

impl MidiRouter {
    /// A router for `map` that plays `target`.
    pub fn new(map: ControllerMap, target: Option<DeviceId>) -> Self {
        Self { map, target }
    }

    /// The engine message for one parsed MIDI message, if it has one.
    /// Clock and transport messages have none. Does not allocate, so it
    /// may run on a MIDI callback thread.
    pub fn route(&self, message: &MidiMessage) -> Option<AudioMessage> {
        let MidiMessage::Event(kind) = *message else {
            return None;
        };
        if let EventKind::ControlChange {
            controller,
            value,
            channel,
        } = kind
        {
            if let Some(mapped) = self.map.resolve(channel, controller, value) {
                return Some(mapped);
            }
        }
        self.target.map(|device| AudioMessage::SendEvent {
            device,
            event: kind,
        })
    }

    /// Parses raw bytes and routes them.
    pub fn route_bytes(&self, bytes: &[u8]) -> Option<AudioMessage> {
        self.route(&parse(bytes)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CUTOFF: ParamId = ParamId(3);

    fn binding(channel: Option<u8>, controller: u8, device: u64) -> ControllerBinding {
        ControllerBinding {
            channel,
            controller,
            device: DeviceId(device),
            param: CUTOFF,
            min: 100.0,
            max: 1_100.0,
        }
    }

    #[test]
    fn channel_messages_parse_and_encode_back() {
        let cases: [(&[u8], EventKind); 5] = [
            (
                &[0x92, 60, 100],
                EventKind::NoteOn {
                    pitch: 60,
                    velocity: 100,
                    channel: 2,
                },
            ),
            (
                &[0x82, 60, 40],
                EventKind::NoteOff {
                    pitch: 60,
                    velocity: 40,
                    channel: 2,
                },
            ),
            (
                &[0xB1, 74, 127],
                EventKind::ControlChange {
                    controller: 74,
                    value: 127,
                    channel: 1,
                },
            ),
            (
                &[0xE0, 0, 0],
                EventKind::PitchBend {
                    value: -8_192,
                    channel: 0,
                },
            ),
            (
                &[0xEF, 127, 127],
                EventKind::PitchBend {
                    value: 8_191,
                    channel: 15,
                },
            ),
        ];
        for (bytes, kind) in cases {
            assert_eq!(parse(bytes), Some(MidiMessage::Event(kind)), "{bytes:?}");
            assert_eq!(encode(&kind), bytes, "{kind:?}");
        }
        assert_eq!(
            parse(&[0x90, 60, 0]),
            Some(MidiMessage::Event(EventKind::NoteOff {
                pitch: 60,
                velocity: 0,
                channel: 0
            })),
            "note-on with velocity zero is a note-off"
        );
        assert_eq!(
            parse(&[0xB0, 123, 0]),
            Some(MidiMessage::Event(EventKind::AllNotesOff))
        );
        assert_eq!(encode(&EventKind::AllNotesOff), [0xB0, 123, 0]);
    }

    #[test]
    fn realtime_messages_parse_and_junk_is_dropped() {
        assert_eq!(parse(&[0xF8]), Some(MidiMessage::Clock));
        assert_eq!(parse(&[0xFA]), Some(MidiMessage::Start));
        assert_eq!(parse(&[0xFB]), Some(MidiMessage::Continue));
        assert_eq!(parse(&[0xFC]), Some(MidiMessage::Stop));
        for junk in [
            &[][..],
            &[0x90],
            &[0x90, 60],
            &[0x90, 200, 10],
            &[0xC0, 5],
            &[0xF0, 1, 0xF7],
            &[60, 100],
        ] {
            assert_eq!(parse(junk), None, "{junk:?}");
        }
    }

    #[test]
    fn a_controller_map_scales_values_and_prefers_the_exact_channel() {
        let map = ControllerMap::new(vec![binding(None, 74, 1), binding(Some(3), 74, 2)]);
        assert_eq!(map.bindings().len(), 2);
        let set = |device: u64, value: f32| AudioMessage::SetParam {
            device: DeviceId(device),
            param: CUTOFF,
            value,
        };
        assert_eq!(map.resolve(0, 74, 0), Some(set(1, 100.0)));
        assert_eq!(map.resolve(9, 74, 127), Some(set(1, 1_100.0)));
        assert_eq!(
            map.resolve(3, 74, 127),
            Some(set(2, 1_100.0)),
            "channel 3 has its own"
        );
        assert_eq!(map.resolve(0, 75, 64), None);
        let Some(AudioMessage::SetParam { value, .. }) = map.resolve(0, 74, 64) else {
            panic!("bound");
        };
        assert!((value - (100.0 + 1_000.0 * 64.0 / 127.0)).abs() < 1e-3);
    }

    #[test]
    fn midi_learn_binds_the_first_controller_that_moves() {
        let mut learn = MidiLearn::new(true);
        let cc = MidiMessage::Event(EventKind::ControlChange {
            controller: 21,
            value: 5,
            channel: 4,
        });
        assert_eq!(learn.feed(&cc), None, "not armed");
        learn.arm(DeviceId(7), CUTOFF, 0.0, 1.0);
        assert!(learn.is_armed());
        let note = MidiMessage::Event(EventKind::note_on(60, 100));
        assert_eq!(learn.feed(&note), None, "notes do not bind");
        assert_eq!(learn.feed(&MidiMessage::Clock), None);
        let learned = learn.feed(&cc).unwrap();
        assert_eq!(
            learned,
            ControllerBinding {
                channel: Some(4),
                controller: 21,
                device: DeviceId(7),
                param: CUTOFF,
                min: 0.0,
                max: 1.0
            }
        );
        assert!(!learn.is_armed());
        assert_eq!(learn.feed(&cc), None, "one binding per arming");

        let mut omni = MidiLearn::new(false);
        omni.arm(DeviceId(7), CUTOFF, 0.0, 1.0);
        assert_eq!(omni.feed(&cc).unwrap().channel, None);
        omni.arm(DeviceId(7), CUTOFF, 0.0, 1.0);
        omni.cancel();
        assert_eq!(omni.feed(&cc), None);
    }

    #[test]
    fn the_router_sends_notes_to_the_instrument_and_mapped_controllers_to_parameters() {
        let router = MidiRouter::new(
            ControllerMap::new(vec![binding(None, 74, 1)]),
            Some(DeviceId(9)),
        );
        assert_eq!(
            router.route_bytes(&[0x90, 60, 100]),
            Some(AudioMessage::SendEvent {
                device: DeviceId(9),
                event: EventKind::note_on(60, 100)
            })
        );
        assert!(matches!(
            router.route_bytes(&[0xB0, 74, 127]),
            Some(AudioMessage::SetParam { device: DeviceId(1), value, .. }) if value == 1_100.0
        ));
        assert!(
            matches!(
                router.route_bytes(&[0xB0, 1, 64]),
                Some(AudioMessage::SendEvent { .. })
            ),
            "an unbound controller goes to the instrument"
        );
        assert_eq!(router.route_bytes(&[0xF8]), None);
        let no_target = MidiRouter::new(ControllerMap::default(), None);
        assert_eq!(no_target.route_bytes(&[0x90, 60, 100]), None);
    }
}
