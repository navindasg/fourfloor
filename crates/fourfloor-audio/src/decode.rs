//! Audio-file decoding on pure-Rust `symphonia` 0.6.
//!
//! Decoding is an offline operation: it allocates and does I/O, and must
//! never run on the audio thread. The whole file is decoded into memory.

use std::io::Cursor;
use std::path::Path;

use fourfloor_core::SampleRate;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream};
use symphonia::core::meta::MetadataOptions;

use crate::buffer::AudioBuffer;
use crate::resample::resample_buffer;
use crate::wav::{is_rf64, read_wav, WavError};

/// Why decoding failed.
#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    /// The file could not be opened or read.
    #[error("cannot read the file: {0}")]
    Io(#[from] std::io::Error),
    /// The container has no audio track.
    #[error("no audio track found")]
    NoAudioTrack,
    /// The stream decoded to nothing, or reported no usable sample rate.
    #[error("stream contains no decodable audio")]
    Empty,
    /// The stream changed sample rate or channel count part-way through.
    #[error("sample rate or channel count changed")]
    FormatChanged,
    /// Symphonia could not probe, demux or decode the stream.
    #[error("symphonia error: {0}")]
    Backend(#[from] SymphoniaError),
    /// The RF64 reader rejected the file.
    #[error("{0}")]
    Wav(#[from] WavError),
}

/// Decodes a whole in-memory file. `extension` (for example `"wav"`) is an
/// optional hint for the format probe.
pub fn decode_bytes(bytes: Vec<u8>, extension: Option<&str>) -> Result<AudioBuffer, DecodeError> {
    if is_rf64(&bytes) {
        return non_empty(read_wav(Cursor::new(bytes))?);
    }
    decode_source(Box::new(Cursor::new(bytes)), extension)
}

/// Decodes a whole file from disk, using its extension as the probe hint.
pub fn decode_file(path: &Path) -> Result<AudioBuffer, DecodeError> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file = std::fs::File::open(path)?;
    let mut magic = [0u8; 12];
    let read = file.read(&mut magic)?;
    file.seek(SeekFrom::Start(0))?;
    if is_rf64(&magic[..read]) {
        return non_empty(read_wav(std::io::BufReader::new(file))?);
    }
    let extension = path.extension().and_then(|e| e.to_str());
    decode_source(Box::new(file), extension)
}

/// Decodes a file and converts it to `sample_rate` (the engine's rate) if
/// it was recorded at another.
pub fn decode_file_at_rate(
    path: &Path,
    sample_rate: SampleRate,
) -> Result<AudioBuffer, DecodeError> {
    let audio = decode_file(path)?;
    Ok(if audio.sample_rate == sample_rate {
        audio
    } else {
        resample_buffer(&audio, sample_rate)
    })
}

fn non_empty(audio: AudioBuffer) -> Result<AudioBuffer, DecodeError> {
    if audio.frames() == 0 {
        Err(DecodeError::Empty)
    } else {
        Ok(audio)
    }
}

fn decode_source(
    source: Box<dyn MediaSource>,
    extension: Option<&str>,
) -> Result<AudioBuffer, DecodeError> {
    let stream = MediaSourceStream::new(source, Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = extension {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe().probe(
        &hint,
        stream,
        FormatOptions::default(),
        MetadataOptions::default(),
    )?;

    let track = format
        .default_track(TrackType::Audio)
        .ok_or(DecodeError::NoAudioTrack)?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .ok_or(DecodeError::NoAudioTrack)?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(params, &AudioDecoderOptions::default())?;

    let mut rate_hz: Option<u32> = None;
    let mut channels: Vec<Vec<f32>> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();

    while let Some(packet) = format.next_packet()? {
        if packet.track_id != track_id {
            continue;
        }
        let buffer = match decoder.decode(&packet) {
            Ok(buffer) => buffer,
            // A corrupt packet is skippable; anything else is fatal.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(e) => return Err(e.into()),
        };
        let packet_rate = buffer.spec().rate();
        let packet_channels = buffer.num_planes();
        match rate_hz {
            None => {
                rate_hz = Some(packet_rate);
                channels = vec![Vec::new(); packet_channels];
            }
            Some(rate) if rate != packet_rate || channels.len() != packet_channels => {
                return Err(DecodeError::FormatChanged);
            }
            Some(_) => {}
        }
        buffer.copy_to_vec_interleaved(&mut scratch);
        if packet_channels > 0 {
            for frame in scratch.chunks_exact(packet_channels) {
                for (channel, &sample) in channels.iter_mut().zip(frame) {
                    channel.push(sample);
                }
            }
        }
    }

    let sample_rate = rate_hz
        .and_then(|hz| SampleRate::new(hz).ok())
        .ok_or(DecodeError::Empty)?;
    non_empty(AudioBuffer {
        sample_rate,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wav::{encode_wav, write_wav_file, SampleFormat, WavSpec, WavWriter};

    fn sine(freq: f64, rate: SampleRate, frames: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / rate.as_f64()).sin()) as f32
            })
            .collect()
    }

    fn mono(rate: SampleRate, format: SampleFormat) -> WavSpec {
        WavSpec {
            sample_rate: rate,
            channels: 1,
            format,
        }
    }

    #[test]
    fn float_wav_from_the_writer_decodes_exactly() {
        let rate = SampleRate::HZ_48000;
        let left = sine(440.0, rate, 10_000);
        let right = sine(880.0, rate, 10_000);
        let interleaved: Vec<f32> = left
            .iter()
            .zip(&right)
            .flat_map(|(l, r)| [*l, *r])
            .collect();
        let wav = encode_wav(WavSpec::float_stereo(rate), &interleaved).unwrap();

        let decoded = decode_bytes(wav, Some("wav")).unwrap();
        assert_eq!(decoded.sample_rate, rate);
        assert_eq!(decoded.channel_count(), 2);
        assert_eq!(decoded.frames(), 10_000);
        assert_eq!(decoded.channels[0], left);
        assert_eq!(decoded.channels[1], right);
    }

    #[test]
    fn integer_wavs_decode_to_scaled_floats_without_a_hint() {
        let rate = SampleRate::HZ_44100;
        let source = sine(1000.0, rate, 4_410);
        for (format, tolerance) in [
            (SampleFormat::Pcm16, 1.0 / 16_384.0),
            (SampleFormat::Pcm24, 1.0 / 4_194_304.0),
        ] {
            let wav = encode_wav(mono(rate, format), &source).unwrap();
            let decoded = decode_bytes(wav, None).unwrap();
            assert_eq!(decoded.sample_rate, rate);
            assert_eq!(decoded.channel_count(), 1);
            assert_eq!(decoded.frames(), source.len());
            let worst = decoded.channels[0]
                .iter()
                .zip(&source)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert!(worst < tolerance, "{format:?}: worst error {worst}");
        }
    }

    #[test]
    fn files_on_disk_decode_and_convert_to_the_engine_rate() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        let rate = SampleRate::HZ_44100;
        let source = sine(220.0, rate, 44_100);
        write_wav_file(&path, mono(rate, SampleFormat::Float32), &source).unwrap();

        let decoded = decode_file(&path).unwrap();
        assert_eq!(decoded.channels[0], source);

        let same = decode_file_at_rate(&path, rate).unwrap();
        assert_eq!(same, decoded);
        let converted = decode_file_at_rate(&path, SampleRate::HZ_48000).unwrap();
        assert_eq!(converted.sample_rate, SampleRate::HZ_48000);
        assert_eq!(converted.frames(), 48_000, "one second stays one second");
        // Still 220 Hz: 220 upward zero crossings in the second (the one at
        // the very first sample may fall either way).
        let crossings = converted.channels[0]
            .windows(2)
            .filter(|w| w[0] <= 0.0 && w[1] > 0.0)
            .count();
        assert!((219..=220).contains(&crossings), "{crossings} cycles");
    }

    #[test]
    fn rf64_files_decode_through_the_built_in_reader() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("long.wav");
        let rate = SampleRate::HZ_48000;
        let source = sine(330.0, rate, 4_000);
        let mut writer = WavWriter::create(&path, mono(rate, SampleFormat::Float32))
            .unwrap()
            .with_rf64_threshold(1_000);
        writer.write_interleaved(&source).unwrap();
        writer.finalize().unwrap();

        let bytes = std::fs::read(&path).unwrap();
        assert!(is_rf64(&bytes));
        assert_eq!(decode_file(&path).unwrap().channels[0], source);
        assert_eq!(decode_bytes(bytes, None).unwrap().channels[0], source);
    }

    #[test]
    fn garbage_missing_and_empty_files_are_errors_not_panics() {
        assert!(decode_bytes(vec![0x42; 4096], None).is_err());
        assert!(decode_bytes(Vec::new(), Some("wav")).is_err());
        let missing = decode_file(Path::new("/definitely/not/here.wav")).unwrap_err();
        assert!(matches!(missing, DecodeError::Io(_)), "{missing}");

        let rate = SampleRate::HZ_48000;
        let empty = encode_wav(WavSpec::float_stereo(rate), &[]).unwrap();
        assert!(decode_bytes(empty, Some("wav")).is_err());
        let mut writer = WavWriter::new(Cursor::new(Vec::new()), WavSpec::float_stereo(rate))
            .unwrap()
            .with_rf64_threshold(0);
        writer.write_interleaved(&[]).unwrap();
        let empty_rf64 = writer.finalize().unwrap().into_inner();
        assert!(matches!(
            decode_bytes(empty_rf64, None),
            Err(DecodeError::Empty)
        ));
    }
}
