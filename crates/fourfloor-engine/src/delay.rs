//! A fixed-length, preallocated multi-channel delay line.

/// Delays every channel by the same whole number of samples.
///
/// All memory is allocated in [`DelayLine::new`]; processing never allocates.
/// Channels are processed one at a time with [`run`](DelayLine::run), then
/// the shared write position is moved once per block with
/// [`advance`](DelayLine::advance).
#[derive(Clone, Debug)]
pub struct DelayLine {
    buf: Vec<f32>,
    len: usize,
    channels: usize,
    pos: usize,
}

impl DelayLine {
    /// Creates a silent delay of `len` samples for `channels` channels.
    /// A length of zero is a pass-through.
    pub fn new(channels: usize, len: usize) -> Self {
        Self {
            buf: vec![0.0; channels * len],
            len,
            channels,
            pos: 0,
        }
    }

    /// Delay length in samples.
    pub fn len(&self) -> usize {
        self.len
    }

    /// Whether this is a zero-length pass-through.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Number of channels.
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Feeds `src` into channel `channel` and writes the delayed signal to
    /// `dst`, adding to what is there when `add` is true and overwriting
    /// otherwise. Channels past [`channels`](DelayLine::channels) are
    /// passed through undelayed.
    pub fn run(&mut self, channel: usize, src: &[f32], dst: &mut [f32], add: bool) {
        if self.len == 0 || channel >= self.channels {
            for (s, d) in src.iter().zip(dst.iter_mut()) {
                *d = if add { *d + *s } else { *s };
            }
            return;
        }
        let line = &mut self.buf[channel * self.len..(channel + 1) * self.len];
        let mut pos = self.pos;
        for (s, d) in src.iter().zip(dst.iter_mut()) {
            let delayed = line[pos];
            line[pos] = *s;
            *d = if add { *d + delayed } else { delayed };
            pos += 1;
            if pos == self.len {
                pos = 0;
            }
        }
    }

    /// Moves the write position on by `frames`. Call once per block after
    /// every channel has been [`run`](DelayLine::run).
    pub fn advance(&mut self, frames: usize) {
        if self.len > 0 {
            self.pos = (self.pos + frames % self.len) % self.len;
        }
    }

    /// Clears the stored audio without reallocating.
    pub fn clear(&mut self) {
        self.buf.fill(0.0);
        self.pos = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delays_by_exact_length_across_blocks() {
        let mut d = DelayLine::new(1, 3);
        let input: Vec<f32> = (1..=10).map(|x| x as f32).collect();
        let mut out = vec![0.0; 10];
        for (i, o) in input.chunks(4).zip(out.chunks_mut(4)) {
            d.run(0, i, o, false);
            d.advance(i.len());
        }
        assert_eq!(out, [0.0, 0.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn zero_length_passes_through_and_add_sums() {
        let mut d = DelayLine::new(2, 0);
        let mut out = [1.0, 1.0];
        d.run(1, &[0.5, 0.25], &mut out, true);
        d.advance(2);
        assert_eq!(out, [1.5, 1.25]);
        assert!(d.is_empty());
    }

    #[test]
    fn clear_silences_tail() {
        let mut d = DelayLine::new(1, 2);
        let mut out = [0.0; 2];
        d.run(0, &[1.0, 1.0], &mut out, false);
        d.advance(2);
        d.clear();
        d.run(0, &[0.0, 0.0], &mut out, false);
        assert_eq!(out, [0.0, 0.0]);
    }
}
