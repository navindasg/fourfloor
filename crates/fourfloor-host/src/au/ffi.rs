//! Hand-written bindings to the parts of AudioToolbox and CoreFoundation the
//! Audio Unit host uses.
//!
//! Only C functions and plain structs are needed for Audio Unit v2, so the
//! bindings are declared here instead of pulling in a bindings crate. Every
//! layout below is copied from the macOS SDK headers
//! (`AudioToolbox/AUComponent.h`, `AudioUnitProperties.h`,
//! `AudioComponent.h`, `CoreAudioTypes/CoreAudioBaseTypes.h`); the tests at
//! the bottom pin the sizes the headers imply.

#![allow(non_snake_case, non_upper_case_globals)]

use std::ffi::c_void;

pub type OSStatus = i32;
pub type AudioComponent = *mut c_void;
pub type AudioUnit = *mut c_void;
pub type CFTypeRef = *const c_void;
pub type CFStringRef = *const c_void;
pub type CFDataRef = *const c_void;
pub type CFPropertyListRef = *const c_void;
pub type CFAllocatorRef = *const c_void;
pub type CFIndex = isize;
pub type Boolean = u8;

/// Builds a four-character code.
pub const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}

pub const TYPE_EFFECT: u32 = fourcc(b"aufx");
pub const TYPE_MUSIC_EFFECT: u32 = fourcc(b"aumf");
pub const TYPE_MUSIC_DEVICE: u32 = fourcc(b"aumu");

/// `kAudioComponentFlag_RequiresAsyncInstantiation`.
pub const FLAG_REQUIRES_ASYNC: u32 = 8;

pub const SCOPE_GLOBAL: u32 = 0;
pub const SCOPE_INPUT: u32 = 1;
pub const SCOPE_OUTPUT: u32 = 2;

pub const PROP_CLASS_INFO: u32 = 0;
pub const PROP_PARAMETER_LIST: u32 = 3;
pub const PROP_PARAMETER_INFO: u32 = 4;
pub const PROP_STREAM_FORMAT: u32 = 8;
pub const PROP_ELEMENT_COUNT: u32 = 11;
pub const PROP_LATENCY: u32 = 12;
pub const PROP_MAX_FRAMES_PER_SLICE: u32 = 14;
pub const PROP_SET_RENDER_CALLBACK: u32 = 23;
pub const PROP_HOST_CALLBACKS: u32 = 27;

/// `kAudioUnitErr_TooManyFramesToProcess`.
pub const ERR_TOO_MANY_FRAMES: OSStatus = -10874;

/// `kAudioFormatLinearPCM`.
pub const FORMAT_LINEAR_PCM: u32 = fourcc(b"lpcm");
/// Native-endian packed float, one buffer per channel.
pub const FORMAT_FLAGS_FLOAT_PLANAR: u32 = 1 | 8 | 32;

/// `kAudioTimeStampSampleTimeValid`.
pub const TIMESTAMP_SAMPLE_TIME_VALID: u32 = 1;

pub const PARAM_FLAG_CF_NAME_RELEASE: u32 = 1 << 4;
pub const PARAM_FLAG_DISPLAY_LOGARITHMIC: u32 = 1 << 22;
pub const PARAM_FLAG_HAS_CF_NAME: u32 = 1 << 27;
pub const PARAM_FLAG_IS_WRITABLE: u32 = 1 << 31;

pub const UNIT_INDEXED: u32 = 1;
pub const UNIT_BOOLEAN: u32 = 2;
pub const UNIT_PERCENT: u32 = 3;
pub const UNIT_SECONDS: u32 = 4;
pub const UNIT_SAMPLE_FRAMES: u32 = 5;
pub const UNIT_HERTZ: u32 = 8;
pub const UNIT_CENTS: u32 = 9;
pub const UNIT_RELATIVE_SEMITONES: u32 = 10;
pub const UNIT_MIDI_NOTE: u32 = 11;
pub const UNIT_MIDI_CONTROLLER: u32 = 12;
pub const UNIT_DECIBELS: u32 = 13;
pub const UNIT_ABSOLUTE_CENTS: u32 = 20;
pub const UNIT_BEATS: u32 = 23;
pub const UNIT_MILLISECONDS: u32 = 24;
pub const UNIT_RATIO: u32 = 25;

