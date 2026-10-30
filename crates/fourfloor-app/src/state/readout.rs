//! What views can read about the running engine each frame: transport,
//! meters, master analysis, launcher state and loaded assets. The app fills
//! it from the services; views only read it.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use fourfloor_analysis::WaveformOverview;
use fourfloor_core::{Tempo, TrackId};

use crate::services::engine_host::LaunchState;
use crate::services::{MasterReadout, TransportReadout};

/// Peak meters with hold and fall-off, smoothed on the UI clock.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MeterSmoother {
    levels: HashMap<TrackId, [f32; 2]>,
    master: [f32; 2],
}

/// How fast meters fall, in dB per second.
pub const METER_FALL_DB_PER_SECOND: f32 = 24.0;

fn fall(shown: f32, new: f32, dt: f32) -> f32 {
    let factor = 10f32.powf(-METER_FALL_DB_PER_SECOND * dt.max(0.0) / 20.0);
    new.max(shown * factor)
}

impl MeterSmoother {
    /// Folds in new peaks for `track` after `dt` seconds.
    pub fn update_track(&mut self, track: TrackId, peak: [f32; 2], dt: f32) {
        let shown = self.levels.entry(track).or_insert([0.0; 2]);
        for (shown, new) in shown.iter_mut().zip(peak) {
            *shown = fall(*shown, new, dt);
        }
    }

    /// Folds in new master peaks.
    pub fn update_master(&mut self, peak: [f32; 2], dt: f32) {
        for (shown, new) in self.master.iter_mut().zip(peak) {
            *shown = fall(*shown, new, dt);
        }
    }

    /// The shown level of `track`.
    pub fn track(&self, track: TrackId) -> [f32; 2] {
        self.levels.get(&track).copied().unwrap_or([0.0; 2])
    }

    /// The shown master level.
    pub fn master(&self) -> [f32; 2] {
        self.master
    }
}

/// Decoded assets shared by views (waveforms of pool files).
#[derive(Clone, Debug, Default)]
pub struct Assets {
    /// Waveform overviews by pool file name.
    pub waveforms: HashMap<String, Arc<WaveformOverview>>,
    /// Pool files whose overview has been asked for.
    pub requested: HashSet<String>,
}

/// Engine and service state for one frame.
#[derive(Clone, Debug)]
pub struct LiveReadout {
    /// Transport.
    pub transport: TransportReadout,
    /// Loop region, if looping.
    pub loop_region: Option<(i64, i64)>,
    /// Meters.
    pub meters: MeterSmoother,
    /// Master loudness, correlation and spectrum.
    pub master: MasterReadout,
    /// Audio callback load, 0 to 1 (and beyond when overloaded).
    pub cpu_load: f32,
    /// Dropouts counted.
    pub xruns: u32,
    /// Backend errors counted.
    pub stream_errors: u32,
    /// Launcher state per track.
    pub launches: HashMap<TrackId, LaunchState>,
    /// Running jobs: label and progress.
    pub jobs: Vec<(String, f32)>,
    /// Where audio goes.
    pub output_name: String,
    /// Engine sample rate in Hz.
    pub sample_rate: u32,
    /// Compiler warnings for the running graph.
    pub warnings: Vec<String>,
    /// A compile failure, if the last compile failed.
    pub compile_error: Option<String>,
    /// Next undo step's label.
    pub undo_label: Option<String>,
    /// Next redo step's label.
    pub redo_label: Option<String>,
    /// Unsaved changes.
    pub dirty: bool,
    /// Steps that can be undone.
    pub undo_depth: usize,
    /// The project folder, once saved.
    pub project_dir: Option<std::path::PathBuf>,
}

impl Default for LiveReadout {
    fn default() -> Self {
        Self {
            transport: TransportReadout {
                playing: false,
                position: 0.0,
                tempo: Tempo::DEFAULT,
            },
            loop_region: None,
            meters: MeterSmoother::default(),
            master: MasterReadout::default(),
            cpu_load: 0.0,
            xruns: 0,
            stream_errors: 0,
            launches: HashMap::new(),
            jobs: Vec::new(),
            output_name: String::new(),
            sample_rate: 48_000,
            warnings: Vec::new(),
            compile_error: None,
            undo_label: None,
            redo_label: None,
            dirty: false,
            undo_depth: 0,
            project_dir: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meters_jump_up_and_fall_at_the_set_rate() {
        let mut meters = MeterSmoother::default();
        let track = TrackId(1);
        meters.update_track(track, [1.0, 0.5], 0.0);
        assert_eq!(meters.track(track), [1.0, 0.5]);
        meters.update_track(track, [0.0, 0.0], 0.5);
        let db = 20.0 * meters.track(track)[0].log10();
        assert!((db + 12.0).abs() < 0.01, "24 dB/s for half a second: {db}");
        meters.update_track(track, [0.9, 0.0], 0.1);
        assert_eq!(meters.track(track)[0], 0.9);
        meters.update_master([0.3, 0.2], 0.0);
        assert_eq!(meters.master(), [0.3, 0.2]);
        assert_eq!(meters.track(TrackId(99)), [0.0, 0.0]);
    }
}
