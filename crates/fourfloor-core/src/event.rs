//! Note and controller events delivered to devices with sample accuracy.
//!
//! An [`Event`] is a small `Copy` value: a frame offset inside the current
//! block plus an [`EventKind`]. The engine hands a device its events for the
//! block as a slice sorted by frame ([`crate::ProcessIo::events`]). A device
//! that produces events (a clip player, an arpeggiator) pushes them into an
//! [`EventBuffer`] ([`crate::ProcessIo::events_out`]).
//!
//! [`EventBuffer`] allocates once, in [`EventBuffer::with_capacity`].
//! Pushing, clearing and reading never allocate, so it is safe on the audio
//! thread.

/// What happened.
///
/// Values follow MIDI 1.0: pitches and controller numbers are 0 to 127,
/// channels are 0 to 15.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum EventKind {
    /// A note starts.
    NoteOn {
        /// MIDI pitch, 0 to 127 (60 is middle C, 69 is A 440 Hz).
        pitch: u8,
        /// MIDI velocity, 1 to 127.
        velocity: u8,
        /// MIDI channel, 0 to 15.
        channel: u8,
    },
    /// A note ends. Instruments release every voice playing `pitch` on
    /// `channel`.
    NoteOff {
        /// MIDI pitch, 0 to 127.
        pitch: u8,
        /// Release velocity, 0 to 127. Most sources send 0.
        velocity: u8,
        /// MIDI channel, 0 to 15.
        channel: u8,
    },
    /// A continuous controller changed (mod wheel is 1, sustain pedal 64).
    ControlChange {
        /// Controller number, 0 to 127.
        controller: u8,
        /// Controller value, 0 to 127.
        value: u8,
        /// MIDI channel, 0 to 15.
        channel: u8,
    },
    /// The pitch wheel moved.
    PitchBend {
        /// Bend amount, -8192 to 8191; 0 is centred.
        value: i16,
        /// MIDI channel, 0 to 15.
        channel: u8,
    },
    /// Release every sounding note on every channel. Sent when the
    /// transport stops or jumps and the sender no longer knows which notes
    /// are held.
    AllNotesOff,
}

impl EventKind {
    /// A note-on on channel 0.
    pub const fn note_on(pitch: u8, velocity: u8) -> Self {
        Self::NoteOn {
            pitch,
            velocity,
            channel: 0,
        }
    }

    /// A note-off on channel 0 with release velocity 0.
    pub const fn note_off(pitch: u8) -> Self {
        Self::NoteOff {
            pitch,
            velocity: 0,
            channel: 0,
        }
    }

    /// Whether this is a note-on.
    pub const fn is_note_on(&self) -> bool {
        matches!(self, Self::NoteOn { .. })
    }

    /// Whether this is a note-off.
    pub const fn is_note_off(&self) -> bool {
        matches!(self, Self::NoteOff { .. })
    }
}

/// Frequency in Hz of a MIDI pitch in twelve-tone equal temperament with
/// A4 (pitch 69) at 440 Hz. Fractional pitches are allowed.
pub fn pitch_to_hz(pitch: f32) -> f32 {
    440.0 * 2.0f32.powf((pitch - 69.0) / 12.0)
}

/// MIDI velocity as a linear factor: 127 is 1.0, 0 is 0.0.
pub fn velocity_to_unit(velocity: u8) -> f32 {
    f32::from(velocity.min(127)) / 127.0
}

/// One event at a frame offset inside the current block.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Event {
    /// Frame within the block at which the event takes effect. Always less
    /// than the block's frame count.
    pub frame: u32,
    /// What happened.
    pub kind: EventKind,
}

impl Event {
    /// An event at `frame`.
    pub const fn new(frame: u32, kind: EventKind) -> Self {
        Self { frame, kind }
    }

    /// A note-on on channel 0 at `frame`.
    pub const fn note_on(frame: u32, pitch: u8, velocity: u8) -> Self {
        Self::new(frame, EventKind::note_on(pitch, velocity))
    }

    /// A note-off on channel 0 at `frame`.
    pub const fn note_off(frame: u32, pitch: u8) -> Self {
        Self::new(frame, EventKind::note_off(pitch))
    }
}

// Events travel through preallocated buffers and lock-free queues.
const _: () = {
    const fn assert_rt_safe<T: Copy + Send + Sync + 'static>() {}
    assert_rt_safe::<Event>();
    assert!(core::mem::size_of::<Event>() <= 12);
    assert!(!core::mem::needs_drop::<Event>());
};

/// A fixed-capacity list of [`Event`]s kept sorted by frame.
///
/// Events with the same frame keep the order they were pushed in. All
/// memory is allocated by [`with_capacity`](EventBuffer::with_capacity);
/// nothing else allocates.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct EventBuffer {
    events: Vec<Event>,
    capacity: usize,
    dropped: u32,
}

/// A clone keeps the capacity of the original, so it is as real-time safe
/// as the buffer it was cloned from. Cloning allocates.
impl Clone for EventBuffer {
    fn clone(&self) -> Self {
        let mut events = Vec::with_capacity(self.capacity);
        events.extend_from_slice(&self.events);
        Self {
            events,
            capacity: self.capacity,
            dropped: self.dropped,
        }
    }
}

