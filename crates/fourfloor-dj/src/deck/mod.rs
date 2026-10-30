//! A DJ deck as an engine [`Device`]: plays one preloaded track with
//! varispeed or key lock, cue points, loops and hot cues, and follows the
//! session's tempo master through a [`SyncBus`].
//!
//! The control side talks to the deck through a [`DeckHandle`]: commands go
//! in through a lock-free queue and are carried out at the start of the
//! next block; the deck reports its state through atomics, and hands
//! replaced tracks back so they are freed off the audio thread.

mod command;
mod playhead;
mod source;
mod status;
#[cfg(feature = "stretch")]
mod stretch;
mod wsola;

use std::sync::atomic::Ordering;
use std::sync::Arc;

use fourfloor_core::{Device, PrepareConfig, ProcessIo, TransportContext};
use rtrb::{Consumer, Producer, RingBuffer};

pub use command::{DeckCommand, SyncMode};
pub use playhead::LoopRegion;
pub use status::DeckSnapshot;

use crate::error::DjError;
use crate::sync::{DeckClock, SyncBus};
use crate::track::{DeckTrack, MAX_STEMS};
use playhead::Playhead;
use status::DeckStatus;
use wsola::Wsola;

/// Hot cue slots per deck.
pub const HOT_CUES: usize = 8;
/// Slowest playback rate a deck accepts.
pub const MIN_RATE: f64 = 0.25;
/// Fastest playback rate a deck accepts.
pub const MAX_RATE: f64 = 4.0;
/// Commands that fit in a deck's queue.
pub const COMMAND_CAPACITY: usize = 256;
/// Largest share by which phase correction may change the rate.
const MAX_CORRECTION: f64 = 0.1;
/// Time constant of beat-phase correction, in seconds.
const SETTLE_SECONDS: f64 = 0.1;
/// Replaced tracks the deck can hold while the control side is not
/// collecting them.
const RETIRED_CAPACITY: usize = 16;
/// Stem gain glide, in seconds.
const STEM_GLIDE_SECONDS: f64 = 0.005;

/// A DJ deck (see the [module docs](self)).
pub struct Deck {
    index: usize,
    commands: Consumer<DeckCommand>,
    retired: Producer<Arc<DeckTrack>>,
    status: Arc<DeckStatus>,
    bus: Arc<SyncBus>,
    track: Option<Arc<DeckTrack>>,
    head: Playhead,
    engine_rate: f64,
    /// Track sample rate over engine sample rate.
    track_ratio: f64,
    playing: bool,
    at_end: bool,
    roll: bool,
    pitch: f64,
    bend: f64,
    key_lock: bool,
    sync: SyncMode,
    quantize: bool,
    cue_point: f64,
    hot_cues: [f64; HOT_CUES],
    gains: [f32; MAX_STEMS],
    target_gains: [f32; MAX_STEMS],
    glide: f32,
    /// Extra beats this deck sits ahead of the master in beat sync (from
    /// nudges and bends while synced).
    phase_offset: f64,
    snap_pending: bool,
    restart_pending: bool,
    frames: u64,
    rate_now: f64,
    wsola: Wsola,
    #[cfg(feature = "stretch")]
    stretch: stretch::StretchLock,
}

/// The control side of a [`Deck`].
pub struct DeckHandle {
    index: usize,
    commands: Producer<DeckCommand>,
    retired: Consumer<Arc<DeckTrack>>,
    status: Arc<DeckStatus>,
}

impl Deck {
    /// A deck with index `index` on `bus`, and the handle that drives it.
    pub fn new(index: usize, bus: Arc<SyncBus>) -> (Self, DeckHandle) {
        let (command_tx, command_rx) = RingBuffer::new(COMMAND_CAPACITY);
        let (retired_tx, retired_rx) = RingBuffer::new(RETIRED_CAPACITY);
        let status = Arc::new(DeckStatus::new());
        let deck = Self {
            index,
            commands: command_rx,
            retired: retired_tx,
            status: Arc::clone(&status),
            bus,
            track: None,
            head: Playhead::new(),
            engine_rate: 48_000.0,
            track_ratio: 1.0,
            playing: false,
            at_end: false,
            roll: false,
            pitch: 1.0,
            bend: 0.0,
            key_lock: false,
            sync: SyncMode::Off,
            quantize: true,
            cue_point: 0.0,
            hot_cues: [f64::NAN; HOT_CUES],
            gains: [1.0; MAX_STEMS],
            target_gains: [1.0; MAX_STEMS],
            glide: 1.0,
            phase_offset: 0.0,
            snap_pending: false,
            restart_pending: true,
            frames: 0,
            rate_now: 1.0,
            wsola: Wsola::new(),
            #[cfg(feature = "stretch")]
            stretch: stretch::StretchLock::new(),
        };
        let handle = DeckHandle {
            index,
            commands: command_tx,
            retired: retired_rx,
            status,
        };
        (deck, handle)
    }

