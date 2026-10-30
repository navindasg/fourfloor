//! What each subcommand does. Each function returns a summary value; the
//! caller decides how to print it.

use std::path::{Path, PathBuf};

use fourfloor_audio::{decode_file, measure, Loudness, WavSpec, WavWriter};
use fourfloor_compile::{
    normalize_loudness, project_length, render_stems, CompileOptions, Normalized, RenderedProject,
    SampleFormat, CHANNELS,
};
use fourfloor_core::{DeviceRegistry, SampleRate, Ticks};
use fourfloor_model::command::{AddDevice, RenameProject, SetSwing, SetTempo, SetTrackGain};
use fourfloor_model::sequencer::{write_house_pattern, HousePattern};
use fourfloor_model::storage::PROJECT_FILE;
use fourfloor_model::{load_project, save_project, CommandBus, Project};

use crate::args::{NewOptions, PlayOptions, RenderOptions, StemsOptions};
use crate::error::CliError;

/// Frames per engine callback when rendering offline.
const RENDER_BLOCK: usize = 512;

/// The devices a project can use: the engine's built-ins, everything the
/// instrument and effect crates register, and the hosted plugins in the
/// cached plugin scan (`fourfloor plugins scan` writes it).
pub fn device_registry() -> Result<DeviceRegistry, CliError> {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry)?;
    fourfloor_instruments::register(&mut registry)?;
    fourfloor_effects::register(&mut registry)?;
    fourfloor_host::register(&mut registry)?;
    Ok(registry)
}

/// What `new` created.
#[derive(Clone, Debug, PartialEq)]
pub struct NewSummary {
    /// The project's name.
    pub name: String,
    /// Its tempo in BPM.
    pub bpm: f64,
    /// Pattern length in bars.
    pub bars: u32,
    /// Tracks in the project.
    pub tracks: usize,
    /// Notes written across all clips.
    pub notes: usize,
}

/// What `render` wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderSummary {
    /// Frames written.
    pub frames: usize,
    /// Sample rate of the file.
    pub sample_rate: SampleRate,
    /// Channels in the file.
    pub channels: usize,
    /// Highest absolute sample value, linear.
    pub peak: f32,
    /// Tracks that produced sound.
    pub sounding_tracks: usize,
    /// What loudness normalisation did, if it was asked for.
    pub normalized: Option<Normalized>,
    /// Things in the project the compiler ignored.
    pub warnings: Vec<String>,
}

/// What `stems` wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct StemsSummary {
    /// The files written, master first.
    pub files: Vec<PathBuf>,
    /// Frames in each file.
    pub frames: usize,
    /// Sample rate of the files.
    pub sample_rate: SampleRate,
    /// What loudness normalisation did, if it was asked for.
    pub normalized: Option<Normalized>,
    /// Things in the project the compiler ignored.
    pub warnings: Vec<String>,
}

/// What `play` did.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaySummary {
    /// Sample rate the device ran at.
    pub sample_rate: SampleRate,
    /// Channels the device stream was opened with.
    pub device_channels: usize,
    /// Seconds of audio played.
    pub seconds: f64,
    /// Highest absolute output sample seen by the engine, linear.
    pub peak: f32,
    /// Stream errors (xruns, device changes) the backend reported.
    pub stream_errors: u32,
    /// Things in the project the compiler ignored.
    pub warnings: Vec<String>,
}

/// What `meter` measured.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MeterSummary {
    /// Channels in the file.
    pub channels: usize,
    /// Sample rate of the file.
    pub sample_rate: SampleRate,
    /// Frames in the file.
    pub frames: usize,
    /// The measurement.
    pub loudness: Loudness,
}

pub(crate) fn default_name(dir: &Path) -> String {
    dir.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| "Untitled".to_owned())
}