impl EventBuffer {
    /// An empty buffer that holds up to `capacity` events. Allocates; call
    /// it off the audio thread.
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            events: Vec::with_capacity(capacity),
            capacity,
            dropped: 0,
        }
    }

    /// Most events the buffer can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Number of events held.
    pub fn len(&self) -> usize {
        self.events.len()
    }

    /// Whether the buffer holds no events.
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Whether the buffer is at capacity.
    pub fn is_full(&self) -> bool {
        self.events.len() >= self.capacity
    }

    /// Removes every event. Keeps the allocation.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Adds an event, keeping the buffer sorted by frame. Returns `false`
    /// and counts the event as dropped when the buffer is full. Never
    /// allocates.
    pub fn push(&mut self, event: Event) -> bool {
        if self.is_full() {
            self.dropped = self.dropped.saturating_add(1);
            return false;
        }
        // Events almost always arrive in order, so look from the back.
        let at = self
            .events
            .iter()
            .rposition(|held| held.frame <= event.frame)
            .map_or(0, |index| index + 1);
        self.events.insert(at, event);
        true
    }

    /// Adds every event of `other`, keeping the result sorted. Returns how
    /// many did not fit.
    pub fn extend_from(&mut self, other: &[Event]) -> usize {
        other.iter().filter(|event| !self.push(**event)).count()
    }

    /// The events, sorted by frame.
    pub fn as_slice(&self) -> &[Event] {
        &self.events
    }

    /// Iterates the events in frame order.
    pub fn iter(&self) -> core::slice::Iter<'_, Event> {
        self.events.iter()
    }

    /// Events refused since the last call because the buffer was full.
    /// Reading resets the count.
    pub fn take_dropped(&mut self) -> u32 {
        core::mem::take(&mut self.dropped)
    }
}

impl<'a> IntoIterator for &'a EventBuffer {
    type Item = &'a Event;
    type IntoIter = core::slice::Iter<'a, Event>;

    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_keeps_events_sorted_and_stable() {
        let mut buffer = EventBuffer::with_capacity(8);
        assert!(buffer.is_empty());
        for (frame, pitch) in [(10, 1), (5, 2), (10, 3), (0, 4), (5, 5)] {
            assert!(buffer.push(Event::note_on(frame, pitch, 100)));
        }
        let order: Vec<(u32, u8)> = buffer
            .iter()
            .map(|event| match event.kind {
                EventKind::NoteOn { pitch, .. } => (event.frame, pitch),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(order, [(0, 4), (5, 2), (5, 5), (10, 1), (10, 3)]);
        assert_eq!(buffer.len(), 5);
    }

    #[test]
    fn a_full_buffer_refuses_and_counts_drops_without_growing() {
        let mut buffer = EventBuffer::with_capacity(2);
        assert!(buffer.push(Event::note_on(0, 60, 100)));
        assert!(buffer.push(Event::note_off(1, 60)));
        assert!(buffer.is_full());
        assert!(!buffer.push(Event::note_on(2, 61, 100)));
        assert_eq!(buffer.extend_from(&[Event::note_off(3, 61)]), 1);
        assert_eq!(buffer.len(), 2);
        assert_eq!(buffer.capacity(), 2);
        assert_eq!(buffer.take_dropped(), 2);
        assert_eq!(buffer.take_dropped(), 0);
        buffer.clear();
        assert!(buffer.is_empty());
        assert!(buffer.push(Event::new(0, EventKind::AllNotesOff)));
    }

    #[test]
    fn a_clone_keeps_the_preallocated_capacity() {
        let mut buffer = EventBuffer::with_capacity(16);
        buffer.push(Event::note_on(3, 60, 100));
        let copy = buffer.clone();
        assert_eq!(copy, buffer);
        assert_eq!(copy.capacity(), 16);
        assert!(copy.events.capacity() >= 16);
    }

    #[test]
    fn extend_merges_two_sorted_lists() {
        let mut buffer = EventBuffer::with_capacity(8);
        buffer.extend_from(&[Event::note_on(0, 60, 1), Event::note_on(8, 61, 1)]);
        let lost = buffer.extend_from(&[Event::note_off(4, 60), Event::note_off(8, 61)]);
        assert_eq!(lost, 0);
        let frames: Vec<u32> = (&buffer).into_iter().map(|event| event.frame).collect();
        assert_eq!(frames, [0, 4, 8, 8]);
        assert!(buffer.as_slice()[2].kind.is_note_on());
        assert!(buffer.as_slice()[3].kind.is_note_off());
    }

    #[test]
    fn helpers_build_channel_zero_events() {
        assert_eq!(
            EventKind::note_on(60, 100),
            EventKind::NoteOn {
                pitch: 60,
                velocity: 100,
                channel: 0
            }
        );
        assert_eq!(
            Event::note_off(7, 60),
            Event {
                frame: 7,
                kind: EventKind::NoteOff {
                    pitch: 60,
                    velocity: 0,
                    channel: 0
                }
            }
        );
    }

    #[test]
    fn pitch_and_velocity_conversions_hit_known_points() {
        assert!((pitch_to_hz(69.0) - 440.0).abs() < 1e-3);
        assert!((pitch_to_hz(57.0) - 220.0).abs() < 1e-3);
        assert!((pitch_to_hz(60.0) - 261.6256).abs() < 1e-2);
        assert_eq!(velocity_to_unit(127), 1.0);
        assert_eq!(velocity_to_unit(0), 0.0);
        assert_eq!(velocity_to_unit(200), 1.0);
    }
}
