//! The factory samples: a small set of house one-shots and loops rendered
//! by code from `fourfloor-instruments`, so the browser is never empty.
//!
//! Nothing is recorded or downloaded. Every sample is the output of
//! fourfloor's own synthesis, and is dedicated to the public domain under
//! CC0 1.0; [`write_factory`] puts a note saying so next to the files.

use std::fs;
use std::path::{Path, PathBuf};

use fourfloor_analysis::Key;
use fourfloor_audio::{write_wav_file, AudioBuffer, SampleFormat, WavSpec};
use fourfloor_core::{
    Device, Event, EventBuffer, EventKind, PrepareConfig, ProcessIo, SampleRate, Tempo,
    TransportContext,
};
use fourfloor_instruments::{
    Acid303, Clap909, Cowbell808, DrumKit, FmSynth, Hat909, Kick808, Kick909, PolyPad, Rim909,
    Snare909, Tom909,
};

use crate::error::LibraryError;
use crate::types::FileKind;

/// Tempo of every factory loop.
pub const FACTORY_BPM: f64 = 124.0;
/// Sample rate of every factory sample.
pub const FACTORY_SAMPLE_RATE: SampleRate = SampleRate::HZ_48000;
/// Length of every factory loop in bars of 4/4.
pub const FACTORY_LOOP_BARS: u32 = 4;
/// Peak level the samples are normalised to (-1 dBFS).
pub const FACTORY_PEAK: f32 = 0.891;
/// Name of the licence note written next to the samples.
pub const FACTORY_LICENSE_FILE: &str = "LICENSE.txt";

const LICENSE_TEXT: &str = "fourfloor factory samples\n\
\n\
Every audio file in this folder was synthesised by fourfloor's own\n\
instruments (fourfloor-instruments); none is a recording or a copy of\n\
another work.\n\
\n\
They are dedicated to the public domain under the Creative Commons CC0 1.0\n\
Universal dedication: https://creativecommons.org/publicdomain/zero/1.0/\n\
Use them for anything, including commercial releases, with no credit needed.\n";

const BLOCK: usize = 256;
const STEPS_PER_BAR: u32 = 16;
/// MIDI pitches in the General MIDI drum layout the kit uses.
const KICK: u8 = 36;
const RIM: u8 = 37;
const CLAP: u8 = 39;
const HAT_CLOSED: u8 = 42;
const HAT_OPEN: u8 = 46;
const A_MINOR: Key = Key::minor(9);

/// One note of a recipe: start and length in seconds.
#[derive(Clone, Copy, Debug)]
struct Note {
    start: f64,
    length: f64,
    pitch: u8,
    velocity: u8,
}

/// What is rendered for a sample.
#[derive(Clone, Copy)]
enum Recipe {
    /// Notes played once; the file is `seconds` long with a short fade.
    Hit {
        make: fn() -> Box<dyn Device>,
        pitches: &'static [u8],
        velocity: u8,
        gate: f64,
        seconds: f64,
    },
    /// A pattern of whole bars at [`FACTORY_BPM`]; the tail that rings past
    /// the end is folded back onto the start so the file loops seamlessly.
    Loop {
        make: fn() -> Box<dyn Device>,
        pattern: Pattern,
    },
}

/// A loop pattern: for a bar and a sixteenth step, the notes to play as
/// (pitch, velocity, gate in steps).
type Pattern = fn(u32, u32) -> Vec<(u8, u8, f64)>;

/// One factory sample: where it goes and what it is.
#[derive(Clone, Copy)]
pub struct FactorySample {
    /// Path below the factory folder, with `/` separators.
    pub path: &'static str,
    /// One-shot or loop.
    pub kind: FileKind,
    /// Tempo, for loops.
    pub bpm: Option<f64>,
    /// Key, for pitched material.
    pub key: Option<Key>,
    /// What the sound is: `kick`, `hat`, `bass`...
    pub tags: &'static [&'static str],
    recipe: Recipe,
}

