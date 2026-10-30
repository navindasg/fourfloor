//! Test material: sounds rendered by fourfloor's own instruments with a
//! known tempo, key and instrument, written as WAV files.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use fourfloor_analysis::Key;
use fourfloor_core::{Device, SampleRate};
use fourfloor_instruments::{Acid303, Clap909, DrumKit, Hat909, Kick808, Kick909, PolyPad};
use fourfloor_library::{FileKind, Library, ScanOptions, ScanReport};
use fourfloor_testkit::{notes, render_instrument, RenderConfig, StereoBuffer};

pub const RATE: SampleRate = SampleRate::HZ_48000;

/// What a test file is known to be.
#[derive(Clone, Debug)]
pub struct Expected {
    pub name: &'static str,
    pub kind: FileKind,
    pub bpm: Option<f64>,
    pub key: Option<Key>,
    /// Instrument the file really is.
    pub instrument: &'static str,
}

pub fn render(device: &mut dyn Device, specs: &[(f64, f64, u8, u8)], seconds: f64) -> StereoBuffer {
    render_instrument(
        device,
        &notes(specs, RATE),
        seconds,
        &RenderConfig::default(),
    )
}

pub fn write(dir: &Path, name: &str, buffer: &StereoBuffer) -> PathBuf {
    let path = dir.join(name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create folder");
    }
    buffer.write_wav(&path).expect("write wav");
    path
}

/// Kick on every beat, clap on 2 and 4, closed hats on the off-beats.
pub fn drum_pattern(bpm: f64, bars: u32) -> Vec<(f64, f64, u8, u8)> {
    let beat = 60.0 / bpm;
    let mut out = Vec::new();
    for index in 0..bars * 4 {
        let start = f64::from(index) * beat;
        out.push((start, 0.05, 36, 120));
        if index % 2 == 1 {
            out.push((start, 0.05, 39, 100));
        }
        out.push((start + beat / 2.0, 0.05, 42, 90));
    }
    out
}

/// Four held minor triads: i, iv, v, i of F# minor.
fn f_sharp_minor_progression() -> Vec<(f64, f64, u8, u8)> {
    let chords: [[u8; 3]; 4] = [[54, 57, 61], [59, 62, 66], [61, 64, 68], [54, 57, 61]];
    chords
        .iter()
        .enumerate()
        .flat_map(|(index, chord)| {
            let start = index as f64 * 2.0;
            chord.iter().map(move |&pitch| (start, 1.9, pitch, 100))
        })
        .collect()
}

/// An acid line in A minor (A, C, E, G around A1).
fn acid_line(bpm: f64, bars: u32) -> Vec<(f64, f64, u8, u8)> {
    const LINE: [u8; 8] = [33, 45, 36, 33, 40, 33, 43, 45];
    let step = 60.0 / bpm / 2.0;
    (0..bars * 8)
        .map(|index| {
            let pitch = LINE[index as usize % LINE.len()];
            (f64::from(index) * step, step * 0.6, pitch, 90)
        })
        .collect()
}

pub fn kick_909(velocity: u8) -> StereoBuffer {
    render(&mut Kick909::new(), &[(0.0, 0.1, 36, velocity)], 0.6)
}

pub fn kick_808() -> StereoBuffer {
    render(&mut Kick808::new(), &[(0.0, 0.1, 36, 120)], 0.9)
}

pub fn hat(pitch: u8, seconds: f64) -> StereoBuffer {
    render(&mut Hat909::new(), &[(0.0, 0.05, pitch, 110)], seconds)
}

pub fn clap() -> StereoBuffer {
    render(&mut Clap909::new(), &[(0.0, 0.1, 39, 110)], 0.8)
}

pub fn drum_loop(bpm: f64, bars: u32) -> StereoBuffer {
    let seconds = f64::from(bars * 4) * 60.0 / bpm;
    render(&mut DrumKit::new(), &drum_pattern(bpm, bars), seconds)
}

pub fn pad_progression() -> StereoBuffer {
    render(&mut PolyPad::new(), &f_sharp_minor_progression(), 8.0)
}

pub fn acid_loop(bpm: f64, bars: u32) -> StereoBuffer {
    let seconds = f64::from(bars * 4) * 60.0 / bpm;
    render(&mut Acid303::new(), &acid_line(bpm, bars), seconds)
}

/// Writes the standard test set into `dir` and says what each file is.
/// Several names say nothing, so tags must come from the features.
pub fn write_material(dir: &Path) -> Vec<Expected> {
    let set: Vec<(Expected, StereoBuffer)> = vec![
        (
            expected(
                "drums/sample_001.wav",
                FileKind::OneShot,
                None,
                None,
                "kick",
            ),
            kick_909(120),
        ),
        (
            expected(
                "drums/sample_002.wav",
                FileKind::OneShot,
                None,
                None,
                "kick",
            ),
            kick_808(),
        ),
        (
            expected("drums/Kick Soft.wav", FileKind::OneShot, None, None, "kick"),
            kick_909(70),
        ),
        (
            expected("drums/sample_003.wav", FileKind::OneShot, None, None, "hat"),
            hat(42, 0.3),
        ),
        (
            expected("drums/Hat Open.wav", FileKind::OneShot, None, None, "hat"),
            hat(46, 1.0),
        ),
        (
            expected("drums/Clap.wav", FileKind::OneShot, None, None, "clap"),
            clap(),
        ),
        (
            expected(
                "loops/groove_a.wav",
                FileKind::Loop,
                Some(128.0),
                None,
                "drums",
            ),
            drum_loop(128.0, 4),
        ),
        (
            expected(
                "loops/Bass Line 122bpm Am.wav",
                FileKind::Loop,
                Some(122.0),
                Some(Key::minor(9)),
                "bass",
            ),
            acid_loop(122.0, 4),
        ),
        (
            expected(
                "texture.wav",
                FileKind::OneShot,
                None,
                Some(Key::minor(6)),
                "pad",
            ),
            pad_progression(),
        ),
    ];
    set.into_iter()
        .map(|(expected, buffer)| {
            write(dir, expected.name, &buffer);
            expected
        })
        .collect()
}

fn expected(
    name: &'static str,
    kind: FileKind,
    bpm: Option<f64>,
    key: Option<Key>,
    instrument: &'static str,
) -> Expected {
    Expected {
        name,
        kind,
        bpm,
        key,
        instrument,
    }
}

/// A fresh in-memory library with `dir` scanned into it.
pub fn scanned(dir: &Path) -> (Library, ScanReport) {
    let mut library = Library::open_in_memory().expect("open library");
    let report = library
        .scan(&[dir.to_path_buf()], &ScanOptions::new(), |_| {})
        .expect("scan");
    (library, report)
}

/// The canonical form of `dir/name`, as the library stores it.
pub fn stored(dir: &Path, name: &str) -> PathBuf {
    dir.canonicalize().expect("canonical dir").join(name)
}