/// `kCFStringEncodingUTF8`.
pub const CF_ENCODING_UTF8: u32 = 0x0800_0100;
/// `kCFPropertyListBinaryFormat_v1_0`.
pub const CF_PLIST_BINARY: CFIndex = 200;

/// The most channels one bus of a hosted unit may have.
pub const MAX_CHANNELS: usize = 8;

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioComponentDescription {
    pub componentType: u32,
    pub componentSubType: u32,
    pub componentManufacturer: u32,
    pub componentFlags: u32,
    pub componentFlagsMask: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AudioStreamBasicDescription {
    pub mSampleRate: f64,
    pub mFormatID: u32,
    pub mFormatFlags: u32,
    pub mBytesPerPacket: u32,
    pub mFramesPerPacket: u32,
    pub mBytesPerFrame: u32,
    pub mChannelsPerFrame: u32,
    pub mBitsPerChannel: u32,
    pub mReserved: u32,
}

impl AudioStreamBasicDescription {
    /// 32-bit float, one buffer per channel.
    pub fn float_planar(sample_rate: f64, channels: u32) -> Self {
        Self {
            mSampleRate: sample_rate,
            mFormatID: FORMAT_LINEAR_PCM,
            mFormatFlags: FORMAT_FLAGS_FLOAT_PLANAR,
            mBytesPerPacket: 4,
            mFramesPerPacket: 1,
            mBytesPerFrame: 4,
            mChannelsPerFrame: channels,
            mBitsPerChannel: 32,
            mReserved: 0,
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioBuffer {
    pub mNumberChannels: u32,
    pub mDataByteSize: u32,
    pub mData: *mut c_void,
}

/// An `AudioBufferList` with room for [`MAX_CHANNELS`] buffers. The C type
/// is variable length; this fixed one has the same layout for any count up
/// to its capacity.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct AudioBufferList {
    pub mNumberBuffers: u32,
    pub mBuffers: [AudioBuffer; MAX_CHANNELS],
}

impl AudioBufferList {
    pub const fn empty() -> Self {
        Self {
            mNumberBuffers: 0,
            mBuffers: [AudioBuffer {
                mNumberChannels: 1,
                mDataByteSize: 0,
                mData: std::ptr::null_mut(),
            }; MAX_CHANNELS],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct SMPTETime {
    pub mSubframes: i16,
    pub mSubframeDivisor: i16,
    pub mCounter: u32,
    pub mType: u32,
    pub mFlags: u32,
    pub mHours: i16,
    pub mMinutes: i16,
    pub mSeconds: i16,
    pub mFrames: i16,
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Default)]
pub struct AudioTimeStamp {
    pub mSampleTime: f64,
    pub mHostTime: u64,
    pub mRateScalar: f64,
    pub mWordClockTime: u64,
    pub mSMPTETime: SMPTETime,
    pub mFlags: u32,
    pub mReserved: u32,
}

pub type AURenderCallback = unsafe extern "C" fn(
    inRefCon: *mut c_void,
    ioActionFlags: *mut u32,
    inTimeStamp: *const AudioTimeStamp,
    inBusNumber: u32,
    inNumberFrames: u32,
    ioData: *mut AudioBufferList,
) -> OSStatus;

#[repr(C)]
pub struct AURenderCallbackStruct {
    pub inputProc: Option<AURenderCallback>,
    pub inputProcRefCon: *mut c_void,
}

pub type HostGetBeatAndTempo = unsafe extern "C" fn(
    inHostUserData: *mut c_void,
    outCurrentBeat: *mut f64,
    outCurrentTempo: *mut f64,
) -> OSStatus;

pub type HostGetMusicalTimeLocation = unsafe extern "C" fn(
    inHostUserData: *mut c_void,
    outDeltaSampleOffsetToNextBeat: *mut u32,
    outTimeSig_Numerator: *mut f32,
    outTimeSig_Denominator: *mut u32,
    outCurrentMeasureDownBeat: *mut f64,
) -> OSStatus;

pub type HostGetTransportState = unsafe extern "C" fn(
    inHostUserData: *mut c_void,
    outIsPlaying: *mut Boolean,
    outTransportStateChanged: *mut Boolean,
    outCurrentSampleInTimeLine: *mut f64,
    outIsCycling: *mut Boolean,
    outCycleStartBeat: *mut f64,
    outCycleEndBeat: *mut f64,
) -> OSStatus;

pub type HostGetTransportState2 = unsafe extern "C" fn(
    inHostUserData: *mut c_void,
    outIsPlaying: *mut Boolean,
    outIsRecording: *mut Boolean,
    outTransportStateChanged: *mut Boolean,
    outCurrentSampleInTimeLine: *mut f64,
    outIsCycling: *mut Boolean,
    outCycleStartBeat: *mut f64,
    outCycleEndBeat: *mut f64,
) -> OSStatus;

#[repr(C)]
pub struct HostCallbackInfo {
    pub hostUserData: *mut c_void,
    pub beatAndTempoProc: Option<HostGetBeatAndTempo>,
    pub musicalTimeLocationProc: Option<HostGetMusicalTimeLocation>,
    pub transportStateProc: Option<HostGetTransportState>,
    pub transportStateProc2: Option<HostGetTransportState2>,
}

#[repr(C)]
pub struct AudioUnitParameterInfo {
    pub name: [u8; 52],
    pub unitName: CFStringRef,
    pub clumpID: u32,
    pub cfNameString: CFStringRef,
    pub unit: u32,
    pub minValue: f32,
    pub maxValue: f32,
    pub defaultValue: f32,
    pub flags: u32,
}

#[link(name = "AudioToolbox", kind = "framework")]
extern "C" {
    pub fn AudioComponentFindNext(
        inComponent: AudioComponent,
        inDesc: *const AudioComponentDescription,
    ) -> AudioComponent;
    pub fn AudioComponentGetDescription(
        inComponent: AudioComponent,
        outDesc: *mut AudioComponentDescription,
    ) -> OSStatus;
    pub fn AudioComponentCopyName(
        inComponent: AudioComponent,
        outName: *mut CFStringRef,
    ) -> OSStatus;
    pub fn AudioComponentGetVersion(inComponent: AudioComponent, outVersion: *mut u32) -> OSStatus;
    pub fn AudioComponentInstanceNew(
        inComponent: AudioComponent,
        outInstance: *mut AudioUnit,
    ) -> OSStatus;
    pub fn AudioComponentInstanceDispose(inInstance: AudioUnit) -> OSStatus;

    pub fn AudioUnitInitialize(inUnit: AudioUnit) -> OSStatus;
    pub fn AudioUnitUninitialize(inUnit: AudioUnit) -> OSStatus;
    pub fn AudioUnitGetPropertyInfo(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        outDataSize: *mut u32,
        outWritable: *mut Boolean,
    ) -> OSStatus;
    pub fn AudioUnitGetProperty(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        outData: *mut c_void,
        ioDataSize: *mut u32,
    ) -> OSStatus;
    pub fn AudioUnitSetProperty(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        inData: *const c_void,
        inDataSize: u32,
    ) -> OSStatus;
    pub fn AudioUnitGetParameter(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        outValue: *mut f32,
    ) -> OSStatus;
    pub fn AudioUnitSetParameter(
        inUnit: AudioUnit,
        inID: u32,
        inScope: u32,
        inElement: u32,
        inValue: f32,
        inBufferOffsetInFrames: u32,
    ) -> OSStatus;
    pub fn AudioUnitRender(
        inUnit: AudioUnit,
        ioActionFlags: *mut u32,
        inTimeStamp: *const AudioTimeStamp,
        inOutputBusNumber: u32,
        inNumberFrames: u32,
        ioData: *mut AudioBufferList,
    ) -> OSStatus;
    pub fn AudioUnitReset(inUnit: AudioUnit, inScope: u32, inElement: u32) -> OSStatus;
    pub fn MusicDeviceMIDIEvent(
        inUnit: AudioUnit,
        inStatus: u32,
        inData1: u32,
        inData2: u32,
        inOffsetSampleFrame: u32,
    ) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    pub fn CFRelease(cf: CFTypeRef);
    pub fn CFStringGetLength(string: CFStringRef) -> CFIndex;
    pub fn CFStringGetMaximumSizeForEncoding(length: CFIndex, encoding: u32) -> CFIndex;
    pub fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut u8,
        bufferSize: CFIndex,
        encoding: u32,
    ) -> Boolean;
    pub fn CFDataCreate(allocator: CFAllocatorRef, bytes: *const u8, length: CFIndex) -> CFDataRef;
    pub fn CFDataGetBytePtr(data: CFDataRef) -> *const u8;
    pub fn CFDataGetLength(data: CFDataRef) -> CFIndex;
    pub fn CFPropertyListCreateData(
        allocator: CFAllocatorRef,
        propertyList: CFPropertyListRef,
        format: CFIndex,
        options: usize,
        error: *mut CFTypeRef,
    ) -> CFDataRef;
    pub fn CFPropertyListCreateWithData(
        allocator: CFAllocatorRef,
        data: CFDataRef,
        options: usize,
        format: *mut CFIndex,
        error: *mut CFTypeRef,
    ) -> CFPropertyListRef;
}

/// Copies a `CFString` into a Rust string. A null reference is empty.
///
/// # Safety
/// `string` must be null or a valid `CFStringRef`.
pub unsafe fn cf_string(string: CFStringRef) -> String {
    if string.is_null() {
        return String::new();
    }
    let length = CFStringGetLength(string);
    let size = CFStringGetMaximumSizeForEncoding(length, CF_ENCODING_UTF8) + 1;
    let mut buffer = vec![0u8; usize::try_from(size).unwrap_or(1).max(1)];
    if CFStringGetCString(string, buffer.as_mut_ptr(), size, CF_ENCODING_UTF8) == 0 {
        return String::new();
    }
    let end = buffer.iter().position(|&b| b == 0).unwrap_or(buffer.len());
    String::from_utf8_lossy(&buffer[..end]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn struct_layouts_match_the_sdk_headers() {
        assert_eq!(size_of::<AudioComponentDescription>(), 20);
        assert_eq!(size_of::<AudioStreamBasicDescription>(), 40);
        assert_eq!(size_of::<AudioBuffer>(), 16);
        assert_eq!(offset_of!(AudioBufferList, mBuffers), 8);
        assert_eq!(size_of::<SMPTETime>(), 24);
        assert_eq!(size_of::<AudioTimeStamp>(), 64);
        assert_eq!(offset_of!(AudioTimeStamp, mFlags), 56);
        assert_eq!(size_of::<AudioUnitParameterInfo>(), 104);
        assert_eq!(offset_of!(AudioUnitParameterInfo, unitName), 56);
        assert_eq!(offset_of!(AudioUnitParameterInfo, cfNameString), 72);
        assert_eq!(offset_of!(AudioUnitParameterInfo, flags), 96);
        assert_eq!(size_of::<HostCallbackInfo>(), 40);
        assert_eq!(size_of::<AURenderCallbackStruct>(), 16);
    }

    #[test]
    fn four_character_codes_are_big_endian() {
        assert_eq!(fourcc(b"aufx"), 0x6175_6678);
        assert_eq!(FORMAT_FLAGS_FLOAT_PLANAR, 41);
    }
}
