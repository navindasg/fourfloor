//! The notes of the demo track: pure functions from the arrangement to
//! step lanes and MIDI notes, so they can be tested without a project.

use fourfloor_core::Ticks;
use fourfloor_model::sequencer::{StepLane, BAR_TICKS};
use fourfloor_model::MidiNote;

/// Ticks per sixteenth note.
pub const STEP_TICKS: i64 = BAR_TICKS / 16;

/// One part of the arrangement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Section {
    /// Display name.
    pub name: &'static str,
    /// First bar (0-based).
    pub start_bar: u32,
    /// Length in bars.
    pub bars: u32,
}

impl Section {
    /// Timeline position of the first bar.
    pub fn start(&self) -> Ticks {
        Ticks::new(i64::from(self.start_bar) * BAR_TICKS)
    }

    /// The bar after the last one.
    pub fn end_bar(&self) -> u32 {
        self.start_bar + self.bars
    }

    /// Length in ticks.
    pub fn length(&self) -> Ticks {
        Ticks::new(i64::from(self.bars) * BAR_TICKS)
    }
}

/// Kick and offbeat closed hats only, so a DJ can mix into it.
pub const INTRO: Section = Section {
    name: "Intro",
    start_bar: 0,
    bars: 8,
};
/// No kick: the pad enters with a clap roll, sixteenth hats and the acid
/// line under a rising filter.
pub const BUILD: Section = Section {
    name: "Build",
    start_bar: 8,
    bars: 4,
};
/// Everything.
pub const DROP: Section = Section {
    name: "Drop",
    start_bar: 12,
    bars: 16,
};
/// The arrangement, in order.
pub const SECTIONS: [Section; 3] = [INTRO, BUILD, DROP];
/// Length of the arrangement in bars.
pub const DEMO_BARS: u32 = INTRO.bars + BUILD.bars + DROP.bars;

/// Chord roots, one per two bars: Am, F, C, G.
pub const ROOTS: [u8; 4] = [57, 53, 48, 55];
/// Seventh-chord intervals for each root.
pub const QUALITIES: [[u8; 4]; 4] = [[0, 3, 7, 10], [0, 4, 7, 11], [0, 4, 7, 11], [0, 4, 7, 10]];
/// Bass roots, one per two bars (A1, F1, C2, G1 an octave up: A2..G2).
pub const BASS_ROOTS: [u8; 4] = [45, 41, 36, 43];

/// General MIDI pitches the drum devices are played with.
pub mod pitch {
    /// Kick.
    pub const KICK: u8 = 36;
    /// Clap.
    pub const CLAP: u8 = 39;
    /// Closed hat.
    pub const CLOSED_HAT: u8 = 42;
    /// Open hat.
    pub const OPEN_HAT: u8 = 46;
}

/// A MIDI note.
pub fn note(start: i64, length: i64, pitch: u8, velocity: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch,
        velocity,
    }
}

/// A step lane over `bars` bars with a hit on every step `hit` accepts.
pub fn lane(
    pitch: u8,
    velocity: u8,
    length: i64,
    bars: u32,
    hit: impl Fn(usize) -> bool,
) -> StepLane {
    let steps = bars as usize * 16;
    StepLane {
        pitch,
        velocity,
        note_length: Ticks::new(length),
        steps: (0..steps).map(hit).collect(),
        velocities: Vec::new(),
    }
}

/// Four on the floor.
pub fn kick_lane(bars: u32) -> StepLane {
    lane(pitch::KICK, 120, 200, bars, |step| step % 4 == 0)
}

/// Clap on beats two and four, with a roll over the last bar when `roll`.
pub fn clap_lane(bars: u32, roll: bool) -> StepLane {
    let last_bar = (bars as usize - 1) * 16;
    lane(pitch::CLAP, 105, 200, bars, move |step| {
        step % 8 == 4 || (roll && step >= last_bar + 8 && step % 2 == 0)
    })
}

