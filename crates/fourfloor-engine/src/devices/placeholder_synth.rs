use std::f32::consts::TAU;

use fourfloor_core::{
    pitch_to_hz, velocity_to_unit, Device, EventKind, ParamId, ParamInfo, ParamUnit, PrepareConfig,
    ProcessIo, SampleRate, TransportContext,
};

/// A placeholder instrument: the sound a MIDI track makes until it is given
/// a real instrument device.
///
/// It plays the note events it receives. Voices are chosen by General MIDI
/// drum pitch: 35 and 36 are a swept-sine kick, 37 to 40 a noise snare, 42
/// and 44 a closed hat, 46 an open hat, and every other pitch a sine at the
/// note's frequency, held until its note-off. Drum voices ring out and
/// ignore note-offs. Writes the same signal to every channel.
#[derive(Clone, Debug)]
pub struct PlaceholderSynth {
    voices: [Voice; PlaceholderSynth::VOICES],
    level: f32,
    sample_rate: f32,
    noise: u32,
}

static PARAMS: [ParamInfo; 1] = [ParamInfo::new(
    PlaceholderSynth::LEVEL,
    "Level",
    0.0,
    1.0,
    PlaceholderSynth::DEFAULT_LEVEL,
    ParamUnit::Generic,
)];

#[derive(Clone, Copy, Debug, PartialEq)]
enum VoiceKind {
    Kick,
    /// Noise burst. `bright` differentiates the noise, which tilts it
    /// towards high frequencies (hats).
    Noise {
        bright: bool,
    },
    Tone,
}

#[derive(Clone, Copy, Debug)]
struct Voice {
    kind: VoiceKind,
    pitch: u8,
    channel: u8,
    /// Samples played so far; `>= len` means idle.
    age: u32,
    /// Total length in samples. A held tone has `u32::MAX` until released.
    len: u32,
    level: f32,
    /// Decay time constant in samples (kick and noise).
    decay: f32,
    /// Oscillator frequency in Hz (tone) and running phase in cycles.
    freq: f32,
    phase: f32,
    last_noise: f32,
}

impl Voice {
    const IDLE: Self = Self {
        kind: VoiceKind::Tone,
        pitch: 0,
        channel: 0,
        age: 0,
        len: 0,
        level: 0.0,
        decay: 1.0,
        freq: 0.0,
        phase: 0.0,
        last_noise: 0.0,
    };

    fn is_idle(&self) -> bool {
        self.age >= self.len
    }

    fn is_held_tone(&self) -> bool {
        self.kind == VoiceKind::Tone && self.len == u32::MAX
    }
}

impl PlaceholderSynth {
    /// Linear output level, 0 to 1: the peak of one full-velocity note.
    pub const LEVEL: ParamId = ParamId(0);
    /// The level a new synth starts at. Leaves headroom for a few voices to
    /// sum without clipping.
    pub const DEFAULT_LEVEL: f32 = 0.5;

    /// Notes that can sound at once. When all are busy the oldest is cut.
    pub const VOICES: usize = 8;

    const NOISE_SEED: u32 = 0x9E37_79B9;
    const KICK_SECONDS: f32 = 0.35;
    const KICK_DECAY_SECONDS: f32 = 0.09;
    const KICK_START_HZ: f32 = 160.0;
    const KICK_END_HZ: f32 = 48.0;
    const KICK_SWEEP_SECONDS: f32 = 0.03;
    const TONE_ATTACK_SECONDS: f32 = 0.002;
    const TONE_RELEASE_SECONDS: f32 = 0.01;

    /// A synth at the given linear level.
    pub fn new(level: f32) -> Self {
        Self {
            voices: [Voice::IDLE; Self::VOICES],
            level: PARAMS[0].clamp(level),
            sample_rate: SampleRate::HZ_48000.hz() as f32,
            noise: Self::NOISE_SEED,
        }
    }

    fn seconds(&self, seconds: f32) -> u32 {
        ((seconds * self.sample_rate) as u32).max(1)
    }

