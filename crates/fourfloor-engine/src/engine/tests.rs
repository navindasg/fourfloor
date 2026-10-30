//! Tests for the engine's controller and audio-thread halves.

use super::*;
use crate::devices::{
    Fader, Gain, Metronome, Mixer, NotePlayer, PlaceholderSynth, PlayerNote, SineOsc,
};
use crate::graph::GraphBuilder;
use crate::offline::render;
use fourfloor_core::Ticks;

const RATE: SampleRate = SampleRate::HZ_48000;

fn config(channels: usize, max_block: usize) -> EngineConfig {
    EngineConfig {
        channels,
        max_block,
        ..EngineConfig::new(RATE)
    }
}

fn sine_graph(cfg: &EngineConfig, amplitude: f32) -> CompiledGraph {
    let mut b = GraphBuilder::new(cfg.channels);
    b.add_node(DeviceId(1), Box::new(SineOsc::new(12_000.0, amplitude)), 0)
        .unwrap();
    b.add_node(DeviceId(2), Box::new(Gain::new(1.0)), 1)
        .unwrap();
    b.connect(DeviceId(1), DeviceId(2), 0).unwrap();
    b.set_output(DeviceId(2)).unwrap();
    b.compile(cfg.sample_rate, cfg.max_block).unwrap()
}

fn click_graph(cfg: &EngineConfig) -> CompiledGraph {
    let mut b = GraphBuilder::new(cfg.channels);
    b.add_node(DeviceId(1), Box::new(Metronome::new(1.0)), 0)
        .unwrap();
    b.set_output(DeviceId(1)).unwrap();
    b.compile(cfg.sample_rate, cfg.max_block).unwrap()
}

fn onsets(signal: &[f32]) -> Vec<usize> {
    let mut found = Vec::new();
    let mut last_sound: Option<usize> = None;
    for (i, sample) in signal.iter().enumerate() {
        if *sample != 0.0 {
            if last_sound.is_none_or(|last| i - last > 100) {
                found.push(i);
            }
            last_sound = Some(i);
        }
    }
    found
}

#[test]
fn empty_engine_renders_silence_and_stays_at_zero() {
    let cfg = config(2, 64);
    let (ctl, mut proc) = engine(cfg, CompiledGraph::empty(2, RATE, 64).unwrap()).unwrap();
    let out = render(&mut proc, 4, 64);
    assert_eq!(out.len(), 4 * 64 * 2);
    assert!(out.iter().all(|s| *s == 0.0));
    assert_eq!(ctl.playhead_samples(), 0);
    assert!(!ctl.is_playing());
    assert_eq!(ctl.current_graph(), GraphHandle(0));
}

#[test]
fn transport_messages_drive_the_published_playhead() {
    let cfg = config(1, 128);
    let (mut ctl, mut proc) = engine(cfg, CompiledGraph::empty(1, RATE, 128).unwrap()).unwrap();
    ctl.play().unwrap();
    render(&mut proc, 10, 100);
    assert!(ctl.is_playing());
    assert_eq!(ctl.playhead_samples(), 1000);

    ctl.stop().unwrap();
    render(&mut proc, 3, 100);
    assert!(!ctl.is_playing());
    assert_eq!(ctl.playhead_samples(), 1000, "stop keeps position");

    ctl.seek(Beats(2.0)).unwrap();
    ctl.set_tempo(Tempo::new(60.0).unwrap()).unwrap();
    render(&mut proc, 1, 100);
    assert_eq!(ctl.tempo().bpm(), 60.0);
    assert_eq!(ctl.playhead_beats(), Beats(2.0));
    assert_eq!(ctl.playhead_samples(), 96_000);
}

#[test]
fn callbacks_longer_than_max_block_are_split() {
    let cfg = config(1, 32);
    let (mut ctl, mut proc) = engine(cfg, sine_graph(&cfg, 1.0)).unwrap();
    ctl.play().unwrap();
    let out = render(&mut proc, 1, 1000);
    assert_eq!(ctl.playhead_samples(), 1000);
    // 12 kHz at 48 kHz repeats 0, 1, 0, -1 with no seam at block edges.
    for (i, sample) in out.iter().enumerate() {
        let expected = [0.0, 1.0, 0.0, -1.0][i % 4];
        assert!((sample - expected).abs() < 1e-4, "sample {i} = {sample}");
    }
}

