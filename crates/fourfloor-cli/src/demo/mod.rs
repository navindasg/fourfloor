//! `fourfloor demo`: a full house track built entirely through the command
//! bus, the way a UI or agent edit would build it.
//!
//! 124 BPM in A minor, 28 bars: an 8-bar DJ-friendly intro (kick and
//! offbeat hats only), a 4-bar build (no kick; the pad comes in with a
//! clap roll, sixteenth hats and the acid line under a rising filter) and a
//! 16-bar drop with everything. 909 drums with
//! swing on a glued drum bus, an offbeat FM organ bass, a 303 line, a poly
//! pad ducked by the kick through a sidechain compressor, reverb and delay
//! returns fed by sends, and a "Master" bus with glue and a -1 dBTP true-peak limiter
//! that every other track feeds (the project model has no master chain of
//! its own).

pub mod parts;

use fourfloor_core::{DeviceId, DeviceRegistry, ParamId, Ticks, TrackId};
use fourfloor_effects::{Compressor, Delay, Filter, Limiter, Reverb, Saturator};
use fourfloor_instruments::{Acid303, FmSynth, PolyPad};
use fourfloor_model::command::{
    AddDevice, CreateTrack, RenameProject, SetDeviceSidechain, SetSend, SetSwing, SetTempo,
    SetTrackGain, SetTrackOutput, SetTrackPan, WriteAutomation, WriteMidiClip, WriteStepPattern,
};
use fourfloor_model::sequencer::{StepLane, BAR_TICKS};
use fourfloor_model::{AutomationPoint, CommandBus, MidiNote, Param, Project, TrackKind};

use crate::args::DemoOptions;
use crate::commands::{default_name, device_registry, save_new, NewSummary};
use crate::error::CliError;
use parts::{Section, BUILD, DROP, INTRO};

pub use parts::DEMO_BARS;

/// Swing of the demo's drums, as a percentage.
pub const DEMO_SWING: f64 = 56.0;
/// The master limiter's ceiling, in dB.
pub const LIMITER_CEILING_DB: f32 = -1.0;
/// Name of the bus that carries the master limiter.
pub const MASTER_BUS: &str = "Master";
/// Track names, in creation order.
pub const DEMO_TRACKS: [&str; 11] = [
    "Kick", "Clap", "Hats", "Drums", "Bass", "Acid", "Pad", "Reverb", "Delay", MASTER_BUS, "",
];

/// The parameters of a registered device kind, at their defaults except
/// for `values` (clamped to each parameter's range).
fn params(
    registry: &DeviceRegistry,
    kind: &str,
    values: &[(ParamId, f32)],
) -> Result<Vec<Param>, CliError> {
    let descriptor = registry
        .descriptor(kind)
        .ok_or_else(|| CliError::Export(format!("device kind \"{kind}\" is not registered")))?;
    Ok(descriptor
        .params
        .iter()
        .map(|info| Param {
            id: info.id,
            name: info.name.to_string(),
            value: values
                .iter()
                .find(|(id, _)| *id == info.id)
                .map_or(info.default, |(_, value)| info.clamp(*value)),
            min: info.min,
            max: info.max,
        })
        .collect())
}

fn missing(what: &str) -> CliError {
    CliError::Export(format!("{what} returned no id"))
}

fn point(time: i64, value: f32) -> AutomationPoint {
    AutomationPoint {
        time: Ticks::new(time),
        value,
    }
}

struct Builder<'a> {
    bus: CommandBus,
    registry: &'a DeviceRegistry,
}