/// Closed hats: offbeats only, or every sixteenth with accented offbeats.
pub fn closed_hat_lane(bars: u32, sixteenths: bool) -> StepLane {
    let mut lane = lane(pitch::CLOSED_HAT, 70, 100, bars, move |step| {
        if sixteenths {
            step % 4 != 2
        } else {
            step % 4 == 2
        }
    });
    if sixteenths {
        lane.velocities = (0..bars as usize * 16)
            .map(|step| if step % 2 == 1 { 90 } else { 0 })
            .collect();
    }
    lane
}

/// Open hat on every offbeat.
pub fn open_hat_lane(bars: u32) -> StepLane {
    lane(pitch::OPEN_HAT, 100, 200, bars, |step| step % 4 == 2)
}

/// Offbeat organ bass: one note on the "and" of every beat, following the
/// progression.
pub fn offbeat_bass(bars: u32) -> Vec<MidiNote> {
    (0..i64::from(bars) * 4)
        .map(|beat| {
            let root = BASS_ROOTS[((beat / 8) % 4) as usize];
            note(beat * 960 + 480, 360, root, 105)
        })
        .collect()
}

/// A sixteenth-note acid line an octave above the bass, with accents and
/// octave jumps.
pub fn acid_line(bars: u32) -> Vec<MidiNote> {
    // Per-step pattern over one bar: semitones above the root, or `None`
    // for a rest. Only roots, fifths and octaves, so the line stays in A
    // minor over every chord of the progression.
    const PATTERN: [Option<u8>; 16] = [
        Some(0),
        None,
        Some(12),
        Some(0),
        None,
        Some(7),
        Some(0),
        Some(12),
        Some(0),
        None,
        Some(7),
        Some(0),
        Some(19),
        None,
        Some(0),
        Some(12),
    ];
    (0..i64::from(bars) * 16)
        .filter_map(|step| {
            let offset = PATTERN[(step % 16) as usize]?;
            let root = BASS_ROOTS[((step / 32) % 4) as usize] + 12;
            let velocity = if step % 4 == 2 { 110 } else { 80 };
            Some(note(step * STEP_TICKS, 100, root + offset, velocity))
        })
        .collect()
}

/// Sustained seventh chords, one per two bars.
pub fn pad_chords(bars: u32) -> Vec<MidiNote> {
    (0..i64::from(bars) / 2)
        .flat_map(|pair| {
            let index = (pair % 4) as usize;
            QUALITIES[index].map(|interval| {
                note(
                    pair * 2 * BAR_TICKS,
                    2 * BAR_TICKS - 120,
                    ROOTS[index] + interval,
                    80,
                )
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sections_tile_the_arrangement() {
        assert_eq!(SECTIONS[0].start_bar, 0);
        for pair in SECTIONS.windows(2) {
            assert_eq!(pair[0].end_bar(), pair[1].start_bar);
        }
        assert_eq!(SECTIONS[2].end_bar(), DEMO_BARS);
    }

    #[test]
    fn drum_lanes_hit_where_house_drums_do() {
        let kick = kick_lane(2);
        assert_eq!(kick.steps.iter().filter(|s| **s).count(), 8);
        let clap = clap_lane(1, false);
        let hits: Vec<usize> = (0..16).filter(|s| clap.steps[*s]).collect();
        assert_eq!(hits, [4, 12]);
        let open = open_hat_lane(1);
        assert!(open.steps[2] && open.steps[6] && !open.steps[0]);
    }

    #[test]
    fn the_bass_is_on_the_offbeats_and_the_acid_line_in_key() {
        let bass = offbeat_bass(2);
        assert_eq!(bass.len(), 8);
        assert!(bass.iter().all(|n| n.start.get() % 960 == 480));
        // A natural minor: A B C D E F G.
        let scale = [9u8, 11, 0, 2, 4, 5, 7];
        for n in acid_line(8).iter().chain(&bass).chain(&pad_chords(8)) {
            assert!(scale.contains(&(n.pitch % 12)), "pitch {}", n.pitch);
        }
    }
}
