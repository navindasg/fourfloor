//! The built-in demo, verified by ear-substitute: it is rendered offline
//! through the same compiler and engine the app plays it with, and its
//! sound is measured (onsets, spectrum, ducking, loudness, peaks).

use fourfloor_app::project::{build_demo_project, DEMO_BARS, DEMO_BPM, DEMO_TRACKS};
use fourfloor_app::services::build_registry;
use fourfloor_core::{DeviceRegistry, SampleRate};
use fourfloor_model::command::{SetDeviceBypass, SetTrackMute, SetTrackSolo};
use fourfloor_model::{CommandBus, Project};
use fourfloor_testkit::{
    assert_buffer_ok, lufs, null_residual_db, onset_times_with, rms_window_seconds, spectrum,
    spectrum_band_energy, true_peak_dbtp, OnsetOptions, RenderOptions, StereoBuffer,
};

const RATE: SampleRate = SampleRate::HZ_48000;

fn setup() -> (DeviceRegistry, Project) {
    let registry = build_registry(false).expect("registry");
    let project = build_demo_project(&registry).expect("demo builds");
    (registry, project)
}

fn render(project: &Project, registry: &DeviceRegistry) -> StereoBuffer {
    fourfloor_testkit::render_project_with(
        project,
        registry,
        &RenderOptions::new(RATE).with_tail(1.0),
    )
    .expect("renders")
}

fn id(project: &Project, name: &str) -> fourfloor_core::TrackId {
    project
        .tracks
        .iter()
        .find(|t| t.name == name)
        .map(|t| t.id)
        .expect(name)
}

fn soloed(project: &Project, names: &[&str]) -> Project {
    let mut bus = CommandBus::new(project.clone());
    for name in names {
        bus.execute(SetTrackSolo {
            track: id(project, name),
            solo: true,
        })
        .expect("solo");
    }
    bus.project().clone()
}

fn beat_seconds() -> f64 {
    60.0 / DEMO_BPM
}

#[test]
fn the_demo_uses_real_instruments_effects_and_routing() {
    let (registry, project) = setup();
    let names: Vec<&str> = project.tracks.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(names, DEMO_TRACKS);
    let kinds: Vec<&str> = project
        .tracks
        .iter()
        .flat_map(|t| t.devices.iter().map(|d| d.kind.as_str()))
        .collect();
    for kind in [
        "drum.kick909",
        "drum.clap909",
        "drum.hat909",
        "synth.acid303",
        "fm.stab",
        "poly.pad",
        "fx.compressor",
        "fx.glue",
        "fx.reverb",
        "fx.delay",
        "fx.limiter",
        "fx.filter",
        "fx.saturator",
    ] {
        assert!(kinds.contains(&kind), "{kind}");
        assert!(registry.contains(kind));
    }
    let ducked = project
        .tracks
        .iter()
        .flat_map(|t| &t.devices)
        .filter(|d| d.sidechain.is_some())
        .count();
    assert_eq!(ducked, 2, "bass and pad are keyed by the kick");
    assert_eq!(project.scenes.len(), 2);
    assert_eq!(project.macros.len(), 1);
    assert_eq!(project.modulators.len(), 1);
    project.validate().expect("valid");
}

#[test]
fn the_mix_is_full_length_loud_enough_and_never_clips() {
    let (registry, project) = setup();
    let mix = render(&project, &registry);
    let expected = f64::from(DEMO_BARS) * 4.0 * beat_seconds() + 1.0;
    assert!(
        (mix.seconds() - expected).abs() < 0.05,
        "{} s, expected {expected}",
        mix.seconds()
    );
    assert_buffer_ok(&mix);
    let loudness = lufs(&mix);
    assert!((-20.0..-8.0).contains(&loudness), "{loudness} LUFS");
    let peak = true_peak_dbtp(&mix);
    assert!(
        peak < 0.0,
        "the mix-bus limiter holds the peak below full scale: {peak} dBTP"
    );
}

