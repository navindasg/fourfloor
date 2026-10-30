//! WAV encoding and a small WAV/RF64 reader.
//!
//! [`WavWriter`] streams samples to any `Write + Seek` and fixes up the
//! header in [`finalize`](WavWriter::finalize). A classic RIFF/WAVE header
//! stores sizes in 32 bits, so it cannot describe more than 4 GiB. The
//! writer reserves room for a `ds64` chunk and, when the file grows past
//! that limit, finalizes it as RF64 (EBU Tech 3306) with 64-bit sizes
//! instead of writing a wrapped, corrupt size.
//!
//! [`read_wav`] reads plain PCM and float WAV and RF64. The decoder in
//! [`crate::decode`] uses it for RF64, which `symphonia` 0.6 does not read.

use std::fs::File;
use std::io::{self, BufWriter, Cursor, Read, Seek, SeekFrom, Write};
use std::path::Path;

use fourfloor_core::SampleRate;

use crate::buffer::AudioBuffer;

/// How samples are stored in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SampleFormat {
    /// 16-bit signed integer. Values outside -1.0..=1.0 are clipped; no
    /// dither is applied.
    Pcm16,
    /// 24-bit signed integer. Values outside -1.0..=1.0 are clipped.
    Pcm24,
    /// 32-bit IEEE float. Stored exactly; values above 1.0 are kept.
    Float32,
}

impl SampleFormat {
    /// Bytes one sample takes in the file.
    pub const fn bytes_per_sample(self) -> u16 {
        match self {
            Self::Pcm16 => 2,
            Self::Pcm24 => 3,
            Self::Float32 => 4,
        }
    }

    const fn format_tag(self) -> u16 {
        match self {
            Self::Pcm16 | Self::Pcm24 => FORMAT_PCM,
            Self::Float32 => FORMAT_FLOAT,
        }
    }
}

/// What a WAV file holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavSpec {
    /// Sample rate.
    pub sample_rate: SampleRate,
    /// Channel count, at least 1.
    pub channels: u16,
    /// Sample encoding.
    pub format: SampleFormat,
}

impl WavSpec {
    /// 32-bit float stereo: what `fourfloor render` writes.
    pub const fn float_stereo(sample_rate: SampleRate) -> Self {
        Self {
            sample_rate,
            channels: 2,
            format: SampleFormat::Float32,
        }
    }

    fn block_align(&self) -> u16 {
        self.channels * self.format.bytes_per_sample()
    }
}

