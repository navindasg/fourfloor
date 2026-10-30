//! The callbacks an Audio Unit makes while it renders.
//!
//! All of them can run on the audio thread, so none allocates, locks or
//! logs.

use std::ffi::c_void;
use std::ptr::addr_of_mut;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use fourfloor_core::TransportContext;

use super::ffi::{self, AudioBuffer, AudioBufferList, AudioTimeStamp, Boolean, OSStatus};

/// `paramErr`.
const ERR_PARAM: OSStatus = -50;
/// Beats per bar. The edit model has no time signature yet, so units are
/// told 4/4.
const BEATS_PER_BAR: f64 = 4.0;

/// The audio a unit pulls through its input callbacks.
///
/// The device fills `ports` before each render; the unit reads them (and
/// may process in place) during `AudioUnitRender`, on the same thread.
#[derive(Default)]
pub(crate) struct InputContext {
    /// `ports[bus][channel]`, each `max_block` frames. Bus 0 is the main
    /// input, bus 1 the sidechain.
    pub ports: [Vec<Vec<f32>>; 2],
    pub max_block: usize,
}

/// Hands the unit the input for one bus.
///
/// # Safety
/// `ref_con` must be the `InputContext` registered with the callback, with
/// no other reference to it alive, and `io_data` a valid buffer list.
pub(crate) unsafe extern "C" fn render_input(
    ref_con: *mut c_void,
    _action_flags: *mut u32,
    _time_stamp: *const AudioTimeStamp,
    bus: u32,
    frames: u32,
    io_data: *mut AudioBufferList,
) -> OSStatus {
    if ref_con.is_null() || io_data.is_null() {
        return ERR_PARAM;
    }
    let context = &mut *ref_con.cast::<InputContext>();
    let frames = frames as usize;
    if frames > context.max_block {
        return ffi::ERR_TOO_MANY_FRAMES;
    }
    let Some(port) = context.ports.get_mut(bus as usize) else {
        return ERR_PARAM;
    };
    let bytes = (frames * std::mem::size_of::<f32>()) as u32;
    let count = (*io_data).mNumberBuffers as usize;
    // The list is variable length: index it by pointer, not through the
    // fixed-size array type.
    let buffers = addr_of_mut!((*io_data).mBuffers).cast::<AudioBuffer>();
    for index in 0..count {
        let buffer = buffers.add(index);
        match port.get_mut(index) {
            Some(channel) => {
                (*buffer).mData = channel.as_mut_ptr().cast();
                (*buffer).mDataByteSize = bytes;
            }
            None => {
                // More buffers than this bus has channels: silence, if the
                // unit brought its own memory.
                let data = (*buffer).mData.cast::<f32>();
                if !data.is_null() && (*buffer).mDataByteSize >= bytes {
                    std::slice::from_raw_parts_mut(data, frames).fill(0.0);
                }
            }
        }
    }
    0
}

/// Transport values for the host callbacks. Written by the device before
/// each render; a unit may read them from any thread.
#[derive(Debug, Default)]
pub(crate) struct TransportShared {
    beats: AtomicU64,
    tempo: AtomicU64,
    sample: AtomicU64,
    samples_per_beat: AtomicU64,
    playing: AtomicBool,
    /// Set when `playing` differs from the previous block.
    changed: AtomicBool,
}

impl TransportShared {
    pub fn store(&self, ctx: &TransportContext) {
        let put = |slot: &AtomicU64, value: f64| slot.store(value.to_bits(), Ordering::Relaxed);
        put(&self.beats, ctx.position_beats.get());
        put(&self.tempo, ctx.tempo.bpm());
        put(&self.sample, ctx.position_samples as f64);
        put(
            &self.samples_per_beat,
            ctx.tempo.samples_per_beat(ctx.sample_rate),
        );
        let was = self.playing.swap(ctx.playing, Ordering::Relaxed);
        self.changed.store(was != ctx.playing, Ordering::Relaxed);
    }