impl Builder<'_> {
    fn track(&mut self, name: &str, kind: TrackKind) -> Result<TrackId, CliError> {
        self.bus
            .execute(CreateTrack {
                name: name.to_owned(),
                kind,
                index: None,
            })?
            .track()
            .ok_or_else(|| missing("create_track"))
    }

    fn device(
        &mut self,
        track: TrackId,
        kind: &str,
        values: &[(ParamId, f32)],
    ) -> Result<DeviceId, CliError> {
        let params = params(self.registry, kind, values)?;
        let name = self.registry.descriptor(kind).map(|d| d.name.clone());
        self.bus
            .execute(AddDevice {
                track,
                kind: kind.to_owned(),
                name,
                index: None,
                params,
            })?
            .device()
            .ok_or_else(|| missing("add_device"))
    }

    fn route(&mut self, track: TrackId, output: TrackId) -> Result<(), CliError> {
        self.bus.execute(SetTrackOutput {
            track,
            output: Some(output),
        })?;
        Ok(())
    }

    fn gain(&mut self, track: TrackId, gain_db: f32) -> Result<(), CliError> {
        self.bus.execute(SetTrackGain { track, gain_db })?;
        Ok(())
    }

    /// A step pattern over `section`, with the project's swing.
    fn steps(
        &mut self,
        track: TrackId,
        section: Section,
        lanes: Vec<StepLane>,
    ) -> Result<(), CliError> {
        self.bus.execute(WriteStepPattern {
            track,
            clip: None,
            name: section.name.to_owned(),
            start: section.start(),
            bars: section.bars,
            lanes,
            swing: None,
            groove: None,
            groove_amount: 1.0,
        })?;
        Ok(())
    }

    /// A MIDI clip over `section`; note starts are relative to it.
    fn clip(
        &mut self,
        track: TrackId,
        section: Section,
        notes: Vec<MidiNote>,
    ) -> Result<(), CliError> {
        self.bus.execute(WriteMidiClip {
            track,
            clip: None,
            name: section.name.to_owned(),
            start: section.start(),
            length: section.length(),
            notes,
        })?;
        Ok(())
    }

    /// A compressor on `track` keyed by `key` before its fader.
    fn duck(&mut self, track: TrackId, key: TrackId, depth: f32) -> Result<DeviceId, CliError> {
        let device = self.device(
            track,
            "fx.compressor",
            &[
                (Compressor::THRESHOLD, -18.0 - 18.0 * depth),
                (Compressor::RATIO, 2.0 + 18.0 * depth),
                (Compressor::ATTACK, 0.5),
                (Compressor::RELEASE, 160.0),
                (Compressor::KNEE, 3.0),
                (Compressor::MAKEUP, 0.0),
                (Compressor::MIX, 1.0),
                (Compressor::EXTERNAL_KEY, 1.0),
            ],
        )?;
        self.bus.execute(SetDeviceSidechain {
            device,
            source: Some(key),
            pre_fader: true,
        })?;
        Ok(device)
    }

    fn send(&mut self, track: TrackId, target: TrackId, gain_db: f32) -> Result<(), CliError> {
        self.bus.execute(SetSend {
            track,
            target,
            gain_db,
            pre_fader: false,
        })?;
        Ok(())
    }
}

/// The drum tracks, routed to a glued bus. Returns (kick, clap, hats, bus).
fn drums(b: &mut Builder<'_>) -> Result<[TrackId; 4], CliError> {
    use parts::{clap_lane, closed_hat_lane, kick_lane, open_hat_lane};
    let kick = b.track("Kick", TrackKind::Midi)?;
    b.device(kick, "drum.kick909", &[])?;
    b.steps(kick, INTRO, vec![kick_lane(INTRO.bars)])?;
    b.steps(kick, DROP, vec![kick_lane(DROP.bars)])?;
    b.gain(kick, -6.0)?;

    let clap = b.track("Clap", TrackKind::Midi)?;
    b.device(clap, "drum.clap909", &[])?;
    b.steps(clap, BUILD, vec![clap_lane(BUILD.bars, true)])?;
    b.steps(clap, DROP, vec![clap_lane(DROP.bars, false)])?;
    b.gain(clap, -4.0)?;

    let hats = b.track("Hats", TrackKind::Midi)?;
    b.device(hats, "drum.hat909", &[])?;
    b.steps(hats, INTRO, vec![closed_hat_lane(INTRO.bars, false)])?;
    b.steps(hats, BUILD, vec![closed_hat_lane(BUILD.bars, true)])?;
    b.steps(
        hats,
        DROP,
        vec![closed_hat_lane(DROP.bars, true), open_hat_lane(DROP.bars)],
    )?;
    b.bus.execute(SetTrackPan {
        track: hats,
        pan: 0.2,
    })?;
    b.gain(hats, -8.0)?;

    let bus = b.track("Drums", TrackKind::Bus)?;
    b.device(bus, "fx.glue", &[])?;
    for track in [kick, clap, hats] {
        b.route(track, bus)?;
    }
    Ok([kick, clap, hats, bus])
}

/// Bass, acid and pad. Returns (bass, acid, pad, acid filter).
fn music(b: &mut Builder<'_>, kick: TrackId) -> Result<([TrackId; 3], DeviceId), CliError> {
    let bass = b.track("Bass", TrackKind::Midi)?;
    b.device(bass, "fm.organ", &[(FmSynth::RELEASE, 0.08)])?;
    b.duck(bass, kick, 0.4)?;
    b.clip(bass, DROP, parts::offbeat_bass(DROP.bars))?;
    b.gain(bass, -6.0)?;

    let acid = b.track("Acid", TrackKind::Midi)?;
    b.device(
        acid,
        "synth.acid303",
        &[
            (Acid303::CUTOFF, 700.0),
            (Acid303::RESONANCE, 70.0),
            (Acid303::ENV_MOD, 60.0),
            (Acid303::DECAY, 250.0),
        ],
    )?;
    b.device(acid, "fx.saturator", &[(Saturator::DRIVE, 6.0)])?;
    let filter = b.device(
        acid,
        "fx.filter",
        &[(Filter::CUTOFF, 2500.0), (Filter::RESONANCE, 0.35)],
    )?;
    let acid_section = Section {
        name: "Acid line",
        start_bar: BUILD.start_bar,
        bars: BUILD.bars + DROP.bars,
    };
    b.clip(acid, acid_section, parts::acid_line(acid_section.bars))?;
    b.bus.execute(SetTrackPan {
        track: acid,
        pan: -0.15,
    })?;
    b.gain(acid, -14.0)?;

    let pad = b.track("Pad", TrackKind::Midi)?;
    b.device(
        pad,
        "poly.pad",
        &[
            (PolyPad::ATTACK, 0.4),
            (PolyPad::RELEASE, 0.8),
            (PolyPad::CUTOFF, 1800.0),
        ],
    )?;
    b.duck(pad, kick, 0.8)?;
    let chords = Section {
        name: "Pad chords",
        start_bar: BUILD.start_bar,
        bars: BUILD.bars + DROP.bars,
    };
    b.clip(pad, chords, parts::pad_chords(chords.bars))?;
    b.gain(pad, -12.0)?;
    Ok(([bass, acid, pad], filter))
}