impl std::fmt::Debug for FactorySample {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FactorySample")
            .field("path", &self.path)
            .field("kind", &self.kind)
            .field("bpm", &self.bpm)
            .field("key", &self.key)
            .field("tags", &self.tags)
            .finish_non_exhaustive()
    }
}

fn boxed<D: Device + 'static>(device: D) -> Box<dyn Device> {
    Box::new(device)
}

const fn hit(
    path: &'static str,
    tags: &'static [&'static str],
    key: Option<Key>,
    recipe: Recipe,
) -> FactorySample {
    FactorySample {
        path,
        kind: FileKind::OneShot,
        bpm: None,
        key,
        tags,
        recipe,
    }
}

const fn looped(
    path: &'static str,
    tags: &'static [&'static str],
    key: Option<Key>,
    make: fn() -> Box<dyn Device>,
    pattern: Pattern,
) -> FactorySample {
    FactorySample {
        path,
        kind: FileKind::Loop,
        bpm: Some(FACTORY_BPM),
        key,
        tags,
        recipe: Recipe::Loop { make, pattern },
    }
}

const fn drum(make: fn() -> Box<dyn Device>, pitch: &'static [u8], seconds: f64) -> Recipe {
    Recipe::Hit {
        make,
        pitches: pitch,
        velocity: 110,
        gate: 0.1,
        seconds,
    }
}

/// The factory set, in the order it is written.
pub fn factory_samples() -> Vec<FactorySample> {
    vec![
        hit(
            "One Shots/FF Kick 909.wav",
            &["kick", "drums"],
            None,
            drum(|| boxed(Kick909::new()), &[KICK], 0.6),
        ),
        hit(
            "One Shots/FF Kick 808.wav",
            &["kick", "drums"],
            None,
            drum(|| boxed(Kick808::new()), &[KICK], 1.2),
        ),
        hit(
            "One Shots/FF Snare 909.wav",
            &["snare", "drums"],
            None,
            drum(|| boxed(Snare909::new()), &[38], 0.6),
        ),
        hit(
            "One Shots/FF Clap 909.wav",
            &["clap", "drums"],
            None,
            drum(|| boxed(Clap909::new()), &[CLAP], 0.8),
        ),
        hit(
            "One Shots/FF Hat Closed 909.wav",
            &["hat", "drums"],
            None,
            drum(|| boxed(Hat909::new()), &[HAT_CLOSED], 0.3),
        ),
        hit(
            "One Shots/FF Hat Open 909.wav",
            &["hat", "drums"],
            None,
            drum(|| boxed(Hat909::new()), &[HAT_OPEN], 1.0),
        ),
        hit(
            "One Shots/FF Rim 909.wav",
            &["rim", "perc", "drums"],
            None,
            drum(|| boxed(Rim909::new()), &[RIM], 0.3),
        ),
        hit(
            "One Shots/FF Tom 909.wav",
            &["tom", "drums"],
            None,
            drum(|| boxed(Tom909::new()), &[45], 0.8),
        ),
        hit(
            "One Shots/FF Cowbell 808.wav",
            &["cowbell", "perc", "drums"],
            None,
            drum(|| boxed(Cowbell808::new()), &[56], 0.6),
        ),
        hit(
            "One Shots/FF Bass Note A1.wav",
            &["bass"],
            None,
            Recipe::Hit {
                make: || boxed(Acid303::new()),
                pitches: &[33],
                velocity: 96,
                gate: 0.4,
                seconds: 1.0,
            },
        ),
        hit(
            "One Shots/FF Chord Stab Am.wav",
            &["stab", "chord"],
            Some(A_MINOR),
            Recipe::Hit {
                make: || boxed(FmSynth::stab()),
                pitches: &[57, 60, 64],
                velocity: 104,
                gate: 0.25,
                seconds: 1.5,
            },
        ),
        hit(
            "One Shots/FF Organ Note C3.wav",
            &["organ", "keys"],
            None,
            Recipe::Hit {
                make: || boxed(FmSynth::organ()),
                pitches: &[48],
                velocity: 100,
                gate: 0.6,
                seconds: 1.2,
            },
        ),
        hit(
            "One Shots/FF Pad Chord Am.wav",
            &["pad", "chord"],
            Some(A_MINOR),
            Recipe::Hit {
                make: || boxed(PolyPad::new()),
                pitches: &[57, 60, 64, 69],
                velocity: 96,
                gate: 3.0,
                seconds: 5.0,
            },
        ),
        looped(
            "Loops/FF Drum Loop 124bpm.wav",
            &["drums", "loop"],
            None,
            || boxed(DrumKit::new()),
            drum_pattern,
        ),
        looped(
            "Loops/FF Hat Loop 124bpm.wav",
            &["hat", "drums", "loop"],
            None,
            || boxed(Hat909::new()),
            hat_pattern,
        ),
        looped(
            "Loops/FF Top Loop 124bpm.wav",
            &["top", "drums", "loop"],
            None,
            || boxed(DrumKit::new()),
            top_pattern,
        ),
        looped(
            "Loops/FF Acid Bass Loop 124bpm Am.wav",
            &["bass", "acid", "loop"],
            Some(A_MINOR),
            || boxed(Acid303::new()),
            bass_pattern,
        ),
        looped(
            "Loops/FF Chord Stab Loop 124bpm Am.wav",
            &["stab", "chord", "loop"],
            Some(A_MINOR),
            || boxed(FmSynth::stab()),
            stab_pattern,
        ),
    ]
}

