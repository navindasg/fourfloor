//! Four-band parametric equaliser with high-pass and low-pass filters.

use fourfloor_core::{
    Device, ParamId, ParamInfo, ParamUnit, PrepareConfig, ProcessIo, TransportContext,
};

use crate::biquad::{Biquad, Coefficients, BUTTERWORTH_Q};
use crate::util::{db_to_gain, process_stereo, ParamSet, Smoothed};

const BANDS: usize = 4;
const SECTIONS: usize = BANDS + 2;
const PARAM_COUNT: usize = 19;
/// Time constant with which a band glides to new settings.
const GLIDE_SECONDS: f64 = 0.005;

const fn freq(id: u32, name: &'static str, default: f32) -> ParamInfo {
    ParamInfo::new(ParamId(id), name, 20.0, 20_000.0, default, ParamUnit::Hertz).logarithmic()
}

const fn gain(id: u32, name: &'static str) -> ParamInfo {
    ParamInfo::new(ParamId(id), name, -24.0, 24.0, 0.0, ParamUnit::Decibels)
}

const fn q(id: u32, name: &'static str) -> ParamInfo {
    ParamInfo::new(ParamId(id), name, 0.1, 18.0, 0.71, ParamUnit::Generic).logarithmic()
}

static PARAMS: [ParamInfo; PARAM_COUNT] = [
    ParamInfo::toggle(ParametricEq::HP_ON, "High Pass", false),
    ParamInfo::new(
        ParametricEq::HP_FREQ,
        "High Pass Freq",
        20.0,
        2_000.0,
        30.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    freq(2, "Band 1 Freq", 100.0),
    gain(3, "Band 1 Gain"),
    q(4, "Band 1 Q"),
    freq(5, "Band 2 Freq", 500.0),
    gain(6, "Band 2 Gain"),
    q(7, "Band 2 Q"),
    freq(8, "Band 3 Freq", 2_500.0),
    gain(9, "Band 3 Gain"),
    q(10, "Band 3 Q"),
    freq(11, "Band 4 Freq", 8_000.0),
    gain(12, "Band 4 Gain"),
    q(13, "Band 4 Q"),
    ParamInfo::toggle(ParametricEq::LP_ON, "Low Pass", false),
    ParamInfo::new(
        ParametricEq::LP_FREQ,
        "Low Pass Freq",
        200.0,
        20_000.0,
        18_000.0,
        ParamUnit::Hertz,
    )
    .logarithmic(),
    ParamInfo::toggle(ParametricEq::BAND1_SHELF, "Band 1 Shelf", false),
    ParamInfo::toggle(ParametricEq::BAND4_SHELF, "Band 4 Shelf", false),
    ParamInfo::new(
        ParametricEq::OUTPUT,
        "Output",
        -24.0,
        24.0,
        0.0,
        ParamUnit::Decibels,
    ),
];

/// A four-band parametric equaliser (`fx.eq`).
///
/// Four bells, each with frequency, gain and Q. Band 1 can become a low
/// shelf and band 4 a high shelf. A 12 dB/octave high-pass and low-pass sit
/// before and after the bands. A band at 0 dB, and a filter that is switched
/// off, are skipped entirely, so the default settings pass audio through
/// bit for bit.
///
/// A change to a band glides in over a few milliseconds, sample by sample,
/// so automation neither steps nor clicks. Switching the high-pass or the
/// low-pass on or off is immediate.
#[derive(Clone, Debug)]
pub struct ParametricEq {
    params: ParamSet<PARAM_COUNT>,
    sample_rate: f64,
    /// Coefficients in use, gliding to `targets`.
    coefficients: [Coefficients; SECTIONS],
    targets: [Coefficients; SECTIONS],
    /// Sections that are processed.
    active: [bool; SECTIONS],
    /// Sections the parameters ask for. An active band that is no longer
    /// wanted glides to flat and is then dropped.
    wanted: [bool; SECTIONS],
    settled: bool,
    /// False until the first block after a reset, which takes its settings
    /// at once.
    primed: bool,
    glide: f64,
    state: [[Biquad; SECTIONS]; 2],
    output: Smoothed,
}

impl Default for ParametricEq {
    fn default() -> Self {
        Self::new()
    }
}

impl ParametricEq {
    /// High-pass on/off.
    pub const HP_ON: ParamId = ParamId(0);
    /// High-pass corner frequency in Hz.
    pub const HP_FREQ: ParamId = ParamId(1);
    /// Low-pass on/off.
    pub const LP_ON: ParamId = ParamId(14);
    /// Low-pass corner frequency in Hz.
    pub const LP_FREQ: ParamId = ParamId(15);
    /// Makes band 1 a low shelf.
    pub const BAND1_SHELF: ParamId = ParamId(16);
    /// Makes band 4 a high shelf.
    pub const BAND4_SHELF: ParamId = ParamId(17);
    /// Output gain in decibels.
    pub const OUTPUT: ParamId = ParamId(18);

    /// Centre frequency of `band` (0 to 3) in Hz.
    pub const fn band_freq(band: u32) -> ParamId {
        ParamId(2 + band * 3)
    }

    /// Gain of `band` (0 to 3) in decibels.
    pub const fn band_gain(band: u32) -> ParamId {
        ParamId(3 + band * 3)
    }

    /// Q of `band` (0 to 3).
    pub const fn band_q(band: u32) -> ParamId {
        ParamId(4 + band * 3)
    }

    /// An equaliser with every band flat.
    pub fn new() -> Self {
        Self {
            params: ParamSet::new(&PARAMS),
            sample_rate: 48_000.0,
            coefficients: [Coefficients::IDENTITY; SECTIONS],
            targets: [Coefficients::IDENTITY; SECTIONS],
            active: [false; SECTIONS],
            wanted: [false; SECTIONS],
            settled: true,
            primed: false,
            glide: 1.0,
            state: [[Biquad::default(); SECTIONS]; 2],
            output: Smoothed::new(),
        }
    }

    // The loops below walk several parallel arrays by section number.
    #[allow(clippy::needless_range_loop)]
    fn update(&mut self) {
        let rate = self.sample_rate;
        let p = &self.params;
        let last = SECTIONS - 1;
        // What a band glides from when it is switched in: the same band at
        // 0 dB, which passes the signal unchanged.
        let mut flat = [Coefficients::IDENTITY; SECTIONS];
        self.wanted[0] = p.on(Self::HP_ON);
        self.targets[0] =
            Coefficients::high_pass(rate, f64::from(p.get(Self::HP_FREQ)), BUTTERWORTH_Q);
        for band in 0..BANDS {
            let index = band as u32;
            let frequency = f64::from(p.get(Self::band_freq(index)));
            let gain_db = f64::from(p.get(Self::band_gain(index)));
            let q = f64::from(p.get(Self::band_q(index)));
            self.wanted[band + 1] = gain_db != 0.0;
            flat[band + 1] = Coefficients::peaking(rate, frequency, q, 0.0);
            self.targets[band + 1] = if band == 0 && p.on(Self::BAND1_SHELF) {
                Coefficients::low_shelf(rate, frequency, q, gain_db)
            } else if band == BANDS - 1 && p.on(Self::BAND4_SHELF) {
                Coefficients::high_shelf(rate, frequency, q, gain_db)
            } else {
                Coefficients::peaking(rate, frequency, q, gain_db)
            };
        }
        self.wanted[last] = p.on(Self::LP_ON);
        self.targets[last] =
            Coefficients::low_pass(rate, f64::from(p.get(Self::LP_FREQ)), BUTTERWORTH_Q);

        if !self.primed {
            self.primed = true;
            self.coefficients = self.targets;
            self.active = self.wanted;
            self.settled = true;
            return;
        }
        for section in 0..SECTIONS {
            let band = section != 0 && section != last;
            match (self.active[section], self.wanted[section]) {
                // Switched in: bands glide from flat, the filters start at
                // once. The memory of an inactive section is always clear.
                (false, true) => {
                    self.active[section] = true;
                    self.coefficients[section] = if band {
                        flat[section]
                    } else {
                        self.targets[section]
                    };
                }
                // Switched out: a band glides to flat first, a filter stops
                // at once.
                (true, false) if !band => self.drop_section(section),
                _ => {}
            }
        }
        self.settled = false;
    }

    /// Stops processing a section and clears its memory, so nothing stale
    /// is left for the next time it is switched in.
    fn drop_section(&mut self, section: usize) {
        self.active[section] = false;
        for channel in &mut self.state {
            channel[section].reset();
        }
    }

    /// One sample of glide for every active section. Bands that have
    /// arrived at flat are dropped.
    #[inline]
    fn glide_step(&mut self) {
        let mut settled = true;
        for section in 0..SECTIONS {
            if self.active[section] {
                settled &= self.coefficients[section].glide_to(&self.targets[section], self.glide);
            }
        }
        if settled {
            self.settled = true;
            for section in 0..SECTIONS {
                if self.active[section] && !self.wanted[section] {
                    self.drop_section(section);
                }
            }
        }
    }
}

impl Device for ParametricEq {
    fn prepare(&mut self, config: &PrepareConfig) {
        self.sample_rate = config.sample_rate.as_f64();
        self.output.set_time(0.01, self.sample_rate as f32);
        self.glide = 1.0 - (-1.0 / (GLIDE_SECONDS * self.sample_rate)).exp();
        self.params.mark_dirty();
        self.reset();
    }

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        if self.params.take_dirty() {
            self.update();
        }
        let target = db_to_gain(self.params.get(Self::OUTPUT));
        process_stereo(io, |frame| {
            if !self.settled {
                self.glide_step();
            }
            let gain = self.output.next(target);
            let mut out = frame.input;
            for (sample, sections) in out.iter_mut().zip(self.state.iter_mut()) {
                let mut value = f64::from(*sample);
                for ((section, coefficients), active) in
                    sections.iter_mut().zip(&self.coefficients).zip(self.active)
                {
                    if active {
                        value = section.tick(coefficients, value);
                    }
                }
                *sample = value as f32 * gain;
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
        self.state = [[Biquad::default(); SECTIONS]; 2];
        self.output.reset();
        self.primed = false;
        self.params.mark_dirty();
    }
}