#[test]
fn output_is_interleaved_and_mapped_to_the_device_channel_count() {
    let cfg = config(2, 16);
    let (_ctl, mut proc) = engine(cfg, sine_graph(&cfg, 0.5)).unwrap();
    // Three output channels from a stereo graph: the third is silent.
    let mut out = vec![9.0f32; 4 * 3 + 2];
    proc.process_interleaved(&mut out, 3);
    let frame1 = &out[3..6];
    assert!((frame1[0] - 0.5).abs() < 1e-6);
    assert_eq!(frame1[0], frame1[1]);
    assert_eq!(frame1[2], 0.0);
    assert_eq!(out[12..], [0.0, 0.0], "partial frame is zeroed");

    // One output channel from a stereo graph: just the left channel.
    let mut mono = vec![9.0f32; 4];
    proc.process_interleaved(&mut mono, 1);
    assert!(mono.iter().all(|s| s.abs() <= 0.5 + 1e-6));
    let mut none = vec![9.0f32; 4];
    proc.process_interleaved(&mut none, 0);
    assert_eq!(none, [0.0; 4]);
}

#[test]
fn set_param_message_reaches_the_device() {
    let cfg = config(1, 64);
    let (mut ctl, mut proc) = engine(cfg, sine_graph(&cfg, 1.0)).unwrap();
    ctl.set_param(DeviceId(2), Gain::GAIN, 0.25).unwrap();
    ctl.set_param(DeviceId(404), Gain::GAIN, 9.0).unwrap(); // ignored
    let out = render(&mut proc, 1, 64);
    assert!((out[1] - 0.25).abs() < 1e-6);
}

#[test]
fn peak_meters_hold_the_maximum_until_taken() {
    let cfg = config(2, 64);
    let (mut ctl, mut proc) = engine(cfg, sine_graph(&cfg, 0.5)).unwrap();
    render(&mut proc, 2, 64);
    assert!((ctl.peak(0) - 0.5).abs() < 1e-6);
    assert!((ctl.peak(1) - 0.5).abs() < 1e-6);
    assert_eq!(ctl.peak(MAX_CHANNELS + 3), 0.0);

    ctl.set_param(DeviceId(2), Gain::GAIN, 0.1).unwrap();
    render(&mut proc, 2, 64);
    assert!((ctl.peak(0) - 0.5).abs() < 1e-6, "peak holds");
    assert!((ctl.take_peak(0) - 0.5).abs() < 1e-6);
    assert_eq!(ctl.peak(0), 0.0);
    render(&mut proc, 2, 64);
    assert!((ctl.take_peak(0) - 0.05).abs() < 1e-6);
    assert_eq!(ctl.take_peak(MAX_CHANNELS), 0.0);
}

#[test]
fn swapped_graph_takes_over_at_a_block_boundary_and_the_old_one_comes_back() {
    let cfg = config(1, 64);
    let (mut ctl, mut proc) = engine(cfg, sine_graph(&cfg, 1.0)).unwrap();
    let before = render(&mut proc, 1, 64);
    assert!((before[1] - 1.0).abs() < 1e-6);

    let handle = ctl.swap_graph(sine_graph(&cfg, 0.25)).unwrap();
    assert_eq!(ctl.current_graph(), GraphHandle(0), "not swapped yet");
    assert_eq!(ctl.collect_garbage(), 0);

    let after = render(&mut proc, 1, 64);
    assert!(after.iter().all(|s| s.abs() <= 0.25 + 1e-6));
    assert!((after[1] - 0.25).abs() < 1e-6, "new graph from frame 0");
    assert_eq!(ctl.current_graph(), handle);
    assert_eq!(ctl.collect_garbage(), 1, "old graph returned for dropping");
    assert_eq!(ctl.collect_garbage(), 0);
    assert_eq!(ctl.leaked_graphs(), 0);
}

#[test]
fn commands_queued_after_a_swap_apply_to_the_new_graph() {
    let cfg = config(1, 64);
    let (mut ctl, mut proc) = engine(cfg, CompiledGraph::empty(1, RATE, 64).unwrap()).unwrap();
    ctl.swap_graph(sine_graph(&cfg, 1.0)).unwrap();
    ctl.set_param(DeviceId(2), Gain::GAIN, 0.5).unwrap();
    let out = render(&mut proc, 1, 64);
    assert!((out[1] - 0.5).abs() < 1e-6);
}

