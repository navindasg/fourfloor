//! Commands a deck takes from the control side, and how it carries them
//! out. Everything here runs on the audio thread: no allocation.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::playhead::LoopRegion;
use super::{Deck, HOT_CUES};
use crate::track::{DeckTrack, MAX_STEMS};

/// How a deck follows the tempo master.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncMode {
    /// Plays at its own pitch fader.
    #[default]
    Off,
    /// Matches the master's tempo only.
    Tempo,
    /// Matches tempo and keeps its beats on the master's beats.
    Beat,
}

/// One instruction for a deck. Sent through the deck's queue and carried
/// out at the start of the next block.
#[derive(Clone, Debug)]
pub enum DeckCommand {
    /// Loads a track, stopped at its start. The previous track is handed
    /// back to the control side for freeing.
    Load(Arc<DeckTrack>),
    /// Unloads the track.
    Eject,
    /// Starts playback.
    Play,
    /// Stops playback where it is.
    Pause,
    /// CDJ-style cue: while playing, return to the cue point and stop;
    /// while stopped, set the cue point here.
    Cue,
    /// Sets the cue point at the playhead (snapped when quantize is on).
    SetCuePoint,
    /// Moves the playhead to a track sample.
    Seek(f64),
    /// Playback rate from the pitch fader, 1 is normal speed.
    SetPitch(f64),
    /// Temporary rate offset (0.04 is 4 % faster); 0 releases it.
    Bend(f64),
    /// Shifts the playhead by this many track samples.
    Nudge(f64),
    /// Keeps pitch while the rate changes.
    SetKeyLock(bool),
    /// How the deck follows the tempo master.
    SetSync(SyncMode),
    /// Snaps cue points, hot cues and loop starts to the beat grid.
    SetQuantize(bool),
    /// A loop of this many beats from the playhead (start snapped to the
    /// grid when quantize is on). With an active loop its length changes.
    BeatLoop(f64),
    /// A loop roll of this many beats: the track keeps running underneath
    /// and playback continues from there on [`DeckCommand::LoopExit`].
    LoopRoll(f64),
    /// Leaves the active loop (or loop roll).
    LoopExit,
    /// Re-enters the last loop, or leaves the active one.
    Reloop,
    /// Hot cue slot: sets it when empty, jumps to it otherwise.
    HotCue(usize),
    /// Sets a hot cue slot to an exact track sample.
    SetHotCue(usize, f64),
    /// Empties a hot cue slot.
    ClearHotCue(usize),
    /// Jumps forward (or back, when negative) by beats. An active loop
    /// moves with the playhead.
    BeatJump(f64),
    /// Gain of one stem (linear).
    SetStemGain(usize, f32),
}

const MIN_BEATS: f64 = 1.0 / 64.0;

impl Deck {
    /// Carries out every queued command.
    pub(super) fn drain_commands(&mut self) {
        while let Ok(command) = self.commands.pop() {
            self.handle(command);
        }
    }

    fn quantized(&self, position: f64) -> f64 {
        match (self.quantize, self.track.as_ref().and_then(|t| t.grid())) {
            (true, Some(grid)) => self.head.clamp(grid.quantize(position, 1.0)),
            _ => position,
        }
    }

    fn samples_per_beat(&self) -> Option<f64> {
        self.track
            .as_ref()
            .and_then(|track| track.grid())
            .map(|grid| grid.samples_per_beat())
    }

    /// Moves the playhead, leaving a loop it lands outside of.
    fn jump_to(&mut self, position: f64) {
        let position = self.head.clamp(position);
        if let Some(region) = self.head.active {
            if position < region.start || position >= region.end {
                self.head.active = None;
                self.head.slip = None;
                self.roll = false;
            }
        }
        self.head.pos = position;
        self.at_end = false;
        self.restart();
    }