/// Four on the floor: kick on every beat, clap on two and four, closed
/// hats on the off-beats and an open hat to turn each second bar round.
fn drum_pattern(bar: u32, step: u32) -> Vec<(u8, u8, f64)> {
    let mut notes = Vec::new();
    if step.is_multiple_of(4) {
        notes.push((KICK, 118, 0.5));
    }
    if step % 8 == 4 {
        notes.push((CLAP, 104, 0.5));
    }
    if step % 4 == 2 {
        let open = bar % 2 == 1 && step == 14;
        notes.push((if open { HAT_OPEN } else { HAT_CLOSED }, 96, 0.5));
    }
    notes
}

/// Sixteenth-note hats, accented on the off-beat.
fn hat_pattern(bar: u32, step: u32) -> Vec<(u8, u8, f64)> {
    let velocity = match step % 4 {
        2 => 116,
        0 => 72,
        _ => 52,
    };
    let open = bar % 2 == 1 && step == 14;
    vec![(if open { HAT_OPEN } else { HAT_CLOSED }, velocity, 0.5)]
}

/// The drum loop without its kick, plus a rim shot figure.
fn top_pattern(bar: u32, step: u32) -> Vec<(u8, u8, f64)> {
    let mut notes: Vec<(u8, u8, f64)> = drum_pattern(bar, step)
        .into_iter()
        .filter(|&(pitch, _, _)| pitch != KICK)
        .collect();
    if matches!(step, 3 | 10) || (bar % 2 == 1 && step == 13) {
        notes.push((RIM, 88, 0.5));
    }
    notes
}

/// A one-bar acid line in A minor, with the last bar turned round.
fn bass_pattern(bar: u32, step: u32) -> Vec<(u8, u8, f64)> {
    // Pitch 0 is a rest. A1 = 33.
    const LINE: [u8; 16] = [33, 0, 33, 45, 0, 33, 36, 0, 33, 0, 45, 33, 0, 40, 38, 0];
    const TURN: [u8; 16] = [33, 0, 33, 45, 0, 33, 36, 0, 33, 0, 45, 33, 40, 38, 36, 31];
    let line = if bar % 4 == 3 { &TURN } else { &LINE };
    let pitch = line[step as usize % 16];
    if pitch == 0 {
        return Vec::new();
    }
    let velocity = if step % 8 == 3 { 112 } else { 88 };
    vec![(pitch, velocity, 0.6)]
}

/// Off-beat chord stabs: A minor, A minor, D minor, E minor.
fn stab_pattern(bar: u32, step: u32) -> Vec<(u8, u8, f64)> {
    const CHORDS: [[u8; 3]; 4] = [[57, 60, 64], [57, 60, 64], [57, 62, 65], [59, 64, 67]];
    if !matches!(step, 2 | 6 | 10 | 13) {
        return Vec::new();
    }
    CHORDS[bar as usize % 4]
        .iter()
        .map(|&pitch| (pitch, 100, 0.7))
        .collect()
}