/// Why a WAV file could not be written or read.
#[derive(Debug, thiserror::Error)]
pub enum WavError {
    /// The spec has zero channels.
    #[error("a WAV file needs at least one channel")]
    NoChannels,
    /// The channel count and sample size do not fit the header's fields.
    #[error("{channels} channels of {bytes}-byte samples do not fit a WAV header")]
    TooManyChannels {
        /// Channels asked for.
        channels: u16,
        /// Bytes per sample.
        bytes: u16,
    },
    /// The sample count is not a whole number of frames, or planar channels
    /// differ in length or number.
    #[error("the samples do not make whole frames for {channels} channels")]
    UnevenFrames {
        /// Channels the writer was created with.
        channels: u16,
    },
    /// The file is not a WAV file this reader understands.
    #[error("not a readable WAV file: {0}")]
    Malformed(&'static str),
    /// The file uses an encoding this reader does not handle.
    #[error("unsupported WAV encoding: format tag {tag}, {bits} bits")]
    Unsupported {
        /// The header's format tag.
        tag: u16,
        /// Bits per sample.
        bits: u16,
    },
    /// Reading or writing failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

const FORMAT_PCM: u16 = 1;
const FORMAT_FLOAT: u16 = 3;
const FORMAT_EXTENSIBLE: u16 = 0xFFFE;
/// Bytes before the first sample: RIFF header (12), reserved `JUNK`/`ds64`
/// chunk (8 + 28), `fmt ` chunk (8 + 16) and the `data` chunk header (8).
const HEADER_BYTES: u64 = 80;
const DS64_BODY_BYTES: u32 = 28;
/// The largest value the 32-bit RIFF size field can hold.
const RIFF_LIMIT: u64 = u32::MAX as u64;

/// Streams a WAV file.
///
/// Create it, write samples in any number of calls, then call
/// [`finalize`](WavWriter::finalize). A writer that is dropped without
/// being finalized leaves a file with a zero-length header.
#[derive(Debug)]
pub struct WavWriter<W: Write + Seek> {
    writer: W,
    spec: WavSpec,
    data_bytes: u64,
    /// File size above which the header is finalized as RF64.
    rf64_above: u64,
    scratch: Vec<u8>,
}

impl WavWriter<BufWriter<File>> {
    /// Creates (or truncates) the file at `path`.
    pub fn create(path: &Path, spec: WavSpec) -> Result<Self, WavError> {
        Self::new(BufWriter::new(File::create(path)?), spec)
    }
}

impl<W: Write + Seek> WavWriter<W> {
    /// Starts a WAV stream in `writer`, which must be at the position the
    /// file starts at.
    pub fn new(mut writer: W, spec: WavSpec) -> Result<Self, WavError> {
        if spec.channels == 0 {
            return Err(WavError::NoChannels);
        }
        let bytes = spec.format.bytes_per_sample();
        if spec.channels.checked_mul(bytes).is_none() {
            return Err(WavError::TooManyChannels {
                channels: spec.channels,
                bytes,
            });
        }
        let block_align = spec.block_align();
        let rate = spec.sample_rate.hz();

        writer.write_all(b"RIFF")?;
        writer.write_all(&0u32.to_le_bytes())?;
        writer.write_all(b"WAVE")?;
        // Room for a `ds64` chunk; readers skip `JUNK`.
        writer.write_all(b"JUNK")?;
        writer.write_all(&DS64_BODY_BYTES.to_le_bytes())?;
        writer.write_all(&[0u8; DS64_BODY_BYTES as usize])?;
        writer.write_all(b"fmt ")?;
        writer.write_all(&16u32.to_le_bytes())?;
        writer.write_all(&spec.format.format_tag().to_le_bytes())?;
        writer.write_all(&spec.channels.to_le_bytes())?;
        writer.write_all(&rate.to_le_bytes())?;
        writer.write_all(&rate.saturating_mul(u32::from(block_align)).to_le_bytes())?;
        writer.write_all(&block_align.to_le_bytes())?;
        writer.write_all(&(bytes * 8).to_le_bytes())?;
        writer.write_all(b"data")?;
        writer.write_all(&0u32.to_le_bytes())?;

        Ok(Self {
            writer,
            spec,
            data_bytes: 0,
            rf64_above: RIFF_LIMIT,
            scratch: Vec::new(),
        })
    }

    /// Finalizes as RF64 when the whole file is larger than `bytes`,
    /// instead of the real 4 GiB limit. For tests of the RF64 path; a
    /// normal caller never needs it.
    pub fn with_rf64_threshold(mut self, bytes: u64) -> Self {
        self.rf64_above = bytes.min(RIFF_LIMIT);
        self
    }

    /// The spec the writer was created with.
    pub fn spec(&self) -> WavSpec {
        self.spec
    }

    /// Frames written so far.
    pub fn frames_written(&self) -> u64 {
        self.data_bytes / u64::from(self.spec.block_align())
    }

    fn encode(&mut self, sample: f32) {
        match self.spec.format {
            SampleFormat::Pcm16 => {
                let value = (f64::from(sample.clamp(-1.0, 1.0)) * 32_767.0).round() as i16;
                self.scratch.extend_from_slice(&value.to_le_bytes());
            }
            SampleFormat::Pcm24 => {
                let value = (f64::from(sample.clamp(-1.0, 1.0)) * 8_388_607.0).round() as i32;
                self.scratch.extend_from_slice(&value.to_le_bytes()[..3]);
            }
            SampleFormat::Float32 => self.scratch.extend_from_slice(&sample.to_le_bytes()),
        }
    }

    fn flush_scratch(&mut self) -> Result<(), WavError> {
        self.writer.write_all(&self.scratch)?;
        self.data_bytes += self.scratch.len() as u64;
        self.scratch.clear();
        Ok(())
    }

    /// Appends interleaved samples: a whole number of frames.
    pub fn write_interleaved(&mut self, samples: &[f32]) -> Result<(), WavError> {
        let channels = usize::from(self.spec.channels);
        if !samples.len().is_multiple_of(channels) {
            return Err(WavError::UnevenFrames {
                channels: self.spec.channels,
            });
        }
        // Encode in pieces so a long write does not double its memory.
        for piece in samples.chunks(16_384 * channels) {
            for sample in piece {
                self.encode(if sample.is_nan() { 0.0 } else { *sample });
            }
            self.flush_scratch()?;
        }
        Ok(())
    }

    /// Appends planar samples: one slice per channel, all the same length.
    pub fn write_planar(&mut self, channels: &[&[f32]]) -> Result<(), WavError> {
        let uneven = WavError::UnevenFrames {
            channels: self.spec.channels,
        };
        if channels.len() != usize::from(self.spec.channels) {
            return Err(uneven);
        }
        let frames = channels.first().map_or(0, |channel| channel.len());
        if channels.iter().any(|channel| channel.len() != frames) {
            return Err(uneven);
        }
        for start in (0..frames).step_by(16_384) {
            for frame in start..(start + 16_384).min(frames) {
                for channel in channels {
                    let sample = channel[frame];
                    self.encode(if sample.is_nan() { 0.0 } else { sample });
                }
            }
            self.flush_scratch()?;
        }
        Ok(())
    }

    /// Writes the final sizes into the header and returns the writer.
    /// Files over 4 GiB become RF64.
    pub fn finalize(mut self) -> Result<W, WavError> {
        // Chunks are word-aligned: an odd data size gets one pad byte.
        let pad = self.data_bytes % 2;
        if pad == 1 {
            self.writer.write_all(&[0])?;
        }
        let riff_size = HEADER_BYTES - 8 + self.data_bytes + pad;
        let total = riff_size + 8;
        let frames = self.frames_written();

        self.writer.seek(SeekFrom::Start(0))?;
        if total > self.rf64_above {
            self.writer.write_all(b"RF64")?;
            self.writer.write_all(&u32::MAX.to_le_bytes())?;
            self.writer.write_all(b"WAVE")?;
            self.writer.write_all(b"ds64")?;
            self.writer.write_all(&DS64_BODY_BYTES.to_le_bytes())?;
            self.writer.write_all(&riff_size.to_le_bytes())?;
            self.writer.write_all(&self.data_bytes.to_le_bytes())?;
            self.writer.write_all(&frames.to_le_bytes())?;
            self.writer.write_all(&0u32.to_le_bytes())?; // no size table
            self.writer.seek(SeekFrom::Start(HEADER_BYTES - 4))?;
            self.writer.write_all(&u32::MAX.to_le_bytes())?;
        } else {
            // Both fit: `total` is at most `u32::MAX` here.
            self.writer.seek(SeekFrom::Start(4))?;
            self.writer.write_all(&(riff_size as u32).to_le_bytes())?;
            self.writer.seek(SeekFrom::Start(HEADER_BYTES - 4))?;
            self.writer
                .write_all(&(self.data_bytes as u32).to_le_bytes())?;
        }
        self.writer.seek(SeekFrom::End(0))?;
        self.writer.flush()?;
        Ok(self.writer)
    }
}

/// Encodes interleaved samples as a WAV file in memory.
pub fn encode_wav(spec: WavSpec, interleaved: &[f32]) -> Result<Vec<u8>, WavError> {
    let mut writer = WavWriter::new(Cursor::new(Vec::new()), spec)?;
    writer.write_interleaved(interleaved)?;
    Ok(writer.finalize()?.into_inner())
}

/// Writes interleaved samples to a WAV file at `path`.
pub fn write_wav_file(path: &Path, spec: WavSpec, interleaved: &[f32]) -> Result<(), WavError> {
    let mut writer = WavWriter::create(path, spec)?;
    writer.write_interleaved(interleaved)?;
    writer.finalize()?;
    Ok(())
}

/// Whether `bytes` start like an RF64 file.
pub fn is_rf64(bytes: &[u8]) -> bool {
    bytes.len() >= 12 && &bytes[..4] == b"RF64" && &bytes[8..12] == b"WAVE"
}

fn read_exact_or<R: Read>(
    reader: &mut R,
    buffer: &mut [u8],
    what: &'static str,
) -> Result<(), WavError> {
    reader.read_exact(buffer).map_err(|error| {
        if error.kind() == io::ErrorKind::UnexpectedEof {
            WavError::Malformed(what)
        } else {
            WavError::Io(error)
        }
    })
}

struct Format {
    tag: u16,
    channels: u16,
    rate: u32,
    bits: u16,
}

fn parse_format(body: &[u8]) -> Result<Format, WavError> {
    if body.len() < 16 {
        return Err(WavError::Malformed("the fmt chunk is too short"));
    }
    let u16_at = |at: usize| u16::from_le_bytes([body[at], body[at + 1]]);
    let mut tag = u16_at(0);
    if tag == FORMAT_EXTENSIBLE {
        // The real format is the first two bytes of the sub-format GUID.
        if body.len() < 26 {
            return Err(WavError::Malformed("the extensible fmt chunk is too short"));
        }
        tag = u16_at(24);
    }
    Ok(Format {
        tag,
        channels: u16_at(2),
        rate: u32::from_le_bytes([body[4], body[5], body[6], body[7]]),
        bits: u16_at(14),
    })
}

fn decode_sample(bytes: &[u8], format: &Format) -> Option<f32> {
    Some(match (format.tag, format.bits, bytes) {
        (FORMAT_PCM, 8, [a]) => (f32::from(*a) - 128.0) / 128.0,
        (FORMAT_PCM, 16, [a, b]) => f32::from(i16::from_le_bytes([*a, *b])) / 32_768.0,
        (FORMAT_PCM, 24, [a, b, c]) => {
            (i32::from_le_bytes([0, *a, *b, *c]) >> 8) as f32 / 8_388_608.0
        }
        (FORMAT_PCM, 32, [a, b, c, d]) => {
            (f64::from(i32::from_le_bytes([*a, *b, *c, *d])) / 2_147_483_648.0) as f32
        }
        (FORMAT_FLOAT, 32, [a, b, c, d]) => f32::from_le_bytes([*a, *b, *c, *d]),
        (FORMAT_FLOAT, 64, [a, b, c, d, e, f, g, h]) => {
            f64::from_le_bytes([*a, *b, *c, *d, *e, *f, *g, *h]) as f32
        }
        _ => return None,
    })
}

/// Reads a whole WAV or RF64 file: 8, 16, 24 or 32-bit integer PCM and 32
/// or 64-bit float, plain or `WAVE_FORMAT_EXTENSIBLE`.
pub fn read_wav<R: Read + Seek>(mut reader: R) -> Result<AudioBuffer, WavError> {
    let mut header = [0u8; 12];
    read_exact_or(
        &mut reader,
        &mut header,
        "the file is shorter than a header",
    )?;
    let rf64 = &header[..4] == b"RF64";
    if !(rf64 || &header[..4] == b"RIFF") || &header[8..12] != b"WAVE" {
        return Err(WavError::Malformed("no RIFF/WAVE or RF64/WAVE header"));
    }

    let mut format: Option<Format> = None;
    let mut data_bytes_64: Option<u64> = None;
    loop {
        let mut chunk = [0u8; 8];
        read_exact_or(&mut reader, &mut chunk, "no data chunk")?;
        let size = u32::from_le_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]);
        match &chunk[..4] {
            b"ds64" => {
                let mut body = vec![0u8; size as usize];
                read_exact_or(&mut reader, &mut body, "the ds64 chunk is cut short")?;
                if body.len() < 16 {
                    return Err(WavError::Malformed("the ds64 chunk is too short"));
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&body[8..16]);
                data_bytes_64 = Some(u64::from_le_bytes(bytes));
            }
            b"fmt " => {
                let mut body = vec![0u8; size as usize];
                read_exact_or(&mut reader, &mut body, "the fmt chunk is cut short")?;
                format = Some(parse_format(&body)?);
            }
            b"data" => {
                let format = format.ok_or(WavError::Malformed("data before fmt"))?;
                let length = match (size, data_bytes_64) {
                    (u32::MAX, Some(length)) if rf64 => length,
                    _ => u64::from(size),
                };
                return read_data(reader, &format, length);
            }
            _ => {
                reader.seek(SeekFrom::Current(i64::from(size)))?;
            }
        }
        if size % 2 == 1 && &chunk[..4] != b"data" {
            reader.seek(SeekFrom::Current(1))?;
        }
    }
}