#[test]
fn many_swaps_never_leak_or_overflow() {
    let cfg = EngineConfig {
        graph_capacity: 2,
        ..config(1, 16)
    };
    let (mut ctl, mut proc) = engine(cfg, CompiledGraph::empty(1, RATE, 16).unwrap()).unwrap();
    let mut last = GraphHandle(0);
    for round in 0..20 {
        // Fill the pending queue, then let the audio side take them all.
        ctl.swap_graph(sine_graph(&cfg, 0.1)).unwrap();
        last = ctl.swap_graph(sine_graph(&cfg, 0.2)).unwrap();
        let full = ctl.swap_graph(sine_graph(&cfg, 0.3));
        assert!(
            matches!(full, Err(EngineError::GraphQueueFull(_))),
            "round {round}"
        );
        render(&mut proc, 1, 16);
        assert_eq!(ctl.current_graph(), last);
    }
    ctl.collect_garbage();
    assert_eq!(ctl.leaked_graphs(), 0);
    assert_ne!(last, GraphHandle(0));
}

#[test]
fn mismatched_graphs_and_bad_configs_are_rejected() {
    let cfg = config(2, 64);
    let wrong_block = CompiledGraph::empty(2, RATE, 32).unwrap();
    assert!(matches!(
        engine(cfg, wrong_block),
        Err(EngineError::GraphMismatch(_))
    ));
    let (mut ctl, _proc) = engine(cfg, CompiledGraph::empty(2, RATE, 64).unwrap()).unwrap();
    let wrong_channels = CompiledGraph::empty(1, RATE, 64).unwrap();
    let wrong_rate = CompiledGraph::empty(2, SampleRate::HZ_44100, 64).unwrap();
    for graph in [wrong_channels, wrong_rate] {
        let error = ctl.swap_graph(graph).unwrap_err();
        assert!(matches!(error, EngineError::GraphMismatch(_)));
        assert!(error.to_string().contains("does not match"));
    }
    let zero = EngineConfig {
        message_capacity: 0,
        ..cfg
    };
    assert!(matches!(
        engine(zero, CompiledGraph::empty(2, RATE, 64).unwrap()),
        Err(EngineError::InvalidCapacity)
    ));
    assert_eq!(ctl.config(), cfg);
}

#[test]
fn full_command_queue_reports_an_error() {
    let cfg = EngineConfig {
        message_capacity: 2,
        ..config(1, 16)
    };
    let (mut ctl, mut proc) = engine(cfg, CompiledGraph::empty(1, RATE, 16).unwrap()).unwrap();
    ctl.play().unwrap();
    ctl.stop().unwrap();
    assert!(matches!(ctl.play(), Err(EngineError::QueueFull)));
    assert!(matches!(
        ctl.swap_graph(CompiledGraph::empty(1, RATE, 16).unwrap()),
        Err(EngineError::QueueFull)
    ));
    render(&mut proc, 1, 16);
    ctl.play().unwrap();
}

#[test]
fn click_lands_exactly_on_beats_through_the_engine() {
    // 128 BPM at 48 kHz: 22500 samples per beat. Block of 500 does not
    // line up with the engine's 64-frame max block.
    let cfg = config(2, 64);
    let (mut ctl, mut proc) = engine(cfg, click_graph(&cfg)).unwrap();
    ctl.set_tempo(Tempo::new(128.0).unwrap()).unwrap();
    ctl.play().unwrap();
    let out = render(&mut proc, 200, 500);
    let left: Vec<f32> = out.iter().step_by(2).copied().collect();
    assert_eq!(onsets(&left), [0, 22_500, 45_000, 67_500, 90_000]);
    let right: Vec<f32> = out.iter().skip(1).step_by(2).copied().collect();
    assert_eq!(left, right);
}

#[test]
fn click_stays_silent_until_play() {
    let cfg = config(1, 64);
    let (mut ctl, mut proc) = engine(cfg, click_graph(&cfg)).unwrap();
    let idle = render(&mut proc, 10, 64);
    assert!(idle.iter().all(|s| *s == 0.0));
    ctl.play().unwrap();
    let rolling = render(&mut proc, 1, 64);
    assert_eq!(rolling[0], 1.0);
}

