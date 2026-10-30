//! Mix recording. The [`Recorder`] device sits after the mixer, passes
//! everything through and, while armed, copies the master channels into a
//! lock-free queue. The control side drains that queue into a WAV file
//! with [`RecorderHandle::pump`].

use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use fourfloor_audio::{SampleFormat, WavSpec, WavWriter};
use fourfloor_core::{Device, PrepareConfig, ProcessIo, SampleRate, TransportContext};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::error::DjError;

#[derive(Debug, Default)]
struct Shared {
    /// Set by the control side to record.
    armed: AtomicBool,
    /// What the audio thread last saw of `armed`.
    active: AtomicBool,
    /// Frames lost because the queue was full.
    dropped: AtomicU64,
}

/// The recording tap (see the [module docs](self)).
pub struct Recorder {
    queue: Producer<f32>,
    shared: Arc<Shared>,
}

/// An open recording.
struct Take {
    path: PathBuf,
    writer: WavWriter<BufWriter<File>>,
    scratch: Vec<f32>,
}

/// The control side of a [`Recorder`].
pub struct RecorderHandle {
    queue: Consumer<f32>,
    shared: Arc<Shared>,
    sample_rate: SampleRate,
    take: Option<Take>,
    stopping: bool,
}

/// What a finished recording holds.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordingSummary {
    /// The file written.
    pub path: PathBuf,
    /// Frames written.
    pub frames: u64,
    /// Frames lost because the control side did not drain fast enough.
    pub dropped_frames: u64,
}

impl Recorder {
    /// A recorder whose queue holds `seconds` of stereo audio at
    /// `sample_rate`, and its handle.
    pub fn new(sample_rate: SampleRate, seconds: f64) -> (Self, RecorderHandle) {
        let frames = (seconds.max(0.1) * sample_rate.as_f64()).ceil() as usize;
        let (producer, consumer) = RingBuffer::new(frames * 2);
        let shared = Arc::new(Shared::default());
        (
            Self {
                queue: producer,
                shared: Arc::clone(&shared),
            },
            RecorderHandle {
                queue: consumer,
                shared,
                sample_rate,
                take: None,
                stopping: false,
            },
        )
    }
}

impl Device for Recorder {
    fn prepare(&mut self, _config: &PrepareConfig) {}

    fn process(&mut self, _ctx: &TransportContext, io: &mut ProcessIo<'_, '_>) {
        io.pass_through();
        let armed = self.shared.armed.load(Ordering::Acquire);
        if armed && io.outputs.len() >= 2 {
            let mut dropped = 0u64;
            for index in 0..io.frames() {
                if self.queue.slots() < 2 {
                    dropped += 1;
                    continue;
                }
                // Two free slots were checked above, so neither push fails.
                let _ = self.queue.push(io.outputs[0][index]);
                let _ = self.queue.push(io.outputs[1][index]);
            }
            if dropped > 0 {
                self.shared.dropped.fetch_add(dropped, Ordering::Relaxed);
            }
        }
        self.shared.active.store(armed, Ordering::Release);
    }
}

impl RecorderHandle {
    /// Whether a recording is open (including one that is stopping).
    pub fn is_recording(&self) -> bool {
        self.take.is_some()
    }

    /// The file being recorded.
    pub fn path(&self) -> Option<&Path> {
        self.take.as_ref().map(|take| take.path.as_path())
    }

    /// Frames written to the file so far.
    pub fn frames_written(&self) -> u64 {
        self.take
            .as_ref()
            .map_or(0, |take| take.writer.frames_written())
    }

    /// Opens `path` (float WAV, stereo) and starts recording from the next
    /// block.
    pub fn start(&mut self, path: &Path) -> Result<(), DjError> {
        if self.take.is_some() {
            return Err(DjError::Recording("a recording is already running"));
        }
        // Anything left from an earlier take is stale.
        while self.queue.pop().is_ok() {}
        let spec = WavSpec {
            sample_rate: self.sample_rate,
            channels: 2,
            format: SampleFormat::Float32,
        };
        let writer = WavWriter::create(path, spec)?;
        self.take = Some(Take {
            path: path.to_path_buf(),
            writer,
            scratch: Vec::with_capacity(8192),
        });
        self.shared.dropped.store(0, Ordering::Relaxed);
        self.stopping = false;
        self.shared.armed.store(true, Ordering::Release);
        Ok(())
    }

    /// Asks the recorder to stop after the block in progress. The file is
    /// closed by a later [`pump`](Self::pump) once the audio thread has
    /// seen the request.
    pub fn stop(&mut self) -> Result<(), DjError> {
        if self.take.is_none() || self.stopping {
            return Err(DjError::Recording("no recording is running"));
        }
        self.shared.armed.store(false, Ordering::Release);
        self.stopping = true;
        Ok(())
    }

    /// Writes queued audio to the file. Returns the summary when this
    /// call closed a stopped recording.
    pub fn pump(&mut self) -> Result<Option<RecordingSummary>, DjError> {
        // Read before draining: once the audio thread has stopped, nothing
        // more arrives after this drain.
        let finished = self.stopping && !self.shared.active.load(Ordering::Acquire);
        let Some(take) = self.take.as_mut() else {
            while self.queue.pop().is_ok() {}
            return Ok(None);
        };
        loop {
            take.scratch.clear();
            while take.scratch.len() < take.scratch.capacity() {
                match self.queue.pop() {
                    Ok(sample) => take.scratch.push(sample),
                    Err(_) => break,
                }
            }
            if take.scratch.is_empty() {
                break;
            }
            // Keep whole frames; an odd sample waits for its partner.
            if take.scratch.len() % 2 == 1 {
                let partner = loop {
                    if let Ok(sample) = self.queue.pop() {
                        break sample;
                    }
                    std::hint::spin_loop();
                };
                take.scratch.push(partner);
            }
            take.writer.write_interleaved(&take.scratch)?;
        }
        if !finished {
            return Ok(None);
        }
        self.finish().map(Some)
    }

    fn finish(&mut self) -> Result<RecordingSummary, DjError> {
        let take = self
            .take
            .take()
            .ok_or(DjError::Recording("no recording is running"))?;
        self.stopping = false;
        let frames = take.writer.frames_written();
        take.writer.finalize()?;
        Ok(RecordingSummary {
            path: take.path,
            frames,
            dropped_frames: self.shared.dropped.load(Ordering::Relaxed),
        })
    }

    /// Stops and closes the recording now, without waiting for the audio
    /// thread: for an engine that is no longer running.
    pub fn stop_now(&mut self) -> Result<RecordingSummary, DjError> {
        self.shared.armed.store(false, Ordering::Release);
        self.shared.active.store(false, Ordering::Release);
        self.stopping = true;
        match self.pump()? {
            Some(summary) => Ok(summary),
            None => self.finish(),
        }
    }
}