    fn get(slot: &AtomicU64) -> f64 {
        f64::from_bits(slot.load(Ordering::Relaxed))
    }
}

/// Writes through an out-pointer the unit may have left null.
unsafe fn put<T>(pointer: *mut T, value: T) {
    if !pointer.is_null() {
        pointer.write(value);
    }
}

/// # Safety
/// `user_data` must be null or the registered `TransportShared`; the out
/// pointers must each be null or writable.
pub(crate) unsafe extern "C" fn host_beat_and_tempo(
    user_data: *mut c_void,
    out_beat: *mut f64,
    out_tempo: *mut f64,
) -> OSStatus {
    let Some(transport) = user_data.cast::<TransportShared>().as_ref() else {
        return ERR_PARAM;
    };
    put(out_beat, TransportShared::get(&transport.beats));
    put(out_tempo, TransportShared::get(&transport.tempo));
    0
}

/// # Safety
/// As [`host_beat_and_tempo`].
pub(crate) unsafe extern "C" fn host_musical_time(
    user_data: *mut c_void,
    out_offset_to_next_beat: *mut u32,
    out_numerator: *mut f32,
    out_denominator: *mut u32,
    out_measure_down_beat: *mut f64,
) -> OSStatus {
    let Some(transport) = user_data.cast::<TransportShared>().as_ref() else {
        return ERR_PARAM;
    };
    let beats = TransportShared::get(&transport.beats);
    let to_next = (beats.ceil() - beats) * TransportShared::get(&transport.samples_per_beat);
    put(out_offset_to_next_beat, to_next.max(0.0) as u32);
    put(out_numerator, BEATS_PER_BAR as f32);
    put(out_denominator, 4);
    put(
        out_measure_down_beat,
        (beats / BEATS_PER_BAR).floor() * BEATS_PER_BAR,
    );
    0
}

/// # Safety
/// As [`host_beat_and_tempo`].
pub(crate) unsafe extern "C" fn host_transport_state(
    user_data: *mut c_void,
    out_playing: *mut Boolean,
    out_changed: *mut Boolean,
    out_sample: *mut f64,
    out_cycling: *mut Boolean,
    out_cycle_start: *mut f64,
    out_cycle_end: *mut f64,
) -> OSStatus {
    let Some(transport) = user_data.cast::<TransportShared>().as_ref() else {
        return ERR_PARAM;
    };
    put(
        out_playing,
        Boolean::from(transport.playing.load(Ordering::Relaxed)),
    );
    put(
        out_changed,
        Boolean::from(transport.changed.load(Ordering::Relaxed)),
    );
    put(out_sample, TransportShared::get(&transport.sample));
    put(out_cycling, 0);
    put(out_cycle_start, 0.0);
    put(out_cycle_end, 0.0);
    0
}

