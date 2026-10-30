//! Juno-style stereo chorus.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::clock::PhaseClock;
use crate::delay_line::DelayLine;
use crate::util::{process_stereo, OnePole, ParamSet, Smoothed};

const PARAM_COUNT: usize = 4;
const MAX_DELAY_MS: f32 = 16.0;

/// One of the three button settings: LFO rate and the range the delay
/// sweeps.
struct Setting {
    rate_hz: f64,
    min_ms: f32,
    max_ms: f32,
}

/// Approximate figures for the Juno-60 chorus as measured by several
/// hobbyist analyses: a slow wide sweep (I), a faster one (II), and a fast
/// shallow vibrato with both buttons in (I+II).
const SETTINGS: [Setting; 3] = [
    Setting {
        rate_hz: 0.513,
        min_ms: 1.66,
        max_ms: 5.35,
    },
    Setting {
        rate_hz: 0.863,
        min_ms: 1.66,
        max_ms: 5.35,
    },
    Setting {
        rate_hz: 9.75,
        min_ms: 3.3,
        max_ms: 3.7,
    },
];

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::new(Chorus::MODE, "Mode", 0.0, 2.0, 0.0, ParamUnit::Integer),
    ParamInfo::new(Chorus::MIX, "Mix", 0.0, 1.0, 0.5, ParamUnit::Generic),
    ParamInfo::new(Chorus::DEPTH, "Depth", 0.0, 1.0, 1.0, ParamUnit::Generic),
    ParamInfo::new(
        Chorus::TONE,
        "Tone",
        2_000.0,
        20_000.0,
        9_000.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
];

/// A stereo chorus in the manner of the Roland Juno-60 (`fx.chorus`).
///
/// One short delay per channel, swept by a triangle LFO. The right channel
/// is swept in the opposite direction to the left, so while one side's
/// pitch rises the other falls: that is what turns a mono source into a
/// wide stereo one. The delayed signal is low-passed (`Tone`) like the
/// bucket-brigade chips of the original, and mixed with the dry signal.
///
/// Modes 0, 1 and 2 are the buttons I, II and I+II. The rates and delay
/// ranges are approximations of the hardware, not a circuit model; the
/// original's noise and companding are not imitated.
#[derive(Clone, Debug)]
pub struct Chorus {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    lines: [DelayLine; 2],
    tone: [OnePole; 2],
    clock: PhaseClock,
    centre: Smoothed,
    swing: Smoothed,
    mix: Smoothed,
}

impl Default for Chorus {
    fn default() -> Self {
        Self::new()
    }
}

impl Chorus {
    /// Button setting: 0 is I, 1 is II, 2 is I+II.
    pub const MODE: ParamId = ParamId(0);
    /// Dry/wet mix. 0.5 is the classic equal blend.
    pub const MIX: ParamId = ParamId(1);
    /// Scales how far the delay sweeps, 0 to 1.
    pub const DEPTH: ParamId = ParamId(2);
    /// Low-pass on the delayed signal, in Hz.
    pub const TONE: ParamId = ParamId(3);

    /// Chorus I at an equal blend.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            lines: [DelayLine::new(), DelayLine::new()],
            tone: [OnePole::default(); 2],
            clock: PhaseClock::default(),
            centre: Smoothed::new(),
            swing: Smoothed::new(),
            mix: Smoothed::new(),
        }
    }
}

impl Device for Chorus {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        let capacity = (MAX_DELAY_MS * 0.001 * self.sample_rate).ceil() as usize + 4;
        for line in &mut self.lines {
            line.prepare(capacity);
        }
        self.centre.set_time(0.05, self.sample_rate);
        self.swing.set_time(0.05, self.sample_rate);
        self.mix.set_time(0.01, self.sample_rate);
        self.params.mark_dirty();
        self.reset();
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let rate = self.sample_rate;
        if self.params.take_dirty() {
            for filter in &mut self.tone {
                filter.set_cutoff(self.params.get(Self::TONE), rate);
            }
        }
        let p = &self.params;
        let setting = &SETTINGS[p.choice(Self::MODE).min(SETTINGS.len() - 1)];
        let to_samples = 0.001 * rate;
        let centre_target = 0.5 * (setting.min_ms + setting.max_ms) * to_samples;
        let swing_target =
            0.5 * (setting.max_ms - setting.min_ms) * to_samples * p.get(Self::DEPTH);
        let mix_target = p.get(Self::MIX);
        let phase = self
            .clock
            .free(setting.rate_hz, f64::from(rate), io.frames());
        let Self {
            lines,
            tone,
            centre,
            swing,
            mix,
            ..
        } = self;
        let mut index = 0usize;
        process_stereo(io, |frame| {
            let position = phase.at(index, 0.0) as f32;
            index += 1;
            // Triangle from -1 to 1.
            let lfo = 1.0 - 4.0 * (position - 0.5).abs();
            let (centre, swing) = (centre.next(centre_target), swing.next(swing_target));
            let mix = mix.next(mix_target);
            let delays = [centre + swing * lfo, centre - swing * lfo];
            let mut out = [0.0f32; 2];
            for channel in 0..2 {
                let delayed = lines[channel].tap_fractional(f64::from(delays[channel]));
                lines[channel].push(frame.input[channel]);
                let wet = tone[channel].low(delayed);
                out[channel] = frame.input[channel] * (1.0 - mix) + wet * mix;
            }
            out
        });
    }

    fn params(&self) -> &[ParamInfo] {
        self.params.table()
    }

    fn set_param(&mut self, param: ParamId, value: f32) {
        self.params.set(param, value);
    }

    fn reset(&mut self) {
        for line in &mut self.lines {
            line.clear();
        }
        for filter in &mut self.tone {
            filter.reset();
        }
        self.clock.reset();
        self.centre.reset();
        self.swing.reset();
        self.mix.reset();
    }
}