/// Plays `notes` through `device` and returns `frames` frames of stereo.
fn play(device: &mut dyn Device, notes: &[Note], frames: usize) -> [Vec<f32>; 2] {
    let rate = FACTORY_SAMPLE_RATE;
    let tempo = Tempo::new(FACTORY_BPM).unwrap_or(Tempo::DEFAULT);
    let to_frame = |seconds: f64| (seconds * rate.as_f64()).round().max(0.0) as u64;
    let mut timeline: Vec<(u64, bool, EventKind)> = notes
        .iter()
        .flat_map(|note| {
            [
                (
                    to_frame(note.start),
                    true,
                    EventKind::note_on(note.pitch, note.velocity),
                ),
                (
                    to_frame(note.start + note.length),
                    false,
                    EventKind::note_off(note.pitch),
                ),
            ]
        })
        .collect();
    // Note-offs first at equal frames, so a repeated pitch re-triggers.
    timeline.sort_by_key(|&(frame, is_on, _)| (frame, is_on));

    device.prepare(&PrepareConfig::stereo(rate, BLOCK));
    let (mut left, mut right) = (vec![0.0f32; frames], vec![0.0f32; frames]);
    let mut events: Vec<Event> = Vec::with_capacity(timeline.len().max(1));
    let mut events_out = EventBuffer::with_capacity(64);
    let mut next = 0;
    let mut start = 0;
    while start < frames {
        let n = BLOCK.min(frames - start);
        let end = (start + n) as u64;
        events.clear();
        while let Some(&(frame, _, kind)) = timeline.get(next).filter(|entry| entry.0 < end) {
            events.push(Event::new(frame.saturating_sub(start as u64) as u32, kind));
            next += 1;
        }
        events_out.clear();
        let ctx = TransportContext::playing_at(rate, tempo, start as u64);
        let mut outputs: [&mut [f32]; 2] =
            [&mut left[start..start + n], &mut right[start..start + n]];
        let mut io = ProcessIo {
            inputs: &[],
            sidechain: None,
            outputs: &mut outputs,
            events: &events,
            events_out: &mut events_out,
        };
        device.process(&ctx, &mut io);
        start += n;
    }
    [left, right]
}

/// Scales both channels so the louder peaks at [`FACTORY_PEAK`]. Silence
/// and non-finite samples are left as zeros.
fn normalise(channels: &mut [Vec<f32>; 2]) {
    for sample in channels.iter_mut().flatten() {
        if !sample.is_finite() {
            *sample = 0.0;
        }
    }
    let peak = channels
        .iter()
        .flatten()
        .fold(0.0f32, |peak, sample| peak.max(sample.abs()));
    if peak > 1e-6 {
        let gain = FACTORY_PEAK / peak;
        for sample in channels.iter_mut().flatten() {
            *sample *= gain;
        }
    }
}

/// Length of a factory loop in frames.
pub fn factory_loop_frames() -> usize {
    let beats = f64::from(FACTORY_LOOP_BARS * 4);
    (beats * 60.0 / FACTORY_BPM * FACTORY_SAMPLE_RATE.as_f64()).round() as usize
}

