//! Tests every registered effect has to pass, whatever it does: the
//! registry entries, silence, DC, extreme and moving parameters, broken
//! input, odd bus widths, block sizes, sample rates and the allocation
//! guard.

mod common;

use assert_no_alloc::assert_no_alloc;
use common::*;
use fourfloor_core::{
    Device, DeviceCategory, DeviceDescriptor, DeviceInit, DeviceRegistry, EventBuffer, ParamId,
    PrepareConfig, ProcessIo, SampleRate, Tempo, TransportContext,
};
use fourfloor_effects::{Limiter, Utility, KINDS};
use fourfloor_testkit::{
    dc, is_silent, mix, null_residual_db, peak, render_effect, render_effect_with_sidechain,
    rms_db, silence, sine, white_noise, AllocDisabler, RenderConfig, StereoBuffer,
};

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

/// No setting of any effect may push full-scale input past this (120 dB).
/// The loudest honest case is the EQ with every band and the output at
/// their maximum.
const SANE_PEAK: f32 = 1.0e6;

fn registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    fourfloor_effects::register(&mut registry).expect("effects register");
    registry
}

fn create(registry: &DeviceRegistry, kind: &str) -> Box<dyn Device> {
    registry
        .create_default(kind)
        .unwrap_or_else(|error| panic!("{kind}: {error}"))
}

fn descriptor<'a>(registry: &'a DeviceRegistry, kind: &str) -> &'a DeviceDescriptor {
    registry
        .descriptor(kind)
        .unwrap_or_else(|| panic!("{kind} is not registered"))
}

/// A small deterministic generator for parameter values.
struct Lcg(u32);

impl Lcg {
    fn next_unit(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
}

fn assert_finite_and_sane(buffer: &StereoBuffer, what: &str) {
    for (name, channel) in [("left", &buffer.left), ("right", &buffer.right)] {
        if let Some(index) = channel.iter().position(|sample| !sample.is_finite()) {
            panic!("{what}: {name} is not finite at frame {index}");
        }
        let level = peak(channel);
        assert!(level < SANE_PEAK, "{what}: {name} peaks at {level}");
    }
}

/// Different noise on each side, full scale, followed by silence.
fn loud_then_quiet() -> StereoBuffer {
    let mut left = white_noise(11, 1.0, 0.5, RATE);
    let mut right = white_noise(23, 1.0, 0.5, RATE);
    left.extend(silence(frames(0.5)));
    right.extend(silence(frames(0.5)));
    StereoBuffer::new(RATE, left, right)
}

/// A quieter programme with a tone and noise, different on each side.
fn programme(seconds: f64) -> StereoBuffer {
    let left = mix(&[
        &sine(220.0, 0.2, seconds, RATE),
        &white_noise(5, 0.1, seconds, RATE),
    ]);
    let right = mix(&[
        &sine(331.0, 0.2, seconds, RATE),
        &white_noise(6, 0.1, seconds, RATE),
    ]);
    StereoBuffer::new(RATE, left, right)
}

#[test]
fn registry_lists_every_effect_with_a_usable_description() {
    let registry = registry();
    let kinds: Vec<&str> = registry.kinds().collect();
    assert_eq!(kinds, KINDS, "registered kinds");

    for kind in KINDS {
        let descriptor = descriptor(&registry, kind);
        assert_eq!(descriptor.category, DeviceCategory::Effect, "{kind}");
        assert!(!descriptor.name.is_empty(), "{kind} has a name");
        assert!(!descriptor.params.is_empty(), "{kind} has parameters");
        let keyed = kind == "fx.compressor" || kind == "fx.glue";
        assert_eq!(descriptor.sidechain, keyed, "{kind} sidechain flag");

        let device = create(&registry, kind);
        assert_eq!(device.params().len(), descriptor.params.len(), "{kind}");
        for (index, info) in descriptor.params.iter().enumerate() {
            assert_eq!(info.id, ParamId(index as u32), "{kind}: ids are positions");
            assert!(info.min < info.max, "{kind} {}: empty range", info.name);
            assert!(
                info.contains(info.default),
                "{kind} {}: default outside its range",
                info.name
            );
            assert_eq!(
                descriptor.param_by_name(&info.name).map(|found| found.id),
                Some(info.id),
                "{kind} {}: names are unique",
                info.name
            );
        }
    }
}

#[test]
fn registry_applies_the_parameters_it_is_given() {
    let registry = registry();
    let input = dual_mono(&sine(1_000.0, 0.25, 0.5, RATE));

    let mut plain = create(&registry, "fx.utility");
    let params = [(Utility::GAIN, -6.0)];
    let mut quieter = registry
        .create("fx.utility", &DeviceInit::new().with_params(&params))
        .expect("utility builds");

    let before = render_effect(&mut *plain, &input, &config());
    let after = render_effect(&mut *quieter, &input, &config());
    let change = rms_db(&after.left) - rms_db(&before.left);
    assert!((change + 6.0).abs() < 0.05, "gain changed by {change} dB");
}

#[test]
fn only_the_limiter_reports_latency() {
    let registry = registry();
    for kind in KINDS {
        let mut device = create(&registry, kind);
        device.prepare(&PrepareConfig::stereo(RATE, 512));
        let latency = device.latency_samples();
        if kind == "fx.limiter" {
            assert_eq!(latency, 240, "5 ms at 48 kHz");
        } else {
            assert_eq!(latency, 0, "{kind}");
        }
    }
}

#[test]
fn silence_in_gives_silence_out() {
    let registry = registry();
    let input = StereoBuffer::silent(RATE, frames(2.0));
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let output = render_effect(&mut *device, &input, &config());
        assert_finite_and_sane(&output, kind);
        assert!(is_silent(&output.left, -140.0), "{kind}: left makes sound");
        assert!(
            is_silent(&output.right, -140.0),
            "{kind}: right makes sound"
        );
    }
}

