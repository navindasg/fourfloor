//! Tempo-synced stereo and ping-pong delay.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::delay_line::DelayLine;
use crate::util::{process_stereo, time_coefficient, OnePole, ParamSet, Smoothed};

const PARAM_COUNT: usize = 8;
/// Longest delay the lines hold, in seconds.
const MAX_SECONDS: f64 = 4.0;

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::toggle(Delay::SYNC, "Sync", true),
    ParamInfo::new(Delay::BEATS, "Time", 0.0625, 4.0, 0.75, ParamUnit::Beats).logarithmic(),
    ParamInfo::new(
        Delay::MILLISECONDS,
        "Time (free)",
        1.0,
        2_000.0,
        375.0,
        ParamUnit::Milliseconds,
    )
    .logarithmic(),
    ParamInfo::new(
        Delay::FEEDBACK,
        "Feedback",
        0.0,
        0.95,
        0.4,
        ParamUnit::Generic,
    ),
    ParamInfo::toggle(Delay::PING_PONG, "Ping Pong", true),
    ParamInfo::new(
        Delay::LOW_CUT,
        "Low Cut",
        20.0,
        2_000.0,
        150.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(
        Delay::HIGH_CUT,
        "High Cut",
        500.0,
        20_000.0,
        6_000.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::new(Delay::MIX, "Mix", 0.0, 1.0, 0.3, ParamUnit::Generic),
];

/// A stereo delay (`fx.delay`) whose time follows the tempo.
///
/// With `Sync` on the delay time is `Time` beats at the current tempo (0.75
/// is a dotted eighth); otherwise it is `Time (free)` milliseconds. Times
/// longer than four seconds are shortened to four.
///
/// In stereo mode left and right each repeat on their own side. In
/// ping-pong mode the input is summed to mono and the repeats alternate:
/// the first on the left, the second on the right, and so on.
///
/// Everything written into the delay passes through a low cut and a high
/// cut (6 dB/octave each), so each repeat is thinner and darker than the
/// one before, like a tape or bucket-brigade echo. `Feedback` is the level
/// of each repeat relative to the previous one.
///
/// A change of delay time glides over about 50 ms, which bends the pitch of
/// the repeats for a moment, as on a tape delay.
#[derive(Clone, Debug)]
pub struct Delay {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f32,
    lines: [DelayLine; 2],
    low_cut: [OnePole; 2],
    high_cut: [OnePole; 2],
    /// Current delay in samples; negative until first used.
    time: f64,
    time_coefficient: f64,
    mix: Smoothed,
    feedback: Smoothed,
}

impl Default for Delay {
    fn default() -> Self {
        Self::new()
    }
}

impl Delay {
    /// Follow the tempo (`BEATS`) or not (`MILLISECONDS`).
    pub const SYNC: ParamId = ParamId(0);
    /// Delay time in beats, when synced.
    pub const BEATS: ParamId = ParamId(1);
    /// Delay time in milliseconds, when not synced.
    pub const MILLISECONDS: ParamId = ParamId(2);
    /// Level of each repeat relative to the one before, 0 to 0.95.
    pub const FEEDBACK: ParamId = ParamId(3);
    /// Alternate repeats between left and right.
    pub const PING_PONG: ParamId = ParamId(4);
    /// Low cut on the repeats, in Hz.
    pub const LOW_CUT: ParamId = ParamId(5);
    /// High cut on the repeats, in Hz.
    pub const HIGH_CUT: ParamId = ParamId(6);
    /// Dry/wet mix: 0 is dry only, 1 is repeats only.
    pub const MIX: ParamId = ParamId(7);

    /// A dotted-eighth ping-pong delay.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            lines: [DelayLine::new(), DelayLine::new()],
            low_cut: [OnePole::default(); 2],
            high_cut: [OnePole::default(); 2],
            time: -1.0,
            time_coefficient: 1.0,
            mix: Smoothed::new(),
            feedback: Smoothed::new(),
        }
    }
}

impl Device for Delay {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.hz() as f32;
        let max = (MAX_SECONDS * f64::from(self.sample_rate)).ceil() as usize + 2;
        for line in &mut self.lines {
            line.prepare(max);
        }
        self.time_coefficient = f64::from(time_coefficient(0.05, self.sample_rate));
        self.mix.set_time(0.01, self.sample_rate);
        self.feedback.set_time(0.01, self.sample_rate);
        self.params.mark_dirty();
        self.reset();
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        let rate = self.sample_rate;
        if self.params.take_dirty() {
            for filter in &mut self.low_cut {
                filter.set_cutoff(self.params.get(Self::LOW_CUT), rate);
            }
            for filter in &mut self.high_cut {
                filter.set_cutoff(self.params.get(Self::HIGH_CUT), rate);
            }
        }
        let p = &self.params;
        let seconds = if p.on(Self::SYNC) {
            f64::from(p.get(Self::BEATS)) * ctx.tempo.seconds_per_beat()
        } else {
            f64::from(p.get(Self::MILLISECONDS)) * 0.001
        };
        let limit = (self.lines[0].max_delay().saturating_sub(2)).max(1) as f64;
        let target = (seconds.min(MAX_SECONDS) * f64::from(rate)).clamp(1.0, limit);
        let ping_pong = p.on(Self::PING_PONG);
        let (mix_target, feedback_target) = (p.get(Self::MIX), p.get(Self::FEEDBACK));
        if self.time < 0.0 {
            self.time = target;
        }
        let glide = self.time_coefficient;
        let Self {
            lines,
            low_cut,
            high_cut,
            time,
            mix,
            feedback,
            ..
        } = self;
        process_stereo(io, |frame| {
            if *time != target {
                *time += (target - *time) * glide;
                if (target - *time).abs() < 1.0e-3 {
                    *time = target;
                }
            }
            let mix = mix.next(mix_target);
            let feedback = feedback.next(feedback_target);
            let taps = [
                lines[0].tap_fractional(*time),
                lines[1].tap_fractional(*time),
            ];
            let write = if ping_pong {
                [
                    0.5 * (frame.input[0] + frame.input[1]) + taps[1] * feedback,
                    taps[0] * feedback,
                ]
            } else {
                [
                    frame.input[0] + taps[0] * feedback,
                    frame.input[1] + taps[1] * feedback,
                ]
            };
            for channel in 0..2 {
                let filtered = high_cut[channel].low(low_cut[channel].high(write[channel]));
                lines[channel].push(filtered);
            }
            [
                frame.input[0] * (1.0 - mix) + taps[0] * mix,
                frame.input[1] * (1.0 - mix) + taps[1] * mix,
            ]
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
        for filter in self.low_cut.iter_mut().chain(self.high_cut.iter_mut()) {
            filter.reset();
        }
        self.time = -1.0;
        self.mix.reset();
        self.feedback.reset();
    }
}
