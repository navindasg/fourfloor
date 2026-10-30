//! Helpers shared by every effect: parameter storage, smoothing and the
//! stereo block driver.

use fourfloor_core::{ParamId, ParamInfo, ProcessIo};

/// Decibels to a linear gain factor.
#[inline]
pub(crate) fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db * 0.05)
}

/// One-pole coefficient for a time constant of `seconds`: the share of the
/// distance to the target covered each sample.
#[inline]
pub(crate) fn time_coefficient(seconds: f32, sample_rate: f32) -> f32 {
    let samples = seconds * sample_rate;
    if samples <= 1.0e-6 {
        1.0
    } else {
        1.0 - (-1.0 / samples).exp()
    }
}

/// The values of a device's parameters, indexed by [`ParamId`]. A device's
/// ids are the positions in its descriptor table.
#[derive(Clone, Debug)]
pub(crate) struct ParamSet<const N: usize> {
    table: &'static [ParamInfo; N],
    values: [f32; N],
    dirty: bool,
}

impl<const N: usize> ParamSet<N> {
    /// Every parameter at its default. Starts dirty so the first block
    /// computes coefficients.
    pub(crate) fn new(table: &'static [ParamInfo; N]) -> Self {
        let mut values = [0.0; N];
        for (value, info) in values.iter_mut().zip(table.iter()) {
            *value = info.default;
        }
        Self {
            table,
            values,
            dirty: true,
        }
    }

    pub(crate) fn table(&self) -> &'static [ParamInfo] {
        self.table
    }

    /// Stores a clamped value. Unknown ids and non-finite values are
    /// ignored.
    pub(crate) fn set(&mut self, id: ParamId, value: f32) {
        if !value.is_finite() {
            return;
        }
        let index = id.0 as usize;
        if let (Some(slot), Some(info)) = (self.values.get_mut(index), self.table.get(index)) {
            let clamped = info.clamp(value);
            if *slot != clamped {
                *slot = clamped;
                self.dirty = true;
            }
        }
    }

    #[inline]
    pub(crate) fn get(&self, id: ParamId) -> f32 {
        self.values.get(id.0 as usize).copied().unwrap_or(0.0)
    }

    /// A toggle parameter.
    #[inline]
    pub(crate) fn on(&self, id: ParamId) -> bool {
        self.get(id) >= 0.5
    }

    /// A choice parameter, rounded to the nearest index.
    #[inline]
    pub(crate) fn choice(&self, id: ParamId) -> usize {
        self.get(id).round().max(0.0) as usize
    }

    /// Whether a value changed since the last call.
    pub(crate) fn take_dirty(&mut self) -> bool {
        std::mem::replace(&mut self.dirty, false)
    }

    pub(crate) fn mark_dirty(&mut self) {
        self.dirty = true;
    }
}

/// A value that glides to its target with a one-pole, so parameter changes
/// do not click. The first value after a reset is taken at once.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Smoothed {
    value: f32,
    coefficient: f32,
    primed: bool,
}

impl Smoothed {
    pub(crate) const fn new() -> Self {
        Self {
            value: 0.0,
            coefficient: 1.0,
            primed: false,
        }
    }

    pub(crate) fn set_time(&mut self, seconds: f32, sample_rate: f32) {
        self.coefficient = time_coefficient(seconds, sample_rate);
    }

    pub(crate) fn set_coefficient(&mut self, coefficient: f32) {
        self.coefficient = coefficient.clamp(0.0, 1.0);
    }

    pub(crate) fn reset(&mut self) {
        self.primed = false;
    }

    #[inline]
    pub(crate) fn next(&mut self, target: f32) -> f32 {
        if !self.primed {
            self.value = target;
            self.primed = true;
        } else if self.value != target {
            self.value += (target - self.value) * self.coefficient;
            if (target - self.value).abs() < 1.0e-6 {
                self.value = target;
            }
        }
        self.value
    }
}

/// A one-pole low-pass; `high` gives the complementary high-pass.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct OnePole {
    coefficient: f32,
    state: f32,
}

impl OnePole {
    pub(crate) fn set_cutoff(&mut self, hz: f32, sample_rate: f32) {
        let hz = hz.clamp(1.0, sample_rate * 0.49);
        self.coefficient = 1.0 - (-std::f32::consts::TAU * hz / sample_rate).exp();
    }

    #[inline]
    pub(crate) fn low(&mut self, input: f32) -> f32 {
        self.state += self.coefficient * (input - self.state);
        self.state
    }

    #[inline]
    pub(crate) fn high(&mut self, input: f32) -> f32 {
        input - self.low(input)
    }

    pub(crate) fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// One frame handed to the closure of [`process_stereo`].
#[derive(Clone, Copy, Debug)]
pub(crate) struct Frame {
    /// Left and right input. Zero when nothing is connected.
    pub input: [f32; 2],
    /// Left and right sidechain. Zero when nothing is connected.
    pub key: [f32; 2],
}

#[inline]
fn sample(channel: Option<&[f32]>, index: usize) -> f32 {
    match channel.and_then(|samples| samples.get(index)) {
        Some(value) if value.is_finite() => *value,
        _ => 0.0,
    }
}

/// Runs `tick` once per frame of the block and writes what it returns.
///
/// Effects are written for stereo. This adapts the other cases so no effect
/// has to: a missing input reads as silence, a mono bus feeds both sides and
/// receives the average of the two results, and channels past the second
/// are silenced. Input samples that are not finite read as zero, so a
/// broken upstream device cannot poison filter or delay state for good.
pub(crate) fn process_stereo(io: &mut ProcessIo<'_, '_>, mut tick: impl FnMut(Frame) -> [f32; 2]) {
    let in_left = io.inputs.first().copied();
    let in_right = io.inputs.get(1).copied().or(in_left);
    let key_left = io.sidechain.and_then(|key| key.first().copied());
    let key_right = io
        .sidechain
        .and_then(|key| key.get(1).copied())
        .or(key_left);
    let frame = |index: usize| Frame {
        input: [sample(in_left, index), sample(in_right, index)],
        key: [sample(key_left, index), sample(key_right, index)],
    };
    match &mut *io.outputs {
        [] => {}
        [mono] => {
            for (index, out) in mono.iter_mut().enumerate() {
                let [left, right] = tick(frame(index));
                *out = 0.5 * (left + right);
            }
        }
        [left, right, rest @ ..] => {
            for (index, (out_left, out_right)) in left.iter_mut().zip(right.iter_mut()).enumerate()
            {
                let [l, r] = tick(frame(index));
                *out_left = l;
                *out_right = r;
            }
            for channel in rest {
                channel.fill(0.0);
            }
        }
    }
}