fn read_data<R: Read>(
    mut reader: R,
    format: &Format,
    data_bytes: u64,
) -> Result<AudioBuffer, WavError> {
    let unsupported = WavError::Unsupported {
        tag: format.tag,
        bits: format.bits,
    };
    let sample_rate =
        SampleRate::new(format.rate).map_err(|_| WavError::Malformed("sample rate is zero"))?;
    if format.channels == 0 {
        return Err(WavError::NoChannels);
    }
    let width = usize::from(format.bits / 8);
    if width == 0 || decode_sample(&[0u8; 8][..width.min(8)], format).is_none() {
        return Err(unsupported);
    }
    let channels = usize::from(format.channels);
    let frame_bytes = width * channels;
    let mut planes: Vec<Vec<f32>> = vec![Vec::new(); channels];
    let mut block = vec![0u8; frame_bytes * 4_096];
    let mut left = data_bytes - data_bytes % frame_bytes as u64;
    while left > 0 {
        let want = (block.len() as u64).min(left) as usize;
        // A file cut short still yields the whole frames it holds.
        let mut got = 0;
        while got < want {
            match reader.read(&mut block[got..want])? {
                0 => break,
                n => got += n,
            }
        }
        for frame in block[..got].chunks_exact(frame_bytes) {
            for (plane, bytes) in planes.iter_mut().zip(frame.chunks_exact(width)) {
                plane.push(decode_sample(bytes, format).unwrap_or(0.0));
            }
        }
        if got < want {
            break;
        }
        left -= want as u64;
    }
    Ok(AudioBuffer {
        sample_rate,
        channels: planes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: SampleRate = SampleRate::HZ_48000;

    fn spec(channels: u16, format: SampleFormat) -> WavSpec {
        WavSpec {
            sample_rate: RATE,
            channels,
            format,
        }
    }

    fn ramp(samples: usize) -> Vec<f32> {
        (0..samples)
            .map(|i| (i as f32 / samples as f32) * 1.6 - 0.8)
            .collect()
    }

    fn u32_at(bytes: &[u8], at: usize) -> u32 {
        u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap())
    }

    fn u64_at(bytes: &[u8], at: usize) -> u64 {
        u64::from_le_bytes(bytes[at..at + 8].try_into().unwrap())
    }

    #[test]
    fn a_normal_file_has_a_riff_header_with_exact_sizes() {
        let samples = ramp(2_000);
        let bytes = encode_wav(WavSpec::float_stereo(RATE), &samples).unwrap();
        assert_eq!(bytes.len(), 80 + 8_000);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8);
        assert_eq!(&bytes[8..16], b"WAVEJUNK");
        assert_eq!(&bytes[48..52], b"fmt ");
        assert_eq!(u16::from_le_bytes([bytes[56], bytes[57]]), 3, "float");
        assert_eq!(u16::from_le_bytes([bytes[58], bytes[59]]), 2, "stereo");
        assert_eq!(u32_at(&bytes, 60), 48_000);
        assert_eq!(&bytes[72..76], b"data");
        assert_eq!(u32_at(&bytes, 76), 8_000);
        assert!(!is_rf64(&bytes));
    }

    #[test]
    fn float_round_trips_exactly_through_the_reader() {
        let samples = ramp(3_001 * 2);
        let bytes = encode_wav(WavSpec::float_stereo(RATE), &samples).unwrap();
        let audio = read_wav(Cursor::new(bytes)).unwrap();
        assert_eq!(audio.sample_rate, RATE);
        assert_eq!(audio.frames(), 3_001);
        assert_eq!(audio.interleaved(), samples);
    }

    #[test]
    fn integer_formats_round_trip_within_one_step_and_clip() {
        for (format, step) in [
            (SampleFormat::Pcm16, 1.0 / 32_767.0),
            (SampleFormat::Pcm24, 1.0 / 8_388_607.0),
        ] {
            let mut samples = ramp(999);
            samples.extend([2.0, -2.0, f32::NAN]);
            let bytes = encode_wav(spec(1, format), &samples).unwrap();
            let audio = read_wav(Cursor::new(bytes)).unwrap();
            let decoded = &audio.channels[0];
            assert_eq!(decoded.len(), 1_002);
            // The writer scales by 2^(n-1) - 1 and readers divide by
            // 2^(n-1), so allow for that on top of the rounding.
            let tolerance = 2.0 * step;
            for (a, b) in decoded.iter().zip(&samples[..999]) {
                assert!((a - b).abs() <= tolerance, "{format:?}: {a} vs {b}");
            }
            assert!((decoded[999] - 1.0).abs() <= tolerance, "clipped high");
            assert!((decoded[1_000] + 1.0).abs() <= tolerance, "clipped low");
            assert_eq!(decoded[1_001], 0.0, "NaN is written as silence");
        }
    }

    #[test]
    fn an_odd_data_size_is_padded_to_a_word() {
        // One mono 24-bit frame is 3 bytes.
        let bytes = encode_wav(spec(1, SampleFormat::Pcm24), &[0.5]).unwrap();
        assert_eq!(bytes.len(), 80 + 4);
        assert_eq!(u32_at(&bytes, 76), 3);
        assert_eq!(u32_at(&bytes, 4) as usize, bytes.len() - 8);
        assert_eq!(read_wav(Cursor::new(bytes)).unwrap().frames(), 1);
    }

    #[test]
    fn a_file_past_the_size_limit_is_finalized_as_rf64_and_reads_back() {
        // The limit is lowered so the test does not need 4 GiB of samples;
        // the header logic is the same.
        let samples = ramp(5_000 * 2);
        let mut writer = WavWriter::new(Cursor::new(Vec::new()), WavSpec::float_stereo(RATE))
            .unwrap()
            .with_rf64_threshold(10_000);
        writer.write_interleaved(&samples[..4_000]).unwrap();
        let (left, right): (Vec<f32>, Vec<f32>) = samples[4_000..]
            .chunks_exact(2)
            .map(|frame| (frame[0], frame[1]))
            .unzip();
        writer.write_planar(&[&left, &right]).unwrap();
        assert_eq!(writer.frames_written(), 5_000);
        assert_eq!(writer.spec().channels, 2);
        let bytes = writer.finalize().unwrap().into_inner();

        assert!(is_rf64(&bytes));
        assert_eq!(u32_at(&bytes, 4), u32::MAX, "32-bit size is a marker");
        assert_eq!(&bytes[12..16], b"ds64");
        assert_eq!(u32_at(&bytes, 16), 28);
        assert_eq!(u64_at(&bytes, 20) as usize, bytes.len() - 8, "RIFF size");
        assert_eq!(u64_at(&bytes, 28), 40_000, "data size");
        assert_eq!(u64_at(&bytes, 36), 5_000, "frame count");
        assert_eq!(u32_at(&bytes, 76), u32::MAX);

        let audio = read_wav(Cursor::new(bytes)).unwrap();
        assert_eq!(audio.frames(), 5_000);
        assert_eq!(audio.interleaved(), samples);
    }

    #[test]
    fn sizes_past_four_gigabytes_never_wrap() {
        /// Counts bytes instead of storing them.
        struct Sink {
            position: u64,
            length: u64,
            header: Vec<u8>,
        }
        impl Write for Sink {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                let start = self.position as usize;
                if start < 80 {
                    let end = (start + bytes.len()).min(80);
                    self.header.resize(self.header.len().max(end), 0);
                    self.header[start..end].copy_from_slice(&bytes[..end - start]);
                }
                self.position += bytes.len() as u64;
                self.length = self.length.max(self.position);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        impl Seek for Sink {
            fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
                self.position = match to {
                    SeekFrom::Start(at) => at,
                    SeekFrom::End(offset) => (self.length as i64 + offset) as u64,
                    SeekFrom::Current(offset) => (self.position as i64 + offset) as u64,
                };
                Ok(self.position)
            }
        }

        let sink = Sink {
            position: 0,
            length: 0,
            header: Vec::new(),
        };
        let mut writer = WavWriter::new(sink, WavSpec::float_stereo(RATE)).unwrap();
        // 4.5 GiB of float stereo: 603 979 776 frames, in 2 MiB pieces.
        let piece = vec![0.25f32; 1 << 19];
        let pieces = (9u64 << 29) / (4 * piece.len() as u64);
        for _ in 0..pieces {
            writer.write_interleaved(&piece).unwrap();
        }
        let data_bytes = 9u64 << 29;
        assert_eq!(writer.frames_written(), data_bytes / 8);
        let sink = writer.finalize().unwrap();
        assert_eq!(sink.length, 80 + data_bytes);
        assert!(is_rf64(&sink.header));
        assert_eq!(u64_at(&sink.header, 20), 72 + data_bytes);
        assert_eq!(u64_at(&sink.header, 28), data_bytes);
        assert_eq!(u64_at(&sink.header, 36), data_bytes / 8);
    }

    #[test]
    fn bad_specs_and_uneven_writes_are_typed_errors() {
        let none = WavWriter::new(Cursor::new(Vec::new()), spec(0, SampleFormat::Pcm16));
        assert!(matches!(none, Err(WavError::NoChannels)));
        let wide = WavWriter::new(Cursor::new(Vec::new()), spec(40_000, SampleFormat::Pcm16));
        assert!(matches!(wide, Err(WavError::TooManyChannels { .. })));

        let mut writer =
            WavWriter::new(Cursor::new(Vec::new()), WavSpec::float_stereo(RATE)).unwrap();
        let error = writer.write_interleaved(&[0.0; 3]).unwrap_err();
        assert!(matches!(error, WavError::UnevenFrames { channels: 2 }));
        assert!(writer.write_planar(&[&[0.0]]).is_err());
        assert!(writer.write_planar(&[&[0.0], &[0.0, 0.0]]).is_err());
        assert_eq!(writer.frames_written(), 0);
    }

    #[test]
    fn the_reader_rejects_garbage_and_reports_unknown_encodings() {
        assert!(matches!(
            read_wav(Cursor::new(b"nope".to_vec())),
            Err(WavError::Malformed(_))
        ));
        assert!(matches!(
            read_wav(Cursor::new(vec![0x42u8; 64])),
            Err(WavError::Malformed(_))
        ));
        let mut bytes = encode_wav(spec(1, SampleFormat::Pcm16), &[0.0; 4]).unwrap();
        bytes[56] = 7; // mu-law
        let error = read_wav(Cursor::new(bytes.clone())).unwrap_err();
        assert!(matches!(error, WavError::Unsupported { tag: 7, bits: 16 }));
        // Cut off inside the header: no data chunk.
        bytes.truncate(60);
        assert!(read_wav(Cursor::new(bytes)).is_err());
    }

    #[test]
    fn a_file_cut_short_yields_the_frames_it_holds() {
        let mut bytes = encode_wav(spec(1, SampleFormat::Pcm16), &ramp(100)).unwrap();
        bytes.truncate(80 + 51);
        assert_eq!(read_wav(Cursor::new(bytes)).unwrap().frames(), 25);
    }

    #[test]
    fn files_are_written_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("out.wav");
        let samples = ramp(480);
        write_wav_file(&path, spec(2, SampleFormat::Pcm24), &samples).unwrap();
        let audio = read_wav(File::open(&path).unwrap()).unwrap();
        assert_eq!(audio.frames(), 240);
        assert!(WavWriter::create(
            &dir.path().join("no/such/dir.wav"),
            spec(1, SampleFormat::Pcm16)
        )
        .is_err());
    }
}