/// Builds the project `new` saves: a kick and hat pattern played by the
/// 909 kick and hat devices. Every edit goes through the command bus,
/// exactly as a UI or agent edit would.
pub fn build_house_project(options: &NewOptions) -> Result<Project, CliError> {
    let name = options
        .name
        .clone()
        .unwrap_or_else(|| default_name(&options.dir));
    let mut bus = CommandBus::new(Project::new("Untitled"));
    bus.execute(RenameProject { name })?;
    bus.execute(SetTempo { bpm: options.bpm })?;
    if let Some(percent) = options.swing {
        bus.execute(SetSwing {
            percent,
            grid: None,
        })?;
    }
    let written = write_house_pattern(
        &mut bus,
        &HousePattern {
            bars: options.bars,
            ..HousePattern::default()
        },
    )?;
    for (track, kind, name) in [
        (written.kick_track, "drum.kick909", "Kick 909"),
        (written.hat_track, "drum.hat909", "Hat 909"),
    ] {
        bus.execute(AddDevice {
            track,
            kind: kind.to_owned(),
            name: Some(name.to_owned()),
            index: None,
            params: Vec::new(),
        })?;
    }
    bus.execute(SetTrackGain {
        track: written.hat_track,
        gain_db: -6.0,
    })?;
    Ok(bus.project().clone())
}

/// Saves a freshly built project into `dir`, refusing to overwrite one,
/// and describes it. `bars` is the length to report.
pub(crate) fn save_new(dir: &Path, project: &Project, bars: u32) -> Result<NewSummary, CliError> {
    if dir.join(PROJECT_FILE).exists() {
        return Err(CliError::ProjectExists(dir.to_path_buf()));
    }
    save_project(dir, project)?;
    Ok(NewSummary {
        name: project.name.clone(),
        bpm: project.tempo.bpm(),
        bars,
        tracks: project.tracks.len(),
        notes: project
            .tracks
            .iter()
            .flat_map(|track| track.clips.iter())
            .map(|clip| clip.notes().len())
            .sum(),
    })
}

/// `fourfloor new`: creates a project folder holding a house pattern.
pub fn new_project(options: &NewOptions) -> Result<NewSummary, CliError> {
    if options.dir.join(PROJECT_FILE).exists() {
        return Err(CliError::ProjectExists(options.dir.clone()));
    }
    let project = build_house_project(options)?;
    save_new(&options.dir, &project, options.bars)
}

/// Seconds the project lasts at its tempo.
#[cfg(any(feature = "playback", test))]
fn length_seconds(length: Ticks, tempo: fourfloor_core::Tempo) -> f64 {
    length.to_beats().get() * tempo.seconds_per_beat()
}

/// Refuses a project with no clips: there is nothing to render or play.
fn require_clips(dir: &Path, project: &Project) -> Result<(), CliError> {
    if project_length(project) <= Ticks::ZERO {
        return Err(CliError::EmptyProject(dir.to_path_buf()));
    }
    Ok(())
}

/// Renders `project` offline in stereo, from the start of the timeline to
/// `tail_seconds` after the last clip. `dir` is the project folder: device
/// state is loaded from it, and it names the project in error messages.
pub fn render_project(
    dir: &Path,
    project: &Project,
    sample_rate: SampleRate,
    tail_seconds: f64,
) -> Result<RenderedProject, CliError> {
    require_clips(dir, project)?;
    Ok(fourfloor_compile::render_project(
        project,
        &device_registry()?,
        &export_options(dir, sample_rate, tail_seconds),
    )?)
}

/// True-peak ceiling for `--normalize`, in dBTP.
pub const NORMALIZE_CEILING_DBTP: f64 = -1.0;

fn export_options(
    dir: &Path,
    sample_rate: SampleRate,
    tail_seconds: f64,
) -> fourfloor_compile::RenderOptions {
    fourfloor_compile::RenderOptions {
        compile: CompileOptions::new(sample_rate, RENDER_BLOCK).with_project_dir(dir),
        ..fourfloor_compile::RenderOptions::new(sample_rate).with_tail(tail_seconds)
    }
}