#[test]
fn nothing_connected_gives_silence_too() {
    let registry = registry();
    for kind in KINDS {
        let mut device = create(&registry, kind);
        prepare(&mut *device, 256);
        let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, 0);
        let out = run_block(&mut *device, &ctx, &[], 2, 256);
        for channel in &out {
            assert!(
                channel.iter().all(|sample| sample.abs() < 1.0e-9),
                "{kind}: output without an input"
            );
        }
    }
}

#[test]
fn dc_input_stays_finite_and_bounded() {
    let registry = registry();
    let input = dual_mono(&dc(0.9, frames(2.0)));
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let output = render_effect(&mut *device, &input, &config());
        assert_finite_and_sane(&output, kind);
        // At its defaults no effect should more than double a signal.
        let level = peak(&output.left).max(peak(&output.right));
        assert!(level < 2.0, "{kind}: DC came out at {level}");
    }
}

/// Renders full-scale noise, then silence, with the parameters at
/// `position(index)` of their range.
fn render_at(
    registry: &DeviceRegistry,
    kind: &str,
    what: &str,
    mut position: impl FnMut(usize) -> f32,
) {
    let mut device = create(registry, kind);
    for (index, info) in descriptor(registry, kind).params.iter().enumerate() {
        device.set_param(info.id, info.from_normalized(position(index)));
    }
    let input = loud_then_quiet();
    let key = dual_mono(&white_noise(31, 1.0, 1.0, RATE));
    let what = format!("{kind}, {what}");
    for bpm in [20.0, 999.0] {
        let output = render_effect_with_sidechain(&mut *device, &input, &key, &at_bpm(bpm));
        assert_finite_and_sane(&output, &what);
    }
    let output = render_effect(&mut *device, &input, &config().stopped());
    assert_finite_and_sane(&output, &what);
}

#[test]
fn extreme_parameters_never_blow_up() {
    let registry = registry();
    for kind in KINDS {
        render_at(&registry, kind, "everything at minimum", |_| 0.0);
        render_at(&registry, kind, "everything at maximum", |_| 1.0);
        render_at(&registry, kind, "alternating", |index| (index % 2) as f32);
        render_at(&registry, kind, "alternating the other way", |index| {
            ((index + 1) % 2) as f32
        });
        let mut random = Lcg(0x5eed);
        for round in 0..24 {
            // Corners: every parameter at one end or the other.
            render_at(&registry, kind, &format!("corner {round}"), |_| {
                random.next_unit().round()
            });
        }
    }
}

#[test]
fn out_of_range_and_broken_parameter_values_are_refused() {
    let registry = registry();
    let input = programme(0.25);
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let count = descriptor(&registry, kind).params.len() as u32;
        for id in 0..count + 3 {
            for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0e30, -1.0e30] {
                device.set_param(ParamId(id), value);
            }
        }
        device.set_param(ParamId(u32::MAX), 1.0);
        let output = render_effect(&mut *device, &input, &config());
        assert_finite_and_sane(&output, kind);
    }
}