/// # Safety
/// As [`host_beat_and_tempo`].
pub(crate) unsafe extern "C" fn host_transport_state2(
    user_data: *mut c_void,
    out_playing: *mut Boolean,
    out_recording: *mut Boolean,
    out_changed: *mut Boolean,
    out_sample: *mut f64,
    out_cycling: *mut Boolean,
    out_cycle_start: *mut f64,
    out_cycle_end: *mut f64,
) -> OSStatus {
    put(out_recording, 0);
    host_transport_state(
        user_data,
        out_playing,
        out_changed,
        out_sample,
        out_cycling,
        out_cycle_start,
        out_cycle_end,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_core::{SampleRate, Tempo};
    use std::ptr;

    #[test]
    fn the_input_callback_hands_out_the_bus_and_silences_extra_buffers() {
        let mut context = InputContext {
            ports: [vec![vec![1.0; 8], vec![2.0; 8]], vec![vec![3.0; 8]]],
            max_block: 8,
        };
        let ref_con = (&mut context as *mut InputContext).cast::<c_void>();
        let mut spare = [9.0f32; 8];
        let mut list = AudioBufferList::empty();
        list.mNumberBuffers = 3;
        list.mBuffers[2].mData = spare.as_mut_ptr().cast();
        list.mBuffers[2].mDataByteSize = 32;

        // SAFETY: the context and list are valid and not otherwise in use.
        let status =
            unsafe { render_input(ref_con, ptr::null_mut(), ptr::null(), 0, 4, &mut list) };
        assert_eq!(status, 0);
        assert_eq!(list.mBuffers[0].mDataByteSize, 16);
        // SAFETY: the callback pointed these at the context's channels.
        let (left, right) = unsafe {
            (
                *list.mBuffers[0].mData.cast::<f32>(),
                *list.mBuffers[1].mData.cast::<f32>(),
            )
        };
        assert_eq!((left, right), (1.0, 2.0));
        assert_eq!(spare, [0.0, 0.0, 0.0, 0.0, 9.0, 9.0, 9.0, 9.0]);

        list.mNumberBuffers = 1;
        // SAFETY: as above.
        unsafe {
            assert_eq!(
                render_input(ref_con, ptr::null_mut(), ptr::null(), 1, 8, &mut list),
                0
            );
            assert_eq!(*list.mBuffers[0].mData.cast::<f32>(), 3.0);
            assert_eq!(
                render_input(ref_con, ptr::null_mut(), ptr::null(), 0, 9, &mut list),
                ffi::ERR_TOO_MANY_FRAMES
            );
            assert_eq!(
                render_input(ref_con, ptr::null_mut(), ptr::null(), 2, 4, &mut list),
                ERR_PARAM
            );
            assert_eq!(
                render_input(
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null(),
                    0,
                    4,
                    &mut list
                ),
                ERR_PARAM
            );
        }
    }

    #[test]
    fn host_callbacks_report_the_transport_and_tolerate_null_pointers() {
        let transport = TransportShared::default();
        let rate = SampleRate::HZ_48000;
        let tempo = Tempo::new(120.0).unwrap();
        // 5.5 beats in at 120 BPM: 2.75 s.
        transport.store(&TransportContext::playing_at(rate, tempo, 132_000));
        let user = (&transport as *const TransportShared)
            .cast_mut()
            .cast::<c_void>();

        let (mut beat, mut bpm) = (0.0, 0.0);
        let (mut offset, mut numerator, mut denominator, mut down_beat) = (0u32, 0.0f32, 0u32, 0.0);
        let (mut playing, mut changed, mut sample) = (0u8, 0u8, 0.0);
        // SAFETY: valid user data and out pointers (or null where allowed).
        unsafe {
            assert_eq!(host_beat_and_tempo(user, &mut beat, &mut bpm), 0);
            assert_eq!(
                host_beat_and_tempo(user, ptr::null_mut(), ptr::null_mut()),
                0
            );
            assert_eq!(
                host_musical_time(
                    user,
                    &mut offset,
                    &mut numerator,
                    &mut denominator,
                    &mut down_beat
                ),
                0
            );
            assert_eq!(
                host_transport_state2(
                    user,
                    &mut playing,
                    ptr::null_mut(),
                    &mut changed,
                    &mut sample,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
                0
            );
            assert_eq!(
                host_beat_and_tempo(ptr::null_mut(), &mut beat, &mut bpm),
                ERR_PARAM
            );
        }
        assert!((beat - 5.5).abs() < 1e-9 && (bpm - 120.0).abs() < 1e-9);
        assert_eq!(offset, 12_000, "half a beat to the next one");
        assert_eq!((numerator, denominator, down_beat), (4.0, 4, 4.0));
        assert_eq!((playing, changed, sample), (1, 1, 132_000.0));

        transport.store(&TransportContext::playing_at(rate, tempo, 132_512));
        // SAFETY: as above.
        unsafe {
            host_transport_state(
                user,
                &mut playing,
                &mut changed,
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
                ptr::null_mut(),
            );
        }
        assert_eq!((playing, changed), (1, 0), "still playing: no change");
    }
}
