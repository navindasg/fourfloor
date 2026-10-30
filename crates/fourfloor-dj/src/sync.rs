//! The shared clock decks use to follow a tempo master.
//!
//! Every deck publishes its beat position once per block; a deck that is
//! synced reads the master's entry. All of it is lock-free and
//! allocation-free: each entry is a small sequence lock over atomics.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};

/// Most decks a session (and one [`SyncBus`]) can hold.
pub const MAX_DECKS: usize = 4;

const NO_MASTER: usize = usize::MAX;
/// Read attempts before a reader gives up on a torn entry.
const READ_TRIES: usize = 8;

/// What a deck publishes about itself after each block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeckClock {
    /// The deck's frame counter at the moment `beat` refers to.
    pub stamp: u64,
    /// Beat position in the deck's own grid at `stamp`.
    pub beat: f64,
    /// Beats the deck advances per output frame while it plays.
    pub beats_per_frame: f64,
    /// Tempo actually sounding: the track's tempo times the playback rate.
    pub bpm: f64,
    /// Whether the deck is playing.
    pub playing: bool,
}

#[derive(Debug, Default)]
struct Slot {
    sequence: AtomicU32,
    valid: AtomicBool,
    playing: AtomicBool,
    stamp: AtomicU64,
    beat: AtomicU64,
    beats_per_frame: AtomicU64,
    bpm: AtomicU64,
}

/// The clock shared by the decks of one session.
///
/// Decks that share a bus must be processed in the same blocks (as nodes
/// of one engine graph are): a follower compares its own frame counter
/// with the master's.
#[derive(Debug)]
pub struct SyncBus {
    master: AtomicUsize,
    slots: [Slot; MAX_DECKS],
}

impl Default for SyncBus {
    fn default() -> Self {
        Self::new()
    }
}

impl SyncBus {
    /// A bus with no master.
    pub fn new() -> Self {
        Self {
            master: AtomicUsize::new(NO_MASTER),
            slots: Default::default(),
        }
    }

    /// The deck the others follow, if any.
    pub fn master(&self) -> Option<usize> {
        let master = self.master.load(Ordering::Relaxed);
        (master < MAX_DECKS).then_some(master)
    }

    /// Chooses the tempo master (`None` for no master).
    pub fn set_master(&self, deck: Option<usize>) {
        let value = deck.filter(|&deck| deck < MAX_DECKS).unwrap_or(NO_MASTER);
        self.master.store(value, Ordering::Relaxed);
    }

    /// Audio thread: publishes deck `deck`'s clock, or `None` when it has
    /// no track with a beat grid.
    pub fn publish(&self, deck: usize, clock: Option<DeckClock>) {
        let Some(slot) = self.slots.get(deck) else {
            return;
        };
        slot.sequence.fetch_add(1, Ordering::AcqRel);
        match clock {
            Some(clock) => {
                slot.stamp.store(clock.stamp, Ordering::Relaxed);
                slot.beat.store(clock.beat.to_bits(), Ordering::Relaxed);
                slot.beats_per_frame
                    .store(clock.beats_per_frame.to_bits(), Ordering::Relaxed);
                slot.bpm.store(clock.bpm.to_bits(), Ordering::Relaxed);
                slot.playing.store(clock.playing, Ordering::Relaxed);
                slot.valid.store(true, Ordering::Relaxed);
            }
            None => slot.valid.store(false, Ordering::Relaxed),
        }
        slot.sequence.fetch_add(1, Ordering::AcqRel);
    }

    /// The clock deck `deck` last published. `None` when it has none, or
    /// when a writer kept interrupting the read.
    pub fn read(&self, deck: usize) -> Option<DeckClock> {
        let slot = self.slots.get(deck)?;
        for _ in 0..READ_TRIES {
            let before = slot.sequence.load(Ordering::Acquire);
            if before % 2 == 1 {
                std::hint::spin_loop();
                continue;
            }
            let valid = slot.valid.load(Ordering::Relaxed);
            let clock = DeckClock {
                stamp: slot.stamp.load(Ordering::Relaxed),
                beat: f64::from_bits(slot.beat.load(Ordering::Relaxed)),
                beats_per_frame: f64::from_bits(slot.beats_per_frame.load(Ordering::Relaxed)),
                bpm: f64::from_bits(slot.bpm.load(Ordering::Relaxed)),
                playing: slot.playing.load(Ordering::Relaxed),
            };
            if slot.sequence.load(Ordering::Acquire) == before {
                return valid.then_some(clock);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_and_reads_back() {
        let bus = SyncBus::new();
        assert_eq!(bus.master(), None);
        assert_eq!(bus.read(0), None);
        let clock = DeckClock {
            stamp: 512,
            beat: 3.25,
            beats_per_frame: 1.0 / 24_000.0,
            bpm: 120.0,
            playing: true,
        };
        bus.publish(1, Some(clock));
        assert_eq!(bus.read(1), Some(clock));
        bus.publish(1, None);
        assert_eq!(bus.read(1), None);
        bus.set_master(Some(1));
        assert_eq!(bus.master(), Some(1));
        bus.set_master(Some(9));
        assert_eq!(bus.master(), None);
        bus.publish(9, Some(clock));
        assert_eq!(bus.read(9), None);
    }
}