fn sample_rate(rate_hz: u32) -> Result<SampleRate, CliError> {
    SampleRate::new(rate_hz).map_err(|_| CliError::Usage("--rate must not be zero".to_owned()))
}

/// `fourfloor stems`: renders the master and one stem per track or bus
/// that feeds it, as 32-bit float WAV files in a directory.
pub fn stems(options: &StemsOptions) -> Result<StemsSummary, CliError> {
    let sample_rate = sample_rate(options.rate_hz)?;
    let project = load_project(&options.dir)?;
    require_clips(&options.dir, &project)?;
    let mut rendered = render_stems(
        &project,
        &device_registry()?,
        &export_options(&options.dir, sample_rate, options.tail_seconds),
    )?;
    let normalized = options
        .normalize_lufs
        .map(|target| rendered.normalize(target, NORMALIZE_CEILING_DBTP))
        .transpose()?;
    Ok(StemsSummary {
        files: rendered.write_wav(&options.output, SampleFormat::Float32)?,
        frames: rendered.master.len() / CHANNELS,
        sample_rate,
        normalized,
        warnings: rendered.warnings,
    })
}

/// `fourfloor render`: renders the project to a 32-bit float WAV file.
pub fn render(options: &RenderOptions) -> Result<RenderSummary, CliError> {
    let sample_rate = sample_rate(options.rate_hz)?;
    let project = load_project(&options.dir)?;
    let RenderedProject {
        mut samples,
        sounding_tracks,
        warnings,
        ..
    } = render_project(&options.dir, &project, sample_rate, options.tail_seconds)?;
    let normalized = options
        .normalize_lufs
        .map(|target| {
            normalize_loudness(
                &mut samples,
                CHANNELS,
                sample_rate,
                target,
                NORMALIZE_CEILING_DBTP,
            )
        })
        .transpose()?;

    // The writer streams to disk and switches to RF64 past 4 GiB, so the
    // header is right however long the render is.
    let encode = |source| CliError::Encode {
        path: options.output.clone(),
        source,
    };
    let mut writer =
        WavWriter::create(&options.output, WavSpec::float_stereo(sample_rate)).map_err(encode)?;
    writer.write_interleaved(&samples).map_err(encode)?;
    writer.finalize().map_err(encode)?;

    Ok(RenderSummary {
        frames: samples.len() / CHANNELS,
        sample_rate,
        channels: CHANNELS,
        peak: samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs())),
        sounding_tracks,
        normalized,
        warnings,
    })
}

/// `fourfloor play`: plays the project through the default output device.
#[cfg(feature = "playback")]
pub fn play(options: &PlayOptions) -> Result<PlaySummary, CliError> {
    use std::time::{Duration, Instant};

    use fourfloor_compile::compile_project;
    use fourfloor_core::Beats;
    use fourfloor_engine::backend::cpal_out::{default_output_format, start_default_output};
    use fourfloor_engine::devices::Gain;
    use fourfloor_engine::{engine, EngineConfig, LoopRegion};

    /// Time after the last pass for tails to ring out.
    const TAIL: Duration = Duration::from_millis(500);
    const POLL: Duration = Duration::from_millis(20);

    let playback = |error: &dyn std::fmt::Display| CliError::Playback(error.to_string());

    let project = load_project(&options.dir)?;
    // The engine does not resample: it has to run at the device's rate.
    let format = default_output_format().map_err(|e| playback(&e))?;
    let config = EngineConfig {
        tempo: project.tempo,
        channels: CHANNELS,
        ..EngineConfig::new(format.sample_rate)
    };
    require_clips(&options.dir, &project)?;
    let compile =
        CompileOptions::new(config.sample_rate, config.max_block).with_project_dir(&options.dir);
    let compiled = compile_project(&project, &device_registry()?, &compile)?;
    let (mut controller, processor) = engine(config, compiled.graph)?;

    controller.set_param(compiled.master, Gain::GAIN, options.volume)?;
    if options.loops > 1 {
        controller.set_loop(Some(LoopRegion::new(
            Beats::ZERO,
            compiled.length.to_beats(),
        )))?;
    }
    controller.play()?;

    let pass = length_seconds(compiled.length, project.tempo);
    let total = Duration::from_secs_f64(pass * f64::from(options.loops)) + TAIL;
    let stream = start_default_output(processor).map_err(|e| playback(&e))?;
    let started = Instant::now();
    while started.elapsed() < total {
        std::thread::sleep(POLL.min(total.saturating_sub(started.elapsed())));
    }
    controller.stop()?;
    let seconds = started.elapsed().as_secs_f64();
    let peak = (0..CHANNELS)
        .map(|channel| controller.take_peak(channel))
        .fold(0.0f32, f32::max);
    let summary = PlaySummary {
        sample_rate: format.sample_rate,
        device_channels: stream.channels(),
        seconds,
        peak,
        stream_errors: stream.error_count(),
        warnings: compiled.warnings,
    };
    drop(stream);
    Ok(summary)
}