    fn handle(&mut self, command: DeckCommand) {
        match command {
            DeckCommand::Load(track) => self.load(Some(track)),
            DeckCommand::Eject => self.load(None),
            _ if self.track.is_none() => {}
            DeckCommand::Play => {
                if self.head.pos < self.head.end {
                    self.playing = true;
                    self.at_end = false;
                    self.snap_pending = self.sync == SyncMode::Beat;
                    self.restart();
                }
            }
            DeckCommand::Pause => self.playing = false,
            DeckCommand::Cue => {
                if self.playing {
                    self.playing = false;
                    self.jump_to(self.cue_point);
                } else {
                    self.cue_point = self.quantized(self.head.pos);
                    self.jump_to(self.cue_point);
                }
            }
            DeckCommand::SetCuePoint => self.cue_point = self.quantized(self.head.pos),
            DeckCommand::Seek(position) => self.jump_to(position),
            DeckCommand::SetPitch(rate) => {
                if rate.is_finite() {
                    self.pitch = rate.clamp(super::MIN_RATE, super::MAX_RATE);
                }
            }
            DeckCommand::Bend(amount) => {
                if amount.is_finite() {
                    self.bend = amount.clamp(-0.5, 0.5);
                }
            }
            DeckCommand::Nudge(samples) => {
                if samples.is_finite() {
                    if self.sync == SyncMode::Beat {
                        if let Some(spb) = self.samples_per_beat() {
                            self.phase_offset += samples / spb;
                        }
                    }
                    self.head.pos = self.head.clamp(self.head.pos + samples);
                    self.restart();
                }
            }
            DeckCommand::SetKeyLock(on) => {
                self.key_lock = on;
                self.restart();
            }
            DeckCommand::SetSync(mode) => {
                self.sync = mode;
                self.phase_offset = 0.0;
                self.snap_pending = mode == SyncMode::Beat && self.playing;
            }
            DeckCommand::SetQuantize(on) => self.quantize = on,
            DeckCommand::BeatLoop(beats) => self.beat_loop(beats, false),
            DeckCommand::LoopRoll(beats) => self.beat_loop(beats, true),
            DeckCommand::LoopExit => self.loop_exit(),
            DeckCommand::Reloop => {
                if self.head.active.is_some() {
                    self.loop_exit();
                } else if let Some(region) = self.head.saved {
                    self.head.active = Some(region);
                    if self.head.pos < region.start || self.head.pos >= region.end {
                        self.head.pos = region.start;
                        self.restart();
                    }
                }
            }
            DeckCommand::HotCue(slot) => {
                if let Some(&cue) = self.hot_cues.get(slot) {
                    if cue.is_finite() {
                        self.jump_to(cue);
                    } else {
                        self.hot_cues[slot] = self.quantized(self.head.pos);
                    }
                }
            }
            DeckCommand::SetHotCue(slot, position) => {
                if slot < HOT_CUES && position.is_finite() {
                    self.hot_cues[slot] = self.head.clamp(position);
                }
            }
            DeckCommand::ClearHotCue(slot) => {
                if let Some(cue) = self.hot_cues.get_mut(slot) {
                    *cue = f64::NAN;
                }
            }
            DeckCommand::BeatJump(beats) => {
                if let (Some(spb), true) = (self.samples_per_beat(), beats.is_finite()) {
                    let shift = beats * spb;
                    if let Some(region) = self.head.active {
                        let moved = LoopRegion::new(region.start + shift, region.end + shift);
                        self.head.active = moved;
                        self.head.saved = moved.or(self.head.saved);
                    }
                    self.head.pos = self.head.clamp(self.head.pos + shift);
                    self.restart();
                }
            }
            DeckCommand::SetStemGain(stem, gain) => {
                if stem < MAX_STEMS && gain.is_finite() {
                    self.target_gains[stem] = gain.clamp(0.0, 4.0);
                }
            }
        }
    }

    fn load(&mut self, track: Option<Arc<DeckTrack>>) {
        if let Some(old) = self.track.take() {
            self.retire(old);
        }
        self.head = super::playhead::Playhead::new();
        self.head.end = track.as_ref().map_or(0.0, |t| t.frames() as f64);
        self.track_ratio = track
            .as_ref()
            .map_or(1.0, |t| t.sample_rate().as_f64() / self.engine_rate);
        self.track = track;
        self.playing = false;
        self.at_end = false;
        self.roll = false;
        self.cue_point = 0.0;
        self.hot_cues = [f64::NAN; HOT_CUES];
        self.gains = [1.0; MAX_STEMS];
        self.target_gains = [1.0; MAX_STEMS];
        self.phase_offset = 0.0;
        self.restart();
        self.status
            .loads
            .fetch_add(1, std::sync::atomic::Ordering::Release);
    }

    fn beat_loop(&mut self, beats: f64, roll: bool) {
        let (Some(track), true) = (self.track.as_ref(), beats.is_finite()) else {
            return;
        };
        let Some(grid) = track.grid().copied() else {
            return;
        };
        let beats = beats.clamp(MIN_BEATS, 64.0);
        let length = beats * grid.samples_per_beat();
        // Resizing a plain loop keeps its start.
        if let (Some(region), false) = (self.head.active, roll || self.roll) {
            if let Some(resized) = LoopRegion::new(region.start, region.start + length) {
                self.head.active = Some(resized);
                self.head.saved = Some(resized);
                if self.head.pos >= resized.end {
                    self.head.pos = resized.start + (self.head.pos - resized.start) % length;
                    self.restart();
                }
            }
            return;
        }
        let start = if self.quantize {
            // The grid line at or before the playhead, on a grid of the
            // loop's own length (whole beats for loops of a beat or more).
            let step = beats.min(1.0);
            grid.sample_at((grid.beat_at(self.head.pos) / step).floor() * step)
        } else {
            self.head.pos
        };
        let Some(region) = LoopRegion::new(start, start + length) else {
            return;
        };
        if roll {
            if self.head.slip.is_none() {
                self.head.slip = Some(self.head.pos);
            }
            self.roll = true;
        } else {
            self.head.saved = Some(region);
        }
        self.head.active = Some(region);
        if self.head.pos >= region.end || self.head.pos < region.start {
            self.head.pos = region.start;
            self.restart();
        }
    }

    fn loop_exit(&mut self) {
        self.head.active = None;
        if let Some(slip) = self.head.slip.take() {
            self.head.pos = self.head.clamp(slip);
            self.restart();
        }
        self.roll = false;
    }
}