#[test]
fn broken_input_does_not_poison_the_state() {
    let registry = registry();
    let clean = sine(440.0, 0.25, 1.0, RATE);
    let mut dirty = clean.clone();
    for (index, sample) in dirty.iter_mut().enumerate().take(4_800) {
        *sample = match index % 3 {
            0 => f32::NAN,
            1 => f32::INFINITY,
            _ => f32::NEG_INFINITY,
        };
    }
    let input = dual_mono(&dirty);
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let output = render_effect(&mut *device, &input, &config());
        assert_finite_and_sane(&output, kind);
    }
}

#[test]
fn mono_and_wide_buses_are_handled() {
    let registry = registry();
    let block = 128;
    let tone = sine(500.0, 0.25, 0.1, RATE);
    let input = &tone[..block];
    let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, 0);
    for kind in KINDS {
        // One channel in and out.
        let mut device = create(&registry, kind);
        device.prepare(&PrepareConfig::new(RATE, block, 1));
        let out = run_block(&mut *device, &ctx, &[input], 1, block);
        assert!(out[0].iter().all(|s| s.is_finite()), "{kind}: mono");

        // Four channels: the first two carry the effect, the rest silence.
        let mut device = create(&registry, kind);
        device.prepare(&PrepareConfig::new(RATE, block, 4));
        let inputs = [input, input, input, input];
        let out = run_block(&mut *device, &ctx, &inputs, 4, block);
        for (index, channel) in out.iter().enumerate() {
            assert!(
                channel.iter().all(|s| s.is_finite()),
                "{kind}: channel {index} of four"
            );
        }
        assert!(out[2].iter().chain(&out[3]).all(|s| *s == 0.0), "{kind}");

        // No output channels at all.
        let out = run_block(&mut *device, &ctx, &inputs, 0, block);
        assert!(out.is_empty());
    }
}

#[test]
fn processing_before_prepare_does_not_panic() {
    let registry = registry();
    let tone = sine(500.0, 0.25, 0.1, RATE);
    let input = &tone[..64];
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let out = run_block(&mut *device, &stopped(), &[input, input], 2, 64);
        for channel in &out {
            assert!(channel.iter().all(|s| s.is_finite()), "{kind}");
        }
    }
}

#[test]
fn parameters_can_move_every_block_without_allocating() {
    let registry = registry();
    let block = 64;
    let blocks = 400;
    let left = white_noise(41, 0.5, 1.0, RATE);
    let right = white_noise(43, 0.5, 1.0, RATE);
    let key = white_noise(47, 0.5, 1.0, RATE);
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let infos = descriptor(&registry, kind).params.clone();
        device.prepare(&PrepareConfig::stereo(RATE, block));
        let mut random = Lcg(0xabcd);
        let mut events_out = EventBuffer::with_capacity(16);
        let (mut out_left, mut out_right) = (vec![0.0f32; block], vec![0.0f32; block]);
        for index in 0..blocks {
            let start = index * block;
            let range = start..start + block;
            let inputs: [&[f32]; 2] = [&left[range.clone()], &right[range.clone()]];
            let sidechain: [&[f32]; 2] = [&key[range.clone()], &key[range]];
            // Now and then the transport stops, jumps or changes tempo.
            let tempo = Tempo::new(60.0 + 100.0 * f64::from(random.next_unit())).expect("tempo");
            let ctx = match index % 7 {
                0 => TransportContext::stopped(RATE, tempo),
                1 => TransportContext::playing_at(RATE, tempo, 0),
                _ => TransportContext::playing_at(RATE, tempo, start as u64),
            };
            out_left.fill(f32::NAN);
            out_right.fill(f32::NAN);
            {
                let mut outputs: [&mut [f32]; 2] = [&mut out_left, &mut out_right];
                let mut io = ProcessIo {
                    inputs: &inputs,
                    sidechain: (index % 2 == 0).then_some(&sidechain[..]),
                    outputs: &mut outputs,
                    events: &[],
                    events_out: &mut events_out,
                };
                assert_no_alloc(|| {
                    for info in &infos {
                        let value = info.from_normalized(random.next_unit());
                        device.set_param(info.id, value);
                    }
                    if index % 97 == 96 {
                        device.reset();
                    }
                    device.process(&ctx, &mut io);
                });
            }
            for (name, channel) in [("left", &out_left), ("right", &out_right)] {
                let worst = channel.iter().fold(0.0f32, |worst, s| {
                    if s.is_finite() {
                        worst.max(s.abs())
                    } else {
                        f32::INFINITY
                    }
                });
                assert!(
                    worst < SANE_PEAK,
                    "{kind}: {name} reached {worst} in block {index}"
                );
            }
        }
    }
}

