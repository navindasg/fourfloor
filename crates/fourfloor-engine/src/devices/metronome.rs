use std::f32::consts::TAU;

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, SamplePos, SampleRate,
    TransportContext,
};

static PARAMS: [ParamInfo; 2] = [
    ParamInfo::new(Metronome::LEVEL, "Level", 0.0, 1.0, 0.5, ParamUnit::Generic),
    ParamInfo::new(
        Metronome::BEATS_PER_BAR,
        "Beats per bar",
        1.0,
        32.0,
        4.0,
        ParamUnit::Integer,
    ),
];

/// A click on every beat (four-on-the-floor), accented on the first beat of
/// each bar. It follows the transport: clicks start only while it rolls, on
/// the exact sample of each beat, which is `round(beat * samples_per_beat)`.
///
/// The click is a short decaying cosine burst, so its first sample is at
/// full level. Writes the same signal to every channel.
#[derive(Clone, Debug)]
pub struct Metronome {
    level: f32,
    beats_per_bar: u32,
    sample_rate: f32,
    /// Click length in samples.
    click_len: u32,
    /// Samples played of the current click; `>= click_len` means idle.
    click_pos: u32,
    click_freq: f32,
}

impl Metronome {
    /// Linear peak level of the click.
    pub const LEVEL: ParamId = ParamId(0);
    /// Beats per bar (the accent period). Rounded; at least 1.
    pub const BEATS_PER_BAR: ParamId = ParamId(1);

    /// Click length in seconds.
    pub const CLICK_SECONDS: f32 = 0.03;
    /// Pitch of the accented (downbeat) click, in Hz.
    pub const ACCENT_HZ: f32 = 1_760.0;
    /// Pitch of the other clicks, in Hz.
    pub const BEAT_HZ: f32 = 880.0;

    /// A metronome in 4/4 with the given linear click level.
    pub fn new(level: f32) -> Self {
        let mut metronome = Self {
            level,
            beats_per_bar: 4,
            sample_rate: 0.0,
            click_len: 0,
            click_pos: 0,
            click_freq: Self::BEAT_HZ,
        };
        metronome.configure(SampleRate::HZ_48000);
        metronome
    }

    fn configure(&mut self, sample_rate: SampleRate) {
        self.sample_rate = sample_rate.hz() as f32;
        self.click_len = ((self.sample_rate * Self::CLICK_SECONDS) as u32).max(1);
        self.click_pos = self.click_len;
    }

    /// Sample position of beat number `beat`.
    fn beat_position(beat: u64, samples_per_beat: f64) -> SamplePos {
        (beat as f64 * samples_per_beat).round() as SamplePos
    }

    fn next_sample(&mut self) -> f32 {
        if self.click_pos >= self.click_len {
            return 0.0;
        }
        let t = self.click_pos as f32;
        let fade = 1.0 - t / self.click_len as f32;
        self.click_pos += 1;
        self.level * fade * fade * (TAU * self.click_freq * t / self.sample_rate).cos()
    }
}