#[test]
fn the_kick_lands_on_every_beat_and_is_low() {
    let (registry, project) = setup();
    let kick = render(&soloed(&project, &["Kick"]), &registry);
    let mono = kick.mono();
    let options = OnsetOptions {
        threshold_db: -18.0,
        min_gap_ms: 150.0,
        ..OnsetOptions::default()
    };
    let onsets = onset_times_with(&mono, RATE, &options);
    let beats = DEMO_BARS as usize * 4;
    assert_eq!(onsets.len(), beats, "{onsets:?}");
    // The mix-bus limiter's 5 ms lookahead delays everything equally
    // (delay compensation), so hits sit 240 samples after the beat.
    let latency = 240.0 / f64::from(RATE.hz());
    for (index, onset) in onsets.iter().enumerate() {
        let expected = index as f64 * beat_seconds() + latency;
        assert!(
            (onset - expected).abs() < 0.003,
            "beat {index}: {onset} vs {expected}"
        );
    }
    let hit = kick.slice_seconds(0.0, 0.15).mono();
    let low = spectrum_band_energy(&hit, RATE, 30.0, 150.0);
    let high = spectrum_band_energy(&hit, RATE, 1000.0, 10_000.0);
    assert!(low > 10.0 * high, "kick energy is low: {low} vs {high}");
}

#[test]
fn the_pad_ducks_after_each_kick_and_stops_ducking_when_bypassed() {
    let (registry, project) = setup();
    let pad_only = soloed(&project, &["Pad"]);
    let ratio = |project: &Project| -> f64 {
        let audio = render(project, &registry).mono();
        let beat = beat_seconds();
        let (mut ducked, mut open) = (0.0, 0.0);
        // Bars 2 to 7, past the pad's attack.
        for b in 8..28 {
            let at = b as f64 * beat;
            ducked += f64::from(rms_window_seconds(&audio, RATE, at + 0.01, 0.04));
            open += f64::from(rms_window_seconds(&audio, RATE, at + 0.33, 0.08));
        }
        ducked / open
    };
    let with_duck = ratio(&pad_only);
    assert!(
        with_duck < 0.6,
        "pad dips right after the kick: {with_duck}"
    );
    let pad = pad_only.track(id(&pad_only, "Pad")).expect("pad").clone();
    let compressor = pad
        .devices
        .iter()
        .find(|d| d.kind == "fx.compressor")
        .expect("ducker")
        .id;
    let mut bus = CommandBus::new(pad_only.clone());
    bus.execute(SetDeviceBypass {
        device: compressor,
        bypassed: true,
    })
    .expect("bypass");
    let without = ratio(bus.project());
    assert!(without > 0.8, "no dip with the ducker bypassed: {without}");
}

#[test]
fn the_bass_sits_low_and_the_stab_filter_opens_over_the_loop() {
    let (registry, project) = setup();
    let bass = render(&soloed(&project, &["Bass"]), &registry).mono();
    let low = spectrum_band_energy(&bass, RATE, 30.0, 300.0);
    let mid = spectrum_band_energy(&bass, RATE, 1000.0, 5000.0);
    assert!(low > mid, "bass energy below 300 Hz: {low} vs {mid}");
    let stabs = render(&soloed(&project, &["Stabs"]), &registry);
    let bar = 4.0 * beat_seconds();
    // Share of energy above 2 kHz, bar 1 against bar 7 (cutoff about
    // 1.2 kHz against 5.2 kHz on the automation ramp).
    let bright = |from: f64| {
        let part = stabs.slice_seconds(from, from + bar).mono();
        let s = spectrum(&part, RATE);
        s.band_power(2000.0, 12_000.0) / s.total_power()
    };
    let (early, late) = (bright(0.0), bright(6.0 * bar));
    assert!(
        late > early * 4.0,
        "the automated cutoff brightens the stabs: {early} -> {late}"
    );
}

#[test]
fn the_reverb_return_adds_sound_and_muting_it_takes_it_away() {
    let (registry, project) = setup();
    let full = render(&project, &registry);
    let mut bus = CommandBus::new(project.clone());
    bus.execute(SetTrackMute {
        track: id(&project, "Reverb"),
        mute: true,
    })
    .expect("mute");
    let dry = render(bus.project(), &registry);
    let residual = null_residual_db(&full.mono(), &dry.mono());
    assert!(
        residual > -40.0,
        "the return is audible: residual {residual} dB"
    );
    assert!(
        residual < 0.0,
        "but it is a return, not the whole mix: {residual} dB"
    );
}
