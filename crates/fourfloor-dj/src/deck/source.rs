//! Reading samples out of a track: plain, interpolated, with stem gains.

use crate::track::{DeckTrack, MAX_STEMS};

#[inline]
fn at(channel: &[f32], index: i64) -> f32 {
    if index < 0 {
        0.0
    } else {
        channel.get(index as usize).copied().unwrap_or(0.0)
    }
}

/// Four-point Catmull-Rom interpolation. Exact at whole positions.
#[inline]
fn hermite(channel: &[f32], index: i64, t: f32) -> f32 {
    let y1 = at(channel, index);
    if t == 0.0 {
        return y1;
    }
    let (y0, y2, y3) = (
        at(channel, index - 1),
        at(channel, index + 1),
        at(channel, index + 2),
    );
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

/// The stereo frame at a whole position: the mix, or the gain-weighted sum
/// of the stems when the track has any. Silence outside the track.
#[cfg(feature = "stretch")]
#[inline]
pub(crate) fn frame(track: &DeckTrack, index: i64, gains: &[f32; MAX_STEMS]) -> (f32, f32) {
    let stems = track.stems();
    if stems.is_empty() {
        return (at(track.left(), index), at(track.right(), index));
    }
    let (mut left, mut right) = (0.0, 0.0);
    for (stem, &gain) in stems.iter().zip(gains) {
        left += at(&stem.left, index) * gain;
        right += at(&stem.right, index) * gain;
    }
    (left, right)
}

/// The stereo frame at a fractional position.
#[inline]
pub(crate) fn interpolated(
    track: &DeckTrack,
    position: f64,
    gains: &[f32; MAX_STEMS],
) -> (f32, f32) {
    let floor = position.floor();
    let (index, t) = (floor as i64, (position - floor) as f32);
    let stems = track.stems();
    if stems.is_empty() {
        return (
            hermite(track.left(), index, t),
            hermite(track.right(), index, t),
        );
    }
    let (mut left, mut right) = (0.0, 0.0);
    for (stem, &gain) in stems.iter().zip(gains) {
        left += hermite(&stem.left, index, t) * gain;
        right += hermite(&stem.right, index, t) * gain;
    }
    (left, right)
}

/// Mono sum of the mix at a whole position, for waveform matching.
#[inline]
pub(crate) fn mono(track: &DeckTrack, index: i64) -> f32 {
    at(track.left(), index) + at(track.right(), index)
}