    fn note_on(&mut self, pitch: u8, velocity: u8, channel: u8) {
        let (kind, len, decay) = match pitch {
            35 | 36 => (
                VoiceKind::Kick,
                self.seconds(Self::KICK_SECONDS),
                Self::KICK_DECAY_SECONDS,
            ),
            37..=40 => (VoiceKind::Noise { bright: false }, self.seconds(0.2), 0.04),
            42 | 44 => (VoiceKind::Noise { bright: true }, self.seconds(0.05), 0.012),
            46 => (VoiceKind::Noise { bright: true }, self.seconds(0.2), 0.05),
            _ => (VoiceKind::Tone, u32::MAX, 0.0),
        };
        let slot = self
            .voices
            .iter()
            .position(Voice::is_idle)
            .or_else(|| (0..Self::VOICES).max_by_key(|&i| self.voices[i].age))
            .unwrap_or(0);
        self.voices[slot] = Voice {
            kind,
            pitch,
            channel,
            age: 0,
            len,
            level: velocity_to_unit(velocity),
            decay: (decay * self.sample_rate).max(1.0),
            freq: pitch_to_hz(f32::from(pitch)),
            phase: 0.0,
            last_noise: 0.0,
        };
    }

    /// Starts the release of every held tone that `matches`.
    fn release(&mut self, matches: impl Fn(&Voice) -> bool) {
        let release = self.seconds(Self::TONE_RELEASE_SECONDS);
        for voice in &mut self.voices {
            if voice.is_held_tone() && matches(voice) {
                voice.len = voice.age.saturating_add(release);
            }
        }
    }

    fn handle(&mut self, kind: EventKind) {
        match kind {
            EventKind::NoteOn {
                pitch,
                velocity,
                channel,
            } => self.note_on(pitch, velocity, channel),
            EventKind::NoteOff { pitch, channel, .. } => {
                self.release(|voice| voice.pitch == pitch && voice.channel == channel);
            }
            EventKind::AllNotesOff => self.release(|_| true),
            EventKind::ControlChange { .. } | EventKind::PitchBend { .. } => {}
        }
    }

    /// White noise in -1.0..1.0 from a xorshift generator: deterministic,
    /// so renders are repeatable.
    fn next_noise(state: &mut u32) -> f32 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        *state = x;
        (x >> 8) as f32 / 8_388_608.0 - 1.0
    }

    fn next_sample(&mut self) -> f32 {
        let rate = self.sample_rate;
        let mut sum = 0.0;
        for voice in &mut self.voices {
            if voice.is_idle() {
                continue;
            }
            let t = voice.age as f32;
            let sample = match voice.kind {
                VoiceKind::Kick => {
                    // Linear fade to exactly zero at the end: no click.
                    let fade = 1.0 - t / voice.len as f32;
                    let sweep = (-t / (Self::KICK_SWEEP_SECONDS * rate)).exp();
                    let freq =
                        Self::KICK_END_HZ + (Self::KICK_START_HZ - Self::KICK_END_HZ) * sweep;
                    let out = (TAU * voice.phase).sin() * (-t / voice.decay).exp() * fade;
                    voice.phase = (voice.phase + freq / rate).fract();
                    out
                }
                VoiceKind::Noise { bright } => {
                    let fade = 1.0 - t / voice.len as f32;
                    let white = Self::next_noise(&mut self.noise);
                    let shaped = if bright {
                        0.5 * (white - voice.last_noise)
                    } else {
                        white
                    };
                    voice.last_noise = white;
                    shaped * (-t / voice.decay).exp() * fade
                }
                VoiceKind::Tone => {
                    let attack = (t / (Self::TONE_ATTACK_SECONDS * rate)).min(1.0);
                    let release = if voice.len == u32::MAX {
                        1.0
                    } else {
                        let left = (voice.len - voice.age) as f32;
                        (left / (Self::TONE_RELEASE_SECONDS * rate)).min(1.0)
                    };
                    let out = (TAU * voice.phase).sin() * attack * release;
                    voice.phase = (voice.phase + voice.freq / rate).fract();
                    out
                }
            };
            // A tone held for days must not wrap its age into "idle".
            voice.age = (voice.age + 1).min(u32::MAX - 1);
            sum += sample * voice.level;
        }
        sum * self.level
    }
}