impl Device for Metronome {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.configure(config.sample_rate);
    }

    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let Some((first, rest)) = io.outputs.split_first_mut() else {
            return;
        };
        let samples_per_beat = ctx.tempo.samples_per_beat(ctx.sample_rate);
        let start = ctx.position_samples;

        // First beat at or after the block start. floor() can only land on
        // or before it, so step forward.
        let mut beat = (start as f64 / samples_per_beat).floor() as u64;
        let mut beat_at = Self::beat_position(beat, samples_per_beat);
        while beat_at < start {
            beat += 1;
            beat_at = Self::beat_position(beat, samples_per_beat);
        }

        for (frame, sample) in first.iter_mut().enumerate() {
            let position = start + frame as u64;
            if ctx.playing && position == beat_at {
                self.click_pos = 0;
                self.click_freq = if beat.is_multiple_of(u64::from(self.beats_per_bar)) {
                    Self::ACCENT_HZ
                } else {
                    Self::BEAT_HZ
                };
                while beat_at <= position {
                    beat += 1;
                    beat_at = Self::beat_position(beat, samples_per_beat);
                }
            }
            *sample = self.next_sample();
        }
        for channel in rest {
            channel.copy_from_slice(first);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if !value.is_finite() {
            return;
        }
        match param {
            Self::LEVEL => self.level = PARAMS[0].clamp(value),
            Self::BEATS_PER_BAR => {
                self.beats_per_bar = (PARAMS[1].clamp(value).round() as u32).max(1);
            }
            _ => {}
        }
    }

    fn reset(&mut self) {
        self.click_pos = self.click_len;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::run_source;
    use fourfloor_core::{Beats, Tempo};

    /// Renders `frames` from position 0 in blocks of `block`, rolling.
    fn render(bpm: f64, rate: SampleRate, frames: usize, block: usize) -> Vec<f32> {
        let tempo = Tempo::new(bpm).unwrap();
        let mut metronome = Metronome::new(1.0);
        metronome.prepare(&PrepareConfig::stereo(rate, block));
        let mut out = vec![0.0f32; frames];
        let mut position = 0u64;
        for chunk in out.chunks_mut(block) {
            let ctx = TransportContext {
                sample_rate: rate,
                tempo,
                playing: true,
                position_samples: position,
                position_beats: Beats::from_sample_pos(position, tempo, rate),
            };
            run_source(&mut metronome, &ctx, &mut [chunk]);
            position += chunk.len() as u64;
        }
        out
    }

    /// Indices where sound starts after at least 100 samples of silence.
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
    fn clicks_land_exactly_on_beats_at_120_bpm() {
        let out = render(120.0, SampleRate::HZ_48000, 24_000 * 4 + 10, 256);
        assert_eq!(onsets(&out), [0, 24_000, 48_000, 72_000, 96_000]);
        assert_eq!(out[0], 1.0, "a click starts at full level");
        assert_eq!(out[23_999], 0.0);
    }

    #[test]
    fn clicks_round_to_the_nearest_sample_at_fractional_tempos() {
        // 128 BPM at 44.1 kHz: 20671.875 samples per beat.
        let out = render(128.0, SampleRate::HZ_44100, 90_000, 512);
        assert_eq!(onsets(&out), [0, 20_672, 41_344, 62_016, 82_688]);
    }

    #[test]
    fn output_does_not_depend_on_block_size() {
        let reference = render(126.0, SampleRate::HZ_44100, 50_000, 50_000);
        for block in [1, 7, 64, 1024] {
            assert_eq!(
                render(126.0, SampleRate::HZ_44100, 50_000, block),
                reference
            );
        }
    }

    #[test]
    fn downbeat_is_accented_every_four_beats() {
        let out = render(120.0, SampleRate::HZ_48000, 24_000 * 5, 480);
        // One sample into a click the two pitches already differ.
        let second = |beat: usize| out[beat * 24_000 + 1];
        assert_eq!(second(0), second(4));
        assert_eq!(second(1), second(2));
        assert_ne!(second(0), second(1));
    }

    #[test]
    fn silent_while_stopped_and_after_reset() {
        let mut metronome = Metronome::new(1.0);
        metronome.prepare(&PrepareConfig::stereo(SampleRate::HZ_48000, 64));
        let stopped = TransportContext::stopped(SampleRate::HZ_48000, Tempo::DEFAULT);
        let mut out = [9.0f32; 64];
        run_source(&mut metronome, &stopped, &mut [&mut out]);
        assert!(out.iter().all(|s| *s == 0.0));

        let playing = TransportContext {
            playing: true,
            ..stopped
        };
        run_source(&mut metronome, &playing, &mut [&mut out]);
        assert_eq!(out[0], 1.0);
        metronome.reset();
        run_source(&mut metronome, &stopped, &mut [&mut out]);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn params_set_level_and_bar_length() {
        let mut metronome = Metronome::new(1.0);
        metronome.prepare(&PrepareConfig::stereo(SampleRate::HZ_48000, 4));
        metronome.set_param(Metronome::LEVEL, 0.5);
        metronome.set_param(Metronome::BEATS_PER_BAR, 0.0); // clamps to 1
        metronome.set_param(Metronome::LEVEL, f32::NAN);
        assert_eq!(metronome.beats_per_bar, 1);
        let ctx = TransportContext {
            playing: true,
            ..TransportContext::stopped(SampleRate::HZ_48000, Tempo::DEFAULT)
        };
        let mut out = [0.0f32; 4];
        run_source(&mut metronome, &ctx, &mut [&mut out]);
        assert_eq!(out[0], 0.5);
    }
}
