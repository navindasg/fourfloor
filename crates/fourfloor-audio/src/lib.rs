//! Audio files and measurement for fourfloor: decode, WAV encode,
//! resampling and loudness metering.
//!
//! - [`buffer`]: [`AudioBuffer`], decoded audio as planar `f32`.
//! - [`decode`]: [`decode_file`] and [`decode_bytes`] read WAV, AIFF, FLAC,
//!   MP3 and Ogg Vorbis through `symphonia`, and RF64 WAV through this
//!   crate's own reader. [`decode_file_at_rate`] also converts to the
//!   engine's sample rate.
//! - [`wav`]: [`WavWriter`] streams a WAV file to disk and switches to RF64
//!   when the data passes 4 GiB, so long renders never produce a corrupt
//!   header. [`encode_wav`] and [`write_wav_file`] are the one-call forms.
//! - [`resample`]: [`resample`], a windowed-sinc sample-rate converter.
//! - [`meter`]: [`measure`] and [`LoudnessMeter`], EBU R 128 loudness and
//!   true peak through `ebur128`.
//!
//! Nothing here is real-time safe: every function allocates and most do
//! I/O. Run them on a worker, never on the audio thread.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod buffer;
pub mod decode;
pub mod meter;
pub mod resample;
#[cfg(feature = "stretch")]
pub mod stretch;
pub mod wav;

pub use fourfloor_core as core;

pub use buffer::AudioBuffer;
pub use decode::{decode_bytes, decode_file, decode_file_at_rate, DecodeError};
pub use meter::{linear_to_db, measure, Loudness, LoudnessMeter, MeterError};
pub use resample::{resample, resample_buffer};
pub use wav::{encode_wav, write_wav_file, SampleFormat, WavError, WavSpec, WavWriter};