/// `fourfloor play` in a build without the `playback` feature.
#[cfg(not(feature = "playback"))]
pub fn play(_options: &PlayOptions) -> Result<PlaySummary, CliError> {
    Err(CliError::Playback(
        "this build has no audio output; rebuild with the `playback` feature".to_owned(),
    ))
}

/// `fourfloor meter`: decodes a WAV file and measures its loudness.
pub fn meter(path: &Path) -> Result<MeterSummary, CliError> {
    let audio = decode_file(path).map_err(|source| CliError::Decode {
        path: path.to_path_buf(),
        source,
    })?;
    Ok(MeterSummary {
        channels: audio.channel_count(),
        sample_rate: audio.sample_rate,
        frames: audio.frames(),
        loudness: measure(&audio.planes(), audio.sample_rate)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::{DEFAULT_BARS, DEFAULT_BPM};
    use std::path::PathBuf;

    fn options(dir: &Path) -> NewOptions {
        NewOptions {
            dir: dir.to_path_buf(),
            name: None,
            bpm: DEFAULT_BPM,
            bars: DEFAULT_BARS,
            swing: None,
        }
    }

    #[test]
    fn new_writes_a_loadable_four_bar_project_named_after_its_folder() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("friday-night");
        let summary = new_project(&options(&dir)).unwrap();
        assert_eq!(
            summary,
            NewSummary {
                name: "friday-night".to_owned(),
                bpm: 124.0,
                bars: 4,
                tracks: 2,
                notes: 64,
            }
        );
        let project = load_project(&dir).unwrap();
        assert_eq!(project.name, "friday-night");
        assert_eq!(project.tracks.len(), 2);
        assert!(dir.join("audio").is_dir());
    }

    #[test]
    fn new_refuses_to_overwrite_a_project() {
        let root = tempfile::tempdir().unwrap();
        new_project(&options(root.path())).unwrap();
        let error = new_project(&options(root.path())).unwrap_err();
        assert!(matches!(error, CliError::ProjectExists(_)), "{error}");
    }

    #[test]
    fn new_reports_model_validation_errors() {
        let root = tempfile::tempdir().unwrap();
        for bad in [
            NewOptions {
                bpm: 5.0,
                ..options(root.path())
            },
            NewOptions {
                bars: 0,
                ..options(root.path())
            },
            NewOptions {
                swing: Some(90.0),
                ..options(root.path())
            },
            NewOptions {
                name: Some(String::new()),
                ..options(root.path())
            },
        ] {
            let error = new_project(&bad).unwrap_err();
            assert!(matches!(error, CliError::Model(_)), "{error}");
        }
        assert!(!root.path().join(PROJECT_FILE).exists());
    }

    #[test]
    fn swing_option_reaches_the_written_notes() {
        let project = build_house_project(&NewOptions {
            swing: Some(60.0),
            ..options(Path::new("x"))
        })
        .unwrap();
        assert_eq!(project.groove.swing_percent, 60.0);
        // 60% on the sixteenth grid puts the first off-sixteenth at tick 288.
        let hats = project.tracks[1].clips[0].notes();
        assert!(hats.iter().any(|note| note.start.get() == 288));
    }

    #[test]
    fn render_length_is_the_project_plus_the_tail() {
        let project = build_house_project(&options(Path::new("x"))).unwrap();
        let rate = SampleRate::HZ_48000;
        let rendered = render_project(Path::new("x"), &project, rate, 0.25).unwrap();
        let samples = &rendered.samples;
        let expected = Ticks::from_whole_beats(16).to_sample_pos(project.tempo, rate) + 12_000;
        assert_eq!(samples.len() as u64, expected * CHANNELS as u64);
        assert_eq!(rendered.sounding_tracks, 2);
        assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        // Nothing in a new project is panned: both channels match.
        assert!(samples.chunks_exact(2).all(|frame| frame[0] == frame[1]));
        assert_eq!((rendered.channels, rendered.sample_rate), (2, rate));
    }

    #[test]
    fn rendering_twice_gives_identical_audio() {
        let project = build_house_project(&options(Path::new("x"))).unwrap();
        let rate = SampleRate::HZ_44100;
        let first = render_project(Path::new("x"), &project, rate, 0.1).unwrap();
        let second = render_project(Path::new("x"), &project, rate, 0.1).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn an_empty_project_is_refused() {
        let error = render_project(
            Path::new("x"),
            &Project::new("Empty"),
            SampleRate::HZ_48000,
            0.5,
        )
        .unwrap_err();
        assert!(matches!(error, CliError::EmptyProject(_)), "{error}");
    }

    #[test]
    fn render_and_meter_report_missing_files() {
        let root = tempfile::tempdir().unwrap();
        let error = render(&RenderOptions {
            dir: root.path().join("nope"),
            output: root.path().join("out.wav"),
            rate_hz: 48_000,
            tail_seconds: 0.5,
            normalize_lufs: None,
        })
        .unwrap_err();
        assert!(matches!(error, CliError::Storage(_)), "{error}");

        let error = meter(&root.path().join("nope.wav")).unwrap_err();
        assert!(matches!(error, CliError::Decode { .. }), "{error}");

        let garbage: PathBuf = root.path().join("garbage.wav");
        std::fs::write(&garbage, b"not a wav file").unwrap();
        let error = meter(&garbage).unwrap_err();
        assert!(matches!(error, CliError::Decode { .. }), "{error}");
    }

    #[test]
    fn the_registry_holds_the_engine_builtins() {
        let registry = device_registry().unwrap();
        assert!(registry.contains(fourfloor_compile::DEFAULT_INSTRUMENT));
        assert!(registry.contains("builtin.gain"));
    }

    #[test]
    fn render_writes_a_stereo_float_wav_that_reads_back() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("p");
        new_project(&options(&dir)).unwrap();
        let output = root.path().join("out.wav");
        let summary = render(&RenderOptions {
            dir,
            output: output.clone(),
            rate_hz: 44_100,
            tail_seconds: 0.0,
            normalize_lufs: None,
        })
        .unwrap();
        assert_eq!(summary.normalized, None);
        assert_eq!((summary.channels, summary.sounding_tracks), (2, 2));
        assert!(summary.peak > 0.2 && summary.peak <= 1.0);
        let audio = decode_file(&output).unwrap();
        assert_eq!(audio.frames(), summary.frames);
        assert_eq!(audio.channel_count(), 2);
        let metered = meter(&output).unwrap();
        assert!(metered.loudness.integrated_lufs.is_finite());
        assert!(metered.loudness.true_peak_dbtp <= 0.0);
    }

    #[test]
    fn length_in_seconds_follows_the_tempo() {
        let tempo = fourfloor_core::Tempo::new(120.0).unwrap();
        assert_eq!(length_seconds(Ticks::from_whole_beats(16), tempo), 8.0);
    }
}