/// The filter riser: the acid filter opens over the build, drops back on
/// the downbeat of the drop and opens slowly through it.
fn riser(b: &mut Builder<'_>, filter: DeviceId) -> Result<(), CliError> {
    let bar = |n: u32| i64::from(n) * BAR_TICKS;
    b.bus.execute(WriteAutomation {
        device: filter,
        param: Filter::CUTOFF,
        points: vec![
            point(bar(BUILD.start_bar), 250.0),
            point(bar(BUILD.end_bar()) - 1, 9000.0),
            point(bar(DROP.start_bar), 1500.0),
            point(bar(DROP.end_bar()), 4000.0),
        ],
    })?;
    Ok(())
}

/// Builds the demo project. Every edit is a command.
pub fn build_demo_project(options: &DemoOptions) -> Result<Project, CliError> {
    let registry = device_registry()?;
    let name = options
        .name
        .clone()
        .unwrap_or_else(|| default_name(&options.dir));
    let mut b = Builder {
        bus: CommandBus::new(Project::new("Untitled")),
        registry: &registry,
    };
    b.bus.execute(RenameProject { name })?;
    b.bus.execute(SetTempo { bpm: options.bpm })?;
    b.bus.execute(SetSwing {
        percent: DEMO_SWING,
        grid: None,
    })?;

    let [kick, clap, _hats, drum_bus] = drums(&mut b)?;
    let ([bass, acid, pad], filter) = music(&mut b, kick)?;
    riser(&mut b, filter)?;

    let reverb = b.track("Reverb", TrackKind::Bus)?;
    b.device(
        reverb,
        "fx.reverb",
        &[(Reverb::MIX, 1.0), (Reverb::DECAY, 2.5)],
    )?;
    let delay = b.track("Delay", TrackKind::Bus)?;
    b.device(
        delay,
        "fx.delay",
        &[
            (Delay::MIX, 1.0),
            (Delay::BEATS, 0.75),
            (Delay::FEEDBACK, 0.35),
        ],
    )?;
    b.send(clap, reverb, -10.0)?;
    b.send(pad, reverb, -4.0)?;
    b.send(acid, reverb, -12.0)?;
    b.send(acid, delay, -8.0)?;

    let master = b.track(MASTER_BUS, TrackKind::Bus)?;
    b.device(master, "fx.glue", &[])?;
    b.device(
        master,
        "fx.limiter",
        &[
            (Limiter::INPUT_GAIN, 3.0),
            (Limiter::CEILING, LIMITER_CEILING_DB),
            (Limiter::TRUE_PEAK, 1.0),
        ],
    )?;
    for track in [drum_bus, bass, acid, pad, reverb, delay] {
        b.route(track, master)?;
    }
    Ok(b.bus.project().clone())
}

/// `fourfloor demo`: creates a project folder holding the demo project.
pub fn demo_project(options: &DemoOptions) -> Result<NewSummary, CliError> {
    let project = build_demo_project(options)?;
    save_new(&options.dir, &project, DEMO_BARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_compile::{compile_project, CompileOptions};
    use fourfloor_core::SampleRate;

    #[test]
    fn the_demo_is_a_valid_project_that_compiles_without_warnings() {
        let project = build_demo_project(&DemoOptions {
            dir: "demo".into(),
            name: None,
            bpm: 124.0,
        })
        .unwrap();
        assert_eq!(project.validate(), Ok(()));
        let names: Vec<&str> = project.tracks.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, DEMO_TRACKS[..10]);
        let sends: usize = project.tracks.iter().map(|t| t.sends.len()).sum();
        assert_eq!(sends, 4);
        let registry = device_registry().unwrap();
        let compiled = compile_project(
            &project,
            &registry,
            &CompileOptions::new(SampleRate::HZ_48000, 512),
        )
        .unwrap();
        assert!(compiled.warnings.is_empty(), "{:?}", compiled.warnings);
        assert_eq!(compiled.sounding_tracks, 6);
        assert!(compiled.graph.has_control());
    }
}