#[test]
fn loop_region_wraps_the_playhead_and_retriggers_the_downbeat() {
    // Loop the first beat at 120 BPM: 24000 samples. The 512-frame max
    // block does not divide it, so the engine has to split a block.
    let cfg = config(1, 512);
    let (mut ctl, mut proc) = engine(cfg, click_graph(&cfg)).unwrap();
    ctl.set_loop(Some(LoopRegion::new(Beats(0.0), Beats(1.0))))
        .unwrap();
    ctl.play().unwrap();
    let out = render(&mut proc, 1, 24_000 * 3 + 100);
    assert_eq!(onsets(&out), [0, 24_000, 48_000, 72_000]);
    // Every pass replays beat 0, the accented downbeat.
    assert_eq!(out[1], out[24_001]);
    assert_eq!(out[1], out[48_001]);
    assert_eq!(ctl.playhead_samples(), 100);

    ctl.set_loop(None).unwrap();
    render(&mut proc, 1, 24_000);
    assert_eq!(ctl.playhead_samples(), 24_100, "loop cleared");
}

#[test]
fn seek_resets_devices() {
    let cfg = config(1, 64);
    let (mut ctl, mut proc) = engine(cfg, click_graph(&cfg)).unwrap();
    ctl.play().unwrap();
    render(&mut proc, 1, 64); // a click is now ringing
    ctl.stop().unwrap();
    ctl.seek(Beats(0.5)).unwrap();
    let out = render(&mut proc, 1, 64);
    assert!(out.iter().all(|s| *s == 0.0), "tail cut by the seek");
}

#[test]
fn mixer_graph_sums_two_sources() {
    let cfg = config(1, 64);
    let mut b = GraphBuilder::new(1);
    b.add_node(DeviceId(1), Box::new(SineOsc::new(12_000.0, 0.5)), 0)
        .unwrap();
    b.add_node(DeviceId(2), Box::new(SineOsc::new(12_000.0, 0.25)), 0)
        .unwrap();
    b.add_node(DeviceId(3), Box::new(Mixer::new(2)), 2).unwrap();
    b.connect(DeviceId(1), DeviceId(3), 0).unwrap();
    b.connect(DeviceId(2), DeviceId(3), 1).unwrap();
    b.set_output(DeviceId(3)).unwrap();
    let (mut ctl, mut proc) = engine(cfg, b.compile(RATE, 64).unwrap()).unwrap();
    let out = render(&mut proc, 1, 8);
    assert!((out[1] - 0.75).abs() < 1e-6);
    ctl.set_param(DeviceId(3), Mixer::port_gain(1), 0.0)
        .unwrap();
    let out = render(&mut proc, 1, 8);
    assert!((out[1] - 0.5).abs() < 1e-6);
    assert_eq!(proc.channels(), 1);
    assert_eq!(proc.graph().order().len(), 3);
    assert_eq!(proc.transport().position_samples(), 0);
    assert_eq!(proc.config(), cfg);
}

/// A note player feeding the placeholder synth through a fader.
fn note_graph(cfg: &EngineConfig, notes: &[PlayerNote], pan: f32) -> CompiledGraph {
    let mut b = GraphBuilder::new(cfg.channels);
    b.add_node(DeviceId(1), Box::new(NotePlayer::new(notes)), 0)
        .unwrap();
    b.add_node(DeviceId(2), Box::new(PlaceholderSynth::new(1.0)), 0)
        .unwrap();
    b.add_node(DeviceId(3), Box::new(Fader::new(0.0, pan)), 1)
        .unwrap();
    b.connect_events(DeviceId(1), DeviceId(2)).unwrap();
    b.connect(DeviceId(2), DeviceId(3), 0).unwrap();
    b.set_output(DeviceId(3)).unwrap();
    b.compile(cfg.sample_rate, cfg.max_block).unwrap()
}

fn split(interleaved: &[f32]) -> (Vec<f32>, Vec<f32>) {
    (
        interleaved.iter().step_by(2).copied().collect(),
        interleaved.iter().skip(1).step_by(2).copied().collect(),
    )
}