impl FactorySample {
    /// File name of the sample.
    pub fn filename(&self) -> &'static str {
        self.path.rsplit('/').next().unwrap_or(self.path)
    }

    /// Where the sample goes below `folder`.
    pub fn path_in(&self, folder: &Path) -> PathBuf {
        self.path
            .split('/')
            .fold(folder.to_path_buf(), |path, part| path.join(part))
    }

    /// Renders the sample: stereo, [`FACTORY_SAMPLE_RATE`], peaking at
    /// [`FACTORY_PEAK`]. The result is the same on every call.
    pub fn render(&self) -> AudioBuffer {
        let rate = FACTORY_SAMPLE_RATE.as_f64();
        let mut channels = match self.recipe {
            Recipe::Hit {
                make,
                pitches,
                velocity,
                gate,
                seconds,
            } => {
                let notes: Vec<Note> = pitches
                    .iter()
                    .map(|&pitch| Note {
                        start: 0.0,
                        length: gate,
                        pitch,
                        velocity,
                    })
                    .collect();
                let frames = (seconds * rate).round() as usize;
                let mut channels = play(make().as_mut(), &notes, frames);
                // Close the file on silence: a 10 ms fade at the end.
                let fade = ((0.010 * rate) as usize).min(frames);
                for channel in &mut channels {
                    for (index, sample) in channel[frames - fade..].iter_mut().enumerate() {
                        *sample *= 1.0 - (index + 1) as f32 / fade as f32;
                    }
                }
                channels
            }
            Recipe::Loop { make, pattern } => {
                let step_seconds = 60.0 / FACTORY_BPM / 4.0;
                let mut notes = Vec::new();
                for bar in 0..FACTORY_LOOP_BARS {
                    for step in 0..STEPS_PER_BAR {
                        let start = f64::from(bar * STEPS_PER_BAR + step) * step_seconds;
                        notes.extend(pattern(bar, step).into_iter().map(
                            |(pitch, velocity, gate)| Note {
                                start,
                                length: gate * step_seconds,
                                pitch,
                                velocity,
                            },
                        ));
                    }
                }
                let frames = factory_loop_frames();
                let tail = (2.0 * rate) as usize;
                let mut channels = play(make().as_mut(), &notes, frames + tail);
                for channel in &mut channels {
                    let rung_past: Vec<f32> = channel.split_off(frames);
                    for (sample, extra) in channel.iter_mut().zip(rung_past) {
                        *sample += extra;
                    }
                }
                channels
            }
        };
        normalise(&mut channels);
        let [left, right] = channels;
        AudioBuffer::new(FACTORY_SAMPLE_RATE, vec![left, right])
    }
}

/// What [`write_factory`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FactoryReport {
    /// Files rendered and written by this call.
    pub written: Vec<PathBuf>,
    /// Files that were already there and were left alone.
    pub kept: Vec<PathBuf>,
}

impl crate::Library {
    /// Writes the factory samples into `folder` (keeping any already
    /// there) and scans the folder, so the browser has something in it.
    pub fn install_factory(
        &mut self,
        folder: &Path,
        options: &crate::ScanOptions,
    ) -> Result<(FactoryReport, crate::ScanReport), LibraryError> {
        let written = write_factory(folder)?;
        let scanned = self.scan(&[folder.to_path_buf()], options, |_| {})?;
        Ok((written, scanned))
    }
}

/// Renders the factory samples into `folder` as 24-bit WAV files, in
/// `One Shots` and `Loops` subfolders, with a CC0 note beside them. Files
/// that already exist are left alone, so calling this at every start-up
/// is cheap and never undoes a user's edit.
pub fn write_factory(folder: &Path) -> Result<FactoryReport, LibraryError> {
    let mut report = FactoryReport::default();
    fs::create_dir_all(folder).map_err(|error| LibraryError::io(folder, error))?;
    let license = folder.join(FACTORY_LICENSE_FILE);
    if !license.exists() {
        fs::write(&license, LICENSE_TEXT).map_err(|error| LibraryError::io(&license, error))?;
    }
    let spec = WavSpec {
        sample_rate: FACTORY_SAMPLE_RATE,
        channels: 2,
        format: SampleFormat::Pcm24,
    };
    for sample in factory_samples() {
        let path = sample.path_in(folder);
        if path.exists() {
            report.kept.push(path);
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| LibraryError::io(parent, error))?;
        }
        // Write under a temporary name so an interrupted run never leaves
        // half a file that a later run would keep.
        let partial = path.with_extension("wav.part");
        write_wav_file(&partial, spec, &sample.render().interleaved())?;
        fs::rename(&partial, &path).map_err(|error| LibraryError::io(&path, error))?;
        report.written.push(path);
    }
    Ok(report)
}
