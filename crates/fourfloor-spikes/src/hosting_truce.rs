//! Plugin-hosting spike on top of `truce-rack` 1.1.5 (CLAP, VST3 and AU v2).
//!
//! Everything here runs off the audio thread: scanning and loading open
//! dynamic libraries and allocate freely. [`process_silent_block`] allocates
//! its scratch buffers up front and then makes exactly one `process` call; it
//! is a smoke test, not an engine integration.

use truce_rack::core::buffer::BusRange;
use truce_rack::core::plugin::ProcessStatus;
use truce_rack::core::{
    AudioBuffer, BusLayout, EventList, Plugin, PluginInfo, PluginScanner, ProcessContext,
    TransportInfo,
};

/// Plugin formats this spike can scan through `truce-rack`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// CLAP, via `truce-rack-clap` (raw `clap-sys`, not `clack`).
    Clap,
    /// VST3, via `truce-rack-vst3` (the `vst3` crate).
    Vst3,
    /// Audio Unit v2, via `truce-rack-au`. Apple platforms only.
    Au,
}

impl Format {
    /// Every format compiled in for the current target.
    pub fn all() -> &'static [Format] {
        #[cfg(target_vendor = "apple")]
        {
            &[Format::Clap, Format::Vst3, Format::Au]
        }
        #[cfg(not(target_vendor = "apple"))]
        {
            &[Format::Clap, Format::Vst3]
        }
    }

    /// Short lowercase name, matching `PluginInfo::format`.
    pub fn name(self) -> &'static str {
        match self {
            Format::Clap => "clap",
            Format::Vst3 => "vst3",
            Format::Au => "au",
        }
    }
}

/// The outcome of scanning one format's default locations.
#[derive(Debug)]
pub struct ScanReport {
    /// Which format was scanned.
    pub format: Format,
    /// The plugins found, or the scanner's error rendered as text.
    pub plugins: Result<Vec<PluginInfo>, String>,
}

/// Scan one format's default OS locations.
pub fn scan(format: Format) -> ScanReport {
    let plugins = match format {
        Format::Clap => truce_rack::clap::ClapScanner::new().scan(),
        Format::Vst3 => truce_rack::vst3::Vst3Scanner::new().scan(),
        #[cfg(target_vendor = "apple")]
        Format::Au => truce_rack::au::AuScanner::new().scan(),
        #[cfg(not(target_vendor = "apple"))]
        Format::Au => Ok(Vec::new()),
    };
    ScanReport {
        format,
        plugins: plugins.map_err(|e| e.to_string()),
    }
}

/// Scan every compiled-in format.
pub fn scan_all() -> Vec<ScanReport> {
    Format::all().iter().map(|&f| scan(f)).collect()
}

/// What happened when a plugin processed one block of silence.
#[derive(Clone, Debug, PartialEq)]
pub struct SilentBlockReport {
    /// Input channels handed to the plugin.
    pub input_channels: usize,
    /// Output channels handed to the plugin.
    pub output_channels: usize,
    /// Frames processed.
    pub frames: usize,
    /// Parameter count the plugin reported.
    pub parameter_count: usize,
    /// Status the plugin returned from `process`.
    pub status: ProcessStatus,
    /// Largest absolute output sample.
    pub peak: f32,
    /// True when every output sample is finite.
    pub all_finite: bool,
}

/// Activate `plugin`, run one block of digital silence through it, and
/// deactivate it again.
///
/// The layout is the plugin's first supported one (truce-rack 1.1.5 only ever
/// reports stereo in / stereo out).
pub fn process_silent_block<P: Plugin<f32>>(
    plugin: &mut P,
    sample_rate: f64,
    frames: usize,
) -> Result<SilentBlockReport, String> {
    if frames == 0 {
        return Err("frames must be greater than zero".to_string());
    }
    let layout: BusLayout = plugin
        .supported_layouts()
        .first()
        .cloned()
        .ok_or_else(|| "plugin reports no supported bus layouts".to_string())?;

    let input_ranges = bus_ranges(layout.inputs.iter().map(|b| b.channels.count() as usize));
    let output_ranges = bus_ranges(layout.outputs.iter().map(|b| b.channels.count() as usize));
    let input_channels = layout.total_input_channels() as usize;
    let output_channels = layout.total_output_channels() as usize;

    let input_data = vec![vec![0.0_f32; frames]; input_channels];
    // Fill with a sentinel so "the plugin never wrote" is distinguishable
    // from "the plugin wrote silence".
    let mut output_data = vec![vec![f32::NAN; frames]; output_channels];

    plugin
        .activate(layout, sample_rate, frames)
        .map_err(|e| format!("activate failed: {e}"))?;

    let result = {
        let inputs: Vec<&[f32]> = input_data.iter().map(Vec::as_slice).collect();
        let mut outputs: Vec<&mut [f32]> = output_data.iter_mut().map(Vec::as_mut_slice).collect();
        let mut buffer =
            AudioBuffer::new(&inputs, &mut outputs, frames, &input_ranges, &output_ranges);
        let events = EventList::new();
        let mut output_events = EventList::new();
        let mut context = ProcessContext {
            sample_rate,
            max_block_size: frames,
            transport: Some(TransportInfo {
                tempo_bpm: Some(124.0),
                time_signature: Some((4, 4)),
                song_position_beats: Some(0.0),
                song_position_samples: Some(0),
                bar_start_beats: Some(0.0),
                playing: true,
                recording: false,
                loop_active: false,
            }),
            output_events: &mut output_events,
        };
        plugin.process(&mut buffer, &events, &mut context)
    };
    let parameter_count = plugin.parameter_count();
    plugin.deactivate();

    let status = result.map_err(|e| format!("process failed: {e}"))?;
    let all_finite = output_data.iter().flatten().all(|s| s.is_finite());
    let peak = output_data
        .iter()
        .flatten()
        .filter(|s| s.is_finite())
        .fold(0.0_f32, |acc, s| acc.max(s.abs()));

    Ok(SilentBlockReport {
        input_channels,
        output_channels,
        frames,
        parameter_count,
        status,
        peak,
        all_finite,
    })
}