impl Device for PlaceholderSynth {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        self.reset();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let events = io.events;
        let Some((first, rest)) = io.outputs.split_first_mut() else {
            return;
        };
        let mut next = 0;
        for (frame, sample) in first.iter_mut().enumerate() {
            while let Some(event) = events.get(next).filter(|e| e.frame as usize <= frame) {
                self.handle(event.kind);
                next += 1;
            }
            *sample = self.next_sample();
        }
        // Events past the block (a contract violation upstream) still count.
        for event in &events[next..] {
            self.handle(event.kind);
        }
        for channel in rest {
            channel.copy_from_slice(first);
        }
    }

    fn params(&self) -> &[ParamInfo] {
        &PARAMS
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        if param == Self::LEVEL && value.is_finite() {
            self.level = PARAMS[0].clamp(value);
        }
    }

    fn reset(&mut self) {
        self.voices = [Voice::IDLE; Self::VOICES];
        self.noise = Self::NOISE_SEED;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::test_util::{run, stopped, RATE};
    use fourfloor_core::Event;

    /// Renders `frames` in blocks of `block`. `events` are
    /// `(absolute frame, kind)`, sorted.
    fn render(
        synth: &mut PlaceholderSynth,
        events: &[(usize, EventKind)],
        frames: usize,
        block: usize,
    ) -> Vec<f32> {
        synth.prepare(&PrepareConfig::stereo(RATE, block));
        let (mut left, mut right) = (vec![9.0f32; frames], vec![9.0f32; frames]);
        let mut start = 0;
        for (l, r) in left.chunks_mut(block).zip(right.chunks_mut(block)) {
            let end = start + l.len();
            let in_block: Vec<Event> = events
                .iter()
                .filter(|(at, _)| (start..end).contains(at))
                .map(|(at, kind)| Event::new((at - start) as u32, *kind))
                .collect();
            run(synth, &stopped(), &[], None, &mut [l, r], &in_block);
            start = end;
        }
        assert_eq!(left, right, "every channel carries the same signal");
        left
    }

    fn first_sound(signal: &[f32]) -> Option<usize> {
        signal.iter().position(|s| *s != 0.0)
    }

    fn peak(signal: &[f32]) -> f32 {
        signal.iter().fold(0.0f32, |m, s| m.max(s.abs()))
    }

    #[test]
    fn is_silent_without_events() {
        let out = render(&mut PlaceholderSynth::new(1.0), &[], 4_800, 256);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn a_note_starts_on_the_exact_frame_of_its_event() {
        let events = [(36_000, EventKind::note_on(60, 127))];
        let out = render(&mut PlaceholderSynth::new(1.0), &events, 48_000, 512);
        // A sine starts at zero, so the first non-zero sample is one later.
        assert_eq!(first_sound(&out), Some(36_001));

        let hat = [(12_000, EventKind::note_on(42, 127))];
        let out = render(&mut PlaceholderSynth::new(1.0), &hat, 24_000, 100);
        let onset = first_sound(&out).unwrap();
        assert!((12_000..12_002).contains(&onset), "onset at {onset}");
    }

    #[test]
    fn output_does_not_depend_on_block_size() {
        let events = [
            (0, EventKind::note_on(36, 120)),
            (6_000, EventKind::note_on(42, 90)),
            (12_000, EventKind::note_on(46, 90)),
            (24_000, EventKind::note_on(36, 120)),
            (24_000, EventKind::note_on(64, 100)),
            (30_000, EventKind::note_off(64)),
        ];
        let reference = render(&mut PlaceholderSynth::new(0.8), &events, 40_000, 40_000);
        for block in [1, 7, 64, 511, 4096] {
            let out = render(&mut PlaceholderSynth::new(0.8), &events, 40_000, block);
            assert_eq!(out, reference, "block size {block}");
        }
    }

    #[test]
    fn kick_is_loud_at_the_onset_and_gone_after_its_length() {
        let events = [
            (0, EventKind::note_on(36, 127)),
            (10, EventKind::note_off(36)),
        ];
        let out = render(&mut PlaceholderSynth::new(1.0), &events, 48_000, 512);
        assert!(peak(&out[..960]) > 0.5, "onset peak {}", peak(&out[..960]));
        assert!(peak(&out) <= 1.0);
        assert!(
            peak(&out[2_400..9_600]) > 0.05,
            "a note-off does not cut a drum"
        );
        assert_eq!(peak(&out[20_000..]), 0.0);
    }

    #[test]
    fn a_tone_sounds_at_its_pitch_until_its_note_off_then_releases() {
        // A4 for 6 000 samples; the release adds 480.
        let events = [
            (0, EventKind::note_on(69, 127)),
            (6_000, EventKind::note_off(69)),
        ];
        let out = render(&mut PlaceholderSynth::new(1.0), &events, 12_000, 512);
        let last = out.iter().rposition(|s| *s != 0.0).unwrap();
        assert!((6_000..6_480).contains(&last), "last sound at {last}");
        // 440 Hz: about 55 upward zero crossings in 6 000 samples.
        let crossings = out[..6_000]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!((54..=56).contains(&crossings), "{crossings} cycles");

        let held = [(0, EventKind::note_on(69, 127))];
        let out = render(&mut PlaceholderSynth::new(1.0), &held, 48_000, 512);
        assert!(peak(&out[40_000..]) > 0.9, "held until released");
    }

    #[test]
    fn note_off_only_releases_its_own_pitch_and_all_notes_off_releases_all() {
        let events = [
            (0, EventKind::note_on(60, 127)),
            (0, EventKind::note_on(72, 127)),
            (1_000, EventKind::note_off(60)),
        ];
        let out = render(&mut PlaceholderSynth::new(0.5), &events, 9_600, 256);
        assert!(peak(&out[4_800..]) > 0.4, "72 still sounds");

        let all = [
            (0, EventKind::note_on(60, 127)),
            (0, EventKind::note_on(72, 127)),
            (1_000, EventKind::AllNotesOff),
        ];
        let out = render(&mut PlaceholderSynth::new(0.5), &all, 9_600, 256);
        assert_eq!(peak(&out[2_000..]), 0.0);
    }

    #[test]
    fn velocity_and_level_scale_the_output() {
        let loud = render(
            &mut PlaceholderSynth::new(1.0),
            &[(0, EventKind::note_on(36, 127))],
            4_800,
            512,
        );
        let mut quiet_synth = PlaceholderSynth::new(1.0);
        quiet_synth.set_param(PlaceholderSynth::LEVEL, 0.5);
        quiet_synth.set_param(PlaceholderSynth::LEVEL, f32::NAN);
        let soft = render(
            &mut quiet_synth,
            &[(0, EventKind::note_on(36, 64))],
            4_800,
            512,
        );
        for (l, s) in loud.iter().zip(&soft) {
            assert!((l * 0.5 * 64.0 / 127.0 - s).abs() < 1e-6);
        }
        assert_eq!(quiet_synth.params()[0].name, "Level");
    }

    #[test]
    fn many_simultaneous_notes_steal_voices_without_panicking() {
        let events: Vec<(usize, EventKind)> = (0..40)
            .map(|i| (i * 250, EventKind::note_on(40 + (i as u8 % 40), 100)))
            .collect();
        let out = render(&mut PlaceholderSynth::new(0.1), &events, 24_000, 128);
        assert!(out.iter().all(|s| s.is_finite()));
        assert!(out.iter().any(|s| *s != 0.0));
    }

    #[test]
    fn reset_silences_ringing_voices_and_stray_frames_are_tolerated() {
        let mut synth = PlaceholderSynth::new(1.0);
        synth.prepare(&PrepareConfig::stereo(RATE, 64));
        let mut block = [0.0f32; 64];
        // Frame 500 is outside the block; it must not panic or be lost.
        let events = [Event::note_on(0, 36, 127), Event::note_on(500, 69, 127)];
        run(
            &mut synth,
            &stopped(),
            &[],
            None,
            &mut [&mut block],
            &events,
        );
        assert!(block.iter().any(|s| *s != 0.0));
        synth.reset();
        run(&mut synth, &stopped(), &[], None, &mut [&mut block], &[]);
        assert_eq!(block, [0.0; 64]);
    }
}