#[test]
fn notes_play_sample_accurately_across_loop_wraps() {
    // Loop the first beat at 120 BPM (24 000 samples). A hat sits at tick
    // 480 (sample 12 000). The 512-frame block does not divide the loop.
    let cfg = config(2, 512);
    let notes = [PlayerNote::new(Ticks::new(480), Ticks::new(120), 42, 127)];
    let (mut ctl, mut proc) = engine(cfg, note_graph(&cfg, &notes, 0.0)).unwrap();
    ctl.set_loop(Some(LoopRegion::new(Beats(0.0), Beats(1.0))))
        .unwrap();
    ctl.play().unwrap();
    let (left, right) = split(&render(&mut proc, 1, 24_000 * 3));
    // The 50 ms hat fades out long before the next pass.
    let found = onsets(&left);
    assert_eq!(found.len(), 3, "{found:?}");
    for (pass, onset) in found.iter().enumerate() {
        let expected = pass * 24_000 + 12_000;
        assert!(
            (expected..expected + 2).contains(onset),
            "pass {pass}: onset at {onset}"
        );
    }
    assert_eq!(left, right, "centred: both channels match");
}

#[test]
fn a_note_held_over_the_loop_end_is_released_at_the_wrap() {
    // A tone from tick 480 that would last four beats, in a one-beat loop.
    let cfg = config(2, 512);
    let notes = [PlayerNote::new(Ticks::new(480), Ticks::new(3_840), 69, 127)];
    let (mut ctl, mut proc) = engine(cfg, note_graph(&cfg, &notes, 0.0)).unwrap();
    ctl.set_loop(Some(LoopRegion::new(Beats(0.0), Beats(1.0))))
        .unwrap();
    ctl.play().unwrap();
    let (left, _) = split(&render(&mut proc, 1, 48_000));
    let peak =
        |range: std::ops::Range<usize>| left[range].iter().fold(0.0f32, |m, s| m.max(s.abs()));
    assert_eq!(peak(0..12_000), 0.0);
    assert!(peak(12_100..24_000) > 0.9, "sounding to the loop end");
    // Released on the wrap: silent after the 10 ms release, until it
    // retriggers at sample 36 000.
    assert_eq!(peak(24_480..36_000), 0.0, "no hanging note");
    assert!(peak(36_100..48_000) > 0.9, "retriggered on the second pass");
}

#[test]
fn pan_makes_the_output_true_stereo() {
    let cfg = config(2, 256);
    let notes = [PlayerNote::new(Ticks::new(0), Ticks::new(960), 69, 127)];
    let (mut ctl, mut proc) = engine(cfg, note_graph(&cfg, &notes, -1.0)).unwrap();
    ctl.play().unwrap();
    let (left, right) = split(&render(&mut proc, 20, 256));
    assert!(left.iter().any(|s| s.abs() > 0.9));
    assert!(right.iter().all(|s| s.abs() < 1e-6), "hard left");
    assert!(ctl.peak(0) > 0.9 && ctl.peak(1) < 1e-6);
}

#[test]
fn live_events_reach_a_device_while_stopped() {
    let cfg = config(2, 128);
    let (mut ctl, mut proc) = engine(cfg, note_graph(&cfg, &[], 0.0)).unwrap();
    assert!(render(&mut proc, 2, 128).iter().all(|s| *s == 0.0));
    ctl.send_event(DeviceId(2), EventKind::note_on(60, 100))
        .unwrap();
    ctl.send_event(DeviceId(404), EventKind::note_on(60, 100))
        .unwrap(); // ignored
    let out = render(&mut proc, 4, 128);
    assert!(out.iter().any(|s| s.abs() > 0.3), "the note sounds");
    ctl.send_event(DeviceId(2), EventKind::note_off(60))
        .unwrap();
    render(&mut proc, 8, 128);
    assert!(render(&mut proc, 1, 128).iter().all(|s| *s == 0.0));
}

#[test]
fn stopping_the_transport_releases_played_notes() {
    let cfg = config(2, 128);
    let notes = [PlayerNote::new(Ticks::new(0), Ticks::new(96_000), 69, 127)];
    let (mut ctl, mut proc) = engine(cfg, note_graph(&cfg, &notes, 0.0)).unwrap();
    ctl.play().unwrap();
    assert!(render(&mut proc, 8, 128).iter().any(|s| s.abs() > 0.9));
    ctl.stop().unwrap();
    render(&mut proc, 8, 128);
    assert!(render(&mut proc, 1, 128).iter().all(|s| *s == 0.0));
}

#[test]
fn halves_can_move_to_other_threads() {
    fn assert_send<T: Send>() {}
    assert_send::<AudioProcessor>();
    assert_send::<EngineController>();
}