    /// Which key lock this build uses: `"signalsmith"` with the `stretch`
    /// feature, `"wsola"` otherwise.
    pub const fn key_lock_engine() -> &'static str {
        if cfg!(feature = "stretch") {
            "signalsmith"
        } else {
            "wsola"
        }
    }

    fn restart(&mut self) {
        self.restart_pending = true;
    }

    /// Hands a replaced track back for freeing. When the queue is full
    /// the track is leaked rather than freed here.
    fn retire(&mut self, track: Arc<DeckTrack>) {
        if let Err(rtrb::PushError::Full(track)) = self.retired.push(track) {
            std::mem::forget(track);
            self.status.leaked.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// The rate for this block: pitch fader or the master's tempo, with
    /// bend and beat-phase correction.
    fn block_rate(&mut self, frames: usize) -> f64 {
        let Some(grid) = self.track.as_ref().and_then(|t| t.grid()).copied() else {
            return (self.pitch * (1.0 + self.bend)).clamp(MIN_RATE, MAX_RATE);
        };
        let master = self.bus.master().filter(|&m| m != self.index);
        let clock = match (self.sync, master) {
            (SyncMode::Off, _) | (_, None) => None,
            (_, Some(master)) => self.bus.read(master),
        };
        let Some(clock) = clock else {
            return (self.pitch * (1.0 + self.bend)).clamp(MIN_RATE, MAX_RATE);
        };
        let mut rate = clock.bpm / grid.bpm;
        if self.sync == SyncMode::Beat && self.playing && clock.playing {
            let elapsed = self.frames as f64 - clock.stamp as f64;
            let master_beat = clock.beat + elapsed * clock.beats_per_frame;
            let own_beat = grid.beat_at(self.head.pos);
            let error = master_beat + self.phase_offset - own_beat;
            let error = error - error.round();
            if self.snap_pending {
                self.head.pos = self
                    .head
                    .clamp(self.head.pos + error * grid.samples_per_beat());
                self.restart();
            } else if rate > 0.0 {
                let settle = SETTLE_SECONDS * self.engine_rate;
                let base = rate * self.track_ratio / grid.samples_per_beat();
                let correction = error / settle / base;
                rate *= 1.0 + correction.clamp(-MAX_CORRECTION, MAX_CORRECTION);
            }
            self.snap_pending = false;
            // A bend while synced moves the phase target with it, so the
            // correction does not pull it straight back.
            if self.bend != 0.0 {
                let beats_per_frame = rate * self.track_ratio / grid.samples_per_beat();
                self.phase_offset += self.bend * beats_per_frame * frames as f64;
            }
        }
        (rate * (1.0 + self.bend)).clamp(MIN_RATE, MAX_RATE)
    }

    fn glide_gains(&mut self) {
        for (gain, &target) in self.gains.iter_mut().zip(&self.target_gains) {
            *gain = crate::inner::glide(*gain, target, self.glide);
        }
    }

    /// Plays `frames` frames into `left` and `right`.
    fn render(&mut self, left: &mut [f32], right: &mut [f32], step: f64) {
        let Some(track) = self.track.clone() else {
            left.fill(0.0);
            right.fill(0.0);
            return;
        };
        let has_stems = !track.stems().is_empty();
        #[cfg(feature = "stretch")]
        if self.key_lock {
            if !self.playing {
                left.fill(0.0);
                right.fill(0.0);
                return;
            }
            if self.restart_pending {
                self.stretch.restart();
                self.restart_pending = false;
            }
            let ended = self.stretch.render(
                &track,
                &mut self.head,
                &self.gains,
                stretch::Block {
                    step,
                    ratio: self.track_ratio,
                },
                left,
                right,
            );
            if has_stems {
                self.glide_gains();
            }
            if ended {
                self.reach_end();
            }
            return;
        }
        self.wsola.set_ratio(self.track_ratio);
        if self.restart_pending {
            self.wsola.restart();
            self.restart_pending = false;
        }
        for (out_left, out_right) in left.iter_mut().zip(right.iter_mut()) {
            if !self.playing {
                *out_left = 0.0;
                *out_right = 0.0;
                continue;
            }
            if has_stems {
                self.glide_gains();
            }
            let (l, r) = if self.key_lock {
                self.wsola.next(&track, &self.head, &self.gains)
            } else {
                source::interpolated(&track, self.head.pos, &self.gains)
            };
            *out_left = l;
            *out_right = r;
            if self.head.advance(step) {
                self.reach_end();
            }
        }
    }

    fn reach_end(&mut self) {
        self.playing = false;
        self.at_end = true;
        self.head.pos = self.head.end;
        self.head.slip = None;
    }

    fn publish(&self) {
        let status = &self.status;
        status.position.store(self.head.pos);
        status.rate.store(self.rate_now);
        status.cue_point.store(self.cue_point);
        let (start, end) = self
            .head
            .active
            .map_or((f64::NAN, f64::NAN), |r| (r.start, r.end));
        status.loop_start.store(start);
        status.loop_end.store(end);
        for (slot, &cue) in status.hot_cues.iter().zip(&self.hot_cues) {
            slot.store(cue);
        }
        status.playing.store(self.playing, Ordering::Relaxed);
        status.roll.store(self.roll, Ordering::Relaxed);
        status.at_end.store(self.at_end, Ordering::Relaxed);
        let grid = self.track.as_ref().and_then(|t| t.grid());
        status
            .bpm
            .store(grid.map_or(f64::NAN, |g| g.bpm * self.rate_now));
        let clock = grid.map(|grid| DeckClock {
            stamp: self.frames,
            beat: grid.beat_at(self.head.pos),
            beats_per_frame: if self.playing {
                self.rate_now * self.track_ratio / grid.samples_per_beat()
            } else {
                0.0
            },
            bpm: grid.bpm * self.rate_now,
            playing: self.playing,
        });
        self.bus.publish(self.index, clock);
    }
}

impl Device for Deck {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.engine_rate = config.sample_rate.as_f64();
        self.track_ratio = self
            .track
            .as_ref()
            .map_or(1.0, |t| t.sample_rate().as_f64() / self.engine_rate);
        self.glide = (1.0 - (-1.0 / (STEM_GLIDE_SECONDS * self.engine_rate)).exp()) as f32;
        self.wsola.prepare(self.engine_rate);
        #[cfg(feature = "stretch")]
        self.stretch.prepare(config.sample_rate, config.max_block);
        self.restart();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let frames = io.frames();
        self.drain_commands();
        self.rate_now = self.block_rate(frames);
        let step = self.rate_now * self.track_ratio;
        match &mut *io.outputs {
            [left, right, rest @ ..] => {
                self.render(left, right, step);
                for channel in rest {
                    channel.fill(0.0);
                }
            }
            // Decks need a stereo bus; on a mono one they stay silent.
            outputs => {
                for channel in outputs.iter_mut() {
                    channel.fill(0.0);
                }
            }
        }
        self.frames += frames as u64;
        self.publish();
    }

    fn reset(&mut self) {
        self.restart();
    }
}

impl DeckHandle {
    /// Which deck this drives.
    pub fn index(&self) -> usize {
        self.index
    }

    /// Queues a command. Fails when the deck has not been taking commands
    /// (its queue is full).
    pub fn send(&mut self, command: DeckCommand) -> Result<(), DjError> {
        self.commands
            .push(command)
            .map_err(|_| DjError::QueueFull("a deck"))
    }

    /// Frees tracks the deck has replaced. Returns how many.
    pub fn collect(&mut self) -> usize {
        let mut freed = 0;
        while self.retired.pop().is_ok() {
            freed += 1;
        }
        freed
    }

    /// The deck's live state as of its last block.
    pub fn snapshot(&self) -> DeckSnapshot {
        self.status.snapshot()
    }

    /// Tracks the deck had to leak because nothing collected them.
    pub fn leaked(&self) -> u64 {
        self.status.leaked.load(Ordering::Relaxed)
    }
}