/// Load the plugin described by `info` with the scanner for its format and
/// push one silent block through it.
pub fn load_and_process_silence(
    info: &PluginInfo,
    sample_rate: f64,
    frames: usize,
) -> Result<SilentBlockReport, String> {
    match info.format {
        "clap" => {
            let mut plugin = truce_rack::clap::ClapScanner::new()
                .load(info)
                .map_err(|e| format!("load failed: {e}"))?;
            process_silent_block(&mut plugin, sample_rate, frames)
        }
        "vst3" => {
            let mut plugin = truce_rack::vst3::Vst3Scanner::new()
                .load(info)
                .map_err(|e| format!("load failed: {e}"))?;
            process_silent_block(&mut plugin, sample_rate, frames)
        }
        #[cfg(target_vendor = "apple")]
        "au" => {
            let mut plugin = truce_rack::au::AuScanner::new()
                .load(info)
                .map_err(|e| format!("load failed: {e}"))?;
            process_silent_block(&mut plugin, sample_rate, frames)
        }
        other => Err(format!("unsupported plugin format {other:?}")),
    }
}

fn bus_ranges(channel_counts: impl Iterator<Item = usize>) -> Vec<BusRange> {
    channel_counts
        .scan(0_usize, |start, len| {
            let range = BusRange::new(*start, len);
            *start += len;
            Some(range)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use truce_rack::core::info::{ParameterInfo, PluginCategory, PresetInfo};
    use truce_rack::core::plugin::PluginCore;
    use truce_rack::core::{Error, Result};

    /// A host-side fake that copies input to output, so the harness can be
    /// tested without any installed plugin.
    struct Passthrough {
        info: PluginInfo,
        layouts: Vec<BusLayout>,
        active: Option<BusLayout>,
        write_output: bool,
    }

    impl Passthrough {
        fn new(layouts: Vec<BusLayout>, write_output: bool) -> Self {
            Self {
                info: PluginInfo {
                    name: "Passthrough".into(),
                    vendor: "fourfloor".into(),
                    version: 1,
                    category: PluginCategory::Effect,
                    path: std::path::PathBuf::new(),
                    unique_id: "test.passthrough".into(),
                    format: "test",
                    has_editor: false,
                    accepts_midi: false,
                },
                layouts,
                active: None,
                write_output,
            }
        }
    }

    impl PluginCore for Passthrough {
        fn info(&self) -> &PluginInfo {
            &self.info
        }
        fn active_layout(&self) -> Option<&BusLayout> {
            self.active.as_ref()
        }
        fn supported_layouts(&self) -> &[BusLayout] {
            &self.layouts
        }
        fn parameter_count(&self) -> usize {
            0
        }
        fn parameter_info(&self, index: usize) -> Result<ParameterInfo> {
            Err(Error::InvalidParameter(index))
        }
        fn parameter_value(&self, index: usize) -> Result<f64> {
            Err(Error::InvalidParameter(index))
        }
        fn parameter_value_string(&self, index: usize, _value: f64) -> Result<String> {
            Err(Error::InvalidParameter(index))
        }
        fn set_parameter(&mut self, index: usize, _value: f64) -> Result<()> {
            Err(Error::InvalidParameter(index))
        }
        fn preset_count(&self) -> usize {
            0
        }
        fn preset_info(&self, index: usize) -> Result<PresetInfo> {
            Err(Error::InvalidParameter(index))
        }
        fn load_preset(&mut self, _preset_number: i32) -> Result<()> {
            Ok(())
        }
        fn save_state(&self) -> Result<Vec<u8>> {
            Ok(Vec::new())
        }
        fn load_state(&mut self, _bytes: &[u8]) -> Result<()> {
            Ok(())
        }
        fn activate(&mut self, layout: BusLayout, _sr: f64, _max_block: usize) -> Result<()> {
            self.active = Some(layout);
            Ok(())
        }
        fn deactivate(&mut self) {
            self.active = None;
        }
        fn is_active(&self) -> bool {
            self.active.is_some()
        }
    }

    impl Plugin<f32> for Passthrough {
        fn process(
            &mut self,
            buffer: &mut AudioBuffer<'_, f32>,
            _events: &EventList,
            _context: &mut ProcessContext<'_>,
        ) -> Result<ProcessStatus> {
            if !self.is_active() {
                return Err(Error::NotActivated);
            }
            if !self.write_output {
                return Ok(ProcessStatus::Continue);
            }
            let frames = buffer.num_frames();
            for ch in 0..buffer.total_output_channels() {
                let input: Option<&[f32]> = buffer.main_inputs().get(ch).copied();
                let out = &mut buffer.main_outputs()[ch];
                for (i, sample) in out.iter_mut().take(frames).enumerate() {
                    *sample = input.map_or(0.0, |inp| inp[i]);
                }
            }
            Ok(ProcessStatus::Continue)
        }
    }

    #[test]
    fn silent_block_through_a_passthrough_is_silent() {
        let mut plugin = Passthrough::new(vec![BusLayout::stereo()], true);
        let report = process_silent_block(&mut plugin, 48_000.0, 256).unwrap();
        assert_eq!(report.input_channels, 2);
        assert_eq!(report.output_channels, 2);
        assert_eq!(report.frames, 256);
        assert_eq!(report.status, ProcessStatus::Continue);
        assert!(report.all_finite);
        assert_eq!(report.peak, 0.0);
        assert!(!plugin.is_active(), "harness must deactivate the plugin");
    }

    #[test]
    fn a_plugin_that_never_writes_output_is_detected() {
        let mut plugin = Passthrough::new(vec![BusLayout::stereo()], false);
        let report = process_silent_block(&mut plugin, 48_000.0, 64).unwrap();
        assert!(!report.all_finite);
    }

    #[test]
    fn missing_layout_and_zero_frames_are_errors() {
        let mut no_layout = Passthrough::new(Vec::new(), true);
        assert!(process_silent_block(&mut no_layout, 48_000.0, 64).is_err());
        let mut plugin = Passthrough::new(vec![BusLayout::stereo()], true);
        assert!(process_silent_block(&mut plugin, 48_000.0, 0).is_err());
    }

    #[test]
    fn bus_ranges_partition_the_channels() {
        let layout = BusLayout::stereo_with_sidechain("SC");
        let ranges = bus_ranges(layout.inputs.iter().map(|b| b.channels.count() as usize));
        assert_eq!(ranges.len(), 2);
        // BusRange has private fields; AudioBuffer::new debug-asserts that the
        // ranges partition the channel array, so constructing one proves it.
        let data = vec![vec![0.0_f32; 8]; 4];
        let inputs: Vec<&[f32]> = data.iter().map(Vec::as_slice).collect();
        let mut out_data = vec![vec![0.0_f32; 8]; 2];
        let mut outputs: Vec<&mut [f32]> = out_data.iter_mut().map(Vec::as_mut_slice).collect();
        let out_ranges = bus_ranges(std::iter::once(2));
        let buffer = AudioBuffer::new(&inputs, &mut outputs, 8, &ranges, &out_ranges);
        assert_eq!(buffer.bus_inputs(1).len(), 2);
    }

    #[test]
    fn unknown_format_is_rejected() {
        let info = Passthrough::new(Vec::new(), true).info;
        assert!(load_and_process_silence(&info, 48_000.0, 64).is_err());
    }

    /// Needs real plugins: scans this machine and drives Apple's stock AUDelay.
    #[test]
    #[ignore = "scans installed plugins and instantiates a system Audio Unit"]
    #[cfg(target_os = "macos")]
    fn scan_system_and_run_apple_audelay() {
        let reports = scan_all();
        assert_eq!(reports.len(), 3);
        let au = reports
            .iter()
            .find(|r| r.format == Format::Au)
            .and_then(|r| r.plugins.as_ref().ok())
            .expect("AU scan should succeed on macOS");
        let delay = au
            .iter()
            .find(|p| p.unique_id == "aufx:dely:appl")
            .expect("Apple AUDelay ships with macOS");
        let report = load_and_process_silence(delay, 48_000.0, 512).unwrap();
        assert_eq!(report.status, ProcessStatus::Continue);
        assert!(report.all_finite);
        assert!(report.peak < 1e-6, "silence in should be silence out");
    }
}