#[test]
fn the_block_size_does_not_change_the_result() {
    let registry = registry();
    let input = programme(1.0);
    let key = dual_mono(&white_noise(9, 0.5, 1.0, RATE));
    for kind in KINDS {
        let render = |block: usize| {
            let mut device = create(&registry, kind);
            // Turn the modulation on where there is some.
            if kind == "fx.filter" {
                device.set_param(fourfloor_effects::Filter::LFO_DEPTH, 2.0);
            }
            let config = at_bpm(126.0).with_block(block);
            render_effect_with_sidechain(&mut *device, &input, &key, &config)
        };
        let reference = render(512);
        for block in [1, 17, 64, 4_096] {
            let other = render(block);
            let residual = null_residual_db(&reference.left, &other.left)
                .max(null_residual_db(&reference.right, &other.right));
            assert!(
                residual < -100.0,
                "{kind}: block {block} differs from block 512 by {residual} dB"
            );
        }
    }
}

#[test]
fn a_second_render_is_bit_identical() {
    let registry = registry();
    let input = programme(0.5);
    for kind in KINDS {
        let mut device = create(&registry, kind);
        let first = render_effect(&mut *device, &input, &config());
        // `render_effect` prepares the device again, which must clear every
        // trace of the first pass.
        let second = render_effect(&mut *device, &input, &config());
        assert!(first.left == second.left, "{kind}: left differs");
        assert!(first.right == second.right, "{kind}: right differs");
    }
}

#[test]
fn reset_clears_tails() {
    let registry = registry();
    let block = 512;
    let burst = white_noise(3, 0.5, 0.1, RATE);
    let quiet = silence(block);
    let ctx = TransportContext::playing_at(RATE, Tempo::DEFAULT, 0);
    for kind in KINDS {
        let mut device = create(&registry, kind);
        prepare(&mut *device, block);
        for chunk in burst.chunks_exact(block) {
            run_block(&mut *device, &ctx, &[chunk, chunk], 2, block);
        }
        device.reset();
        let out = run_block(&mut *device, &ctx, &[&quiet, &quiet], 2, block);
        for channel in &out {
            assert!(is_silent(channel, -140.0), "{kind}: sound after a reset");
        }
    }
}

#[test]
fn every_effect_works_at_other_sample_rates() {
    let registry = registry();
    for rate in [
        SampleRate::HZ_44100,
        SampleRate::HZ_96000,
        SampleRate::new(8_000).expect("rate"),
        SampleRate::new(192_000).expect("rate"),
    ] {
        let seconds = 0.5;
        let input = StereoBuffer::new(
            rate,
            white_noise(1, 0.5, seconds, rate),
            white_noise(2, 0.5, seconds, rate),
        );
        let config = RenderConfig::at_rate(rate);
        for kind in KINDS {
            let mut device = create(&registry, kind);
            let output = render_effect(&mut *device, &input, &config);
            assert_finite_and_sane(&output, &format!("{kind} at {} Hz", rate.hz()));
            assert!(
                rms_db(&output.left) > -60.0,
                "{kind} at {} Hz: nothing came out",
                rate.hz()
            );
        }
        // The limiter's latency is a time, so it scales with the rate.
        let mut limiter = Limiter::new();
        limiter.prepare(&PrepareConfig::stereo(rate, 512));
        let expected = (0.005 * rate.as_f64()).round() as u32;
        assert_eq!(limiter.latency_samples(), expected, "{} Hz", rate.hz());
    }
}

#[test]
fn a_chain_of_every_effect_survives_a_wav_round_trip() {
    let registry = registry();
    let mut buffer = programme(2.0);
    for kind in KINDS {
        let mut device = create(&registry, kind);
        buffer = render_effect(&mut *device, &buffer, &at_bpm(124.0));
        assert_finite_and_sane(&buffer, kind);
    }
    assert!(rms_db(&buffer.left) > -40.0, "the chain went quiet");
    // The limiter is last but three in the chain; nothing after it at its
    // defaults adds more than a few dB.
    assert!(peak(&buffer.left).max(peak(&buffer.right)) < 2.0);

    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("chain.wav");
    buffer.write_wav(&path).expect("write wav");
    let read = StereoBuffer::read_wav(&path).expect("read wav");
    assert_eq!(read.frames(), buffer.frames());
    assert!(read.left == buffer.left && read.right == buffer.right);
}
