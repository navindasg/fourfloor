//! Real MIDI ports through `midir` (feature `midi`): controller input and
//! clock output.
//!
//! Nothing here runs on the audio thread. Input arrives on a thread the
//! OS MIDI layer owns; the callback should hand messages to the engine
//! through `EngineController` (behind the caller's own channel or lock).
//! Clock goes out from a small timer thread, so its jitter is whatever
//! the OS scheduler gives a sleeping thread (around a millisecond); it is
//! not sample-locked to the audio callback.

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use fourfloor_core::Tempo;
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use super::clock::{CLOCK, CONTINUE, PULSES_PER_BEAT, START, STOP};

const CLIENT_NAME: &str = "fourfloor";

/// Why a MIDI port could not be used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiPortError {
    /// The OS MIDI layer could not be reached.
    Unavailable(String),
    /// No port has a name containing the text asked for.
    NoSuchPort(String),
    /// The port exists but could not be opened.
    Connect(String),
    /// A message could not be sent.
    Send(String),
}

impl fmt::Display for MidiPortError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(reason) => write!(f, "MIDI is not available: {reason}"),
            Self::NoSuchPort(name) => write!(f, "no MIDI port matches \"{name}\""),
            Self::Connect(reason) => write!(f, "cannot open the MIDI port: {reason}"),
            Self::Send(reason) => write!(f, "cannot send MIDI: {reason}"),
        }
    }
}

impl std::error::Error for MidiPortError {}

fn input() -> Result<MidiInput, MidiPortError> {
    let mut input = MidiInput::new(CLIENT_NAME)
        .map_err(|error| MidiPortError::Unavailable(error.to_string()))?;
    // Keep clock and transport bytes; drop sysex and active sensing.
    input.ignore(Ignore::SysexAndActiveSense);
    Ok(input)
}

fn output() -> Result<MidiOutput, MidiPortError> {
    MidiOutput::new(CLIENT_NAME).map_err(|error| MidiPortError::Unavailable(error.to_string()))
}

/// Names of the MIDI input ports present right now.
pub fn input_names() -> Result<Vec<String>, MidiPortError> {
    let input = input()?;
    Ok(input
        .ports()
        .iter()
        .filter_map(|port| input.port_name(port).ok())
        .collect())
}

/// Names of the MIDI output ports present right now.
pub fn output_names() -> Result<Vec<String>, MidiPortError> {
    let output = output()?;
    Ok(output
        .ports()
        .iter()
        .filter_map(|port| output.port_name(port).ok())
        .collect())
}

/// An open MIDI input. Messages stop when it is dropped.
pub struct MidiIn {
    _connection: MidiInputConnection<()>,
    name: String,
}

impl fmt::Debug for MidiIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MidiIn").field("name", &self.name).finish()
    }
}

impl MidiIn {
    /// Opens the first input port whose name contains `name` (an empty
    /// string takes the first port). `on_message` gets each message's
    /// timestamp in microseconds and its bytes, on the OS MIDI thread.
    pub fn open(
        name: &str,
        mut on_message: impl FnMut(u64, &[u8]) + Send + 'static,
    ) -> Result<Self, MidiPortError> {
        let input = input()?;
        let (port, port_name) = input
            .ports()
            .into_iter()
            .filter_map(|port| input.port_name(&port).ok().map(|name| (port, name)))
            .find(|(_, port_name)| port_name.contains(name))
            .ok_or_else(|| MidiPortError::NoSuchPort(name.to_owned()))?;
        let connection = input
            .connect(
                &port,
                "fourfloor-in",
                move |stamp, bytes, _| on_message(stamp, bytes),
                (),
            )
            .map_err(|error| MidiPortError::Connect(error.to_string()))?;
        Ok(Self {
            _connection: connection,
            name: port_name,
        })
    }

    /// The full name of the port that was opened.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// An open MIDI output.
pub struct MidiOut {
    connection: MidiOutputConnection,
    name: String,
}

impl fmt::Debug for MidiOut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MidiOut").field("name", &self.name).finish()
    }
}

impl MidiOut {
    /// Opens the first output port whose name contains `name` (an empty
    /// string takes the first port).
    pub fn open(name: &str) -> Result<Self, MidiPortError> {
        let output = output()?;
        let (port, port_name) = output
            .ports()
            .into_iter()
            .filter_map(|port| output.port_name(&port).ok().map(|name| (port, name)))
            .find(|(_, port_name)| port_name.contains(name))
            .ok_or_else(|| MidiPortError::NoSuchPort(name.to_owned()))?;
        let connection = output
            .connect(&port, "fourfloor-out")
            .map_err(|error| MidiPortError::Connect(error.to_string()))?;
        Ok(Self {
            connection,
            name: port_name,
        })
    }

    /// The full name of the port that was opened.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Sends one complete MIDI message.
    pub fn send(&mut self, bytes: &[u8]) -> Result<(), MidiPortError> {
        self.connection
            .send(bytes)
            .map_err(|error| MidiPortError::Send(error.to_string()))
    }
}

struct ClockShared {
    quit: AtomicBool,
    playing: AtomicBool,
    /// Whether the next start is a continue (playback resumes mid-song).
    resume: AtomicBool,
    tempo_bits: AtomicU64,
}

/// Sends MIDI clock to an output port from its own thread: 24 pulses per
/// beat at the current tempo while playing, with start, continue and stop.
///
/// Tell it what the transport does with [`set_playing`](Self::set_playing)
/// and [`set_tempo`](Self::set_tempo). Dropping it stops the clock and
/// closes the port.
pub struct ClockSender {
    shared: Arc<ClockShared>,
    thread: Option<JoinHandle<()>>,
}

impl fmt::Debug for ClockSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClockSender")
            .field("playing", &self.shared.playing.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

impl ClockSender {
    /// Starts the clock thread on `out`, stopped, at `tempo`.
    pub fn spawn(mut out: MidiOut, tempo: Tempo) -> Self {
        let shared = Arc::new(ClockShared {
            quit: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            resume: AtomicBool::new(false),
            tempo_bits: AtomicU64::new(tempo.bpm().to_bits()),
        });
        let state = Arc::clone(&shared);
        let thread = std::thread::spawn(move || {
            let mut running = false;
            let mut next = Instant::now();
            while !state.quit.load(Ordering::Relaxed) {
                let playing = state.playing.load(Ordering::Relaxed);
                if playing != running {
                    let byte = match (playing, state.resume.load(Ordering::Relaxed)) {
                        (true, false) => START,
                        (true, true) => CONTINUE,
                        (false, _) => STOP,
                    };
                    // A failed send means the port went away; keep the
                    // thread alive so the owner can still shut it down.
                    let _ = out.send(&[byte]);
                    running = playing;
                    next = Instant::now();
                }
                if !running {
                    std::thread::sleep(Duration::from_millis(1));
                    continue;
                }
                let now = Instant::now();
                if now >= next {
                    let _ = out.send(&[CLOCK]);
                    let bpm = f64::from_bits(state.tempo_bits.load(Ordering::Relaxed));
                    let pulse = Duration::from_secs_f64(60.0 / bpm / PULSES_PER_BEAT as f64);
                    next += pulse;
                    if next < now {
                        next = now + pulse; // Fell behind: do not burst.
                    }
                } else {
                    std::thread::sleep((next - now).min(Duration::from_millis(1)));
                }
            }
            if running {
                let _ = out.send(&[STOP]);
            }
        });
        Self {
            shared,
            thread: Some(thread),
        }
    }

    /// Follows the transport: `true` sends start (or continue, when
    /// `resume` is set) and begins the pulses; `false` sends stop.
    pub fn set_playing(&self, playing: bool, resume: bool) {
        self.shared.resume.store(resume, Ordering::Relaxed);
        self.shared.playing.store(playing, Ordering::Relaxed);
    }

    /// Changes the pulse rate.
    pub fn set_tempo(&self, tempo: Tempo) {
        self.shared
            .tempo_bits
            .store(tempo.bpm().to_bits(), Ordering::Relaxed);
    }
}

impl Drop for ClockSender {
    fn drop(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            // A panicked clock thread has nothing left to clean up.
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::midi::{parse, MidiMessage};
    use std::sync::Mutex;

    #[test]
    fn a_missing_port_is_reported_by_name() {
        // Listing may fail on a machine with no MIDI service; opening a
        // port that cannot exist must fail either way.
        let name = "no-such-fourfloor-port-5c1f";
        assert!(MidiOut::open(name).is_err());
        assert!(MidiIn::open(name, |_, _| {}).is_err());
        let error = MidiPortError::NoSuchPort(name.to_owned());
        assert!(error.to_string().contains(name));
    }

    /// Needs the OS MIDI service (CoreMIDI or ALSA): opens a virtual input
    /// port, sends clock to it and counts what arrives.
    #[cfg(unix)]
    #[test]
    #[ignore = "needs the OS MIDI service"]
    fn clock_reaches_a_virtual_port_at_the_tempo() {
        use midir::os::unix::VirtualInput;

        let received: Arc<Mutex<Vec<u8>>> = Arc::default();
        let sink = Arc::clone(&received);
        let port = "fourfloor-clock-test";
        let _virtual_in = input()
            .unwrap()
            .create_virtual(
                port,
                move |_, bytes, _| sink.lock().unwrap().extend_from_slice(bytes),
                (),
            )
            .unwrap();
        assert!(output_names()
            .unwrap()
            .iter()
            .any(|name| name.contains(port)));

        let mut out = MidiOut::open(port).unwrap();
        out.send(&[0xB0, 74, 100]).unwrap();
        let clock = ClockSender::spawn(out, Tempo::DEFAULT);
        clock.set_playing(true, false);
        std::thread::sleep(Duration::from_millis(1_000));
        clock.set_playing(false, false);
        std::thread::sleep(Duration::from_millis(50));
        drop(clock);

        let bytes = received.lock().unwrap().clone();
        assert!(matches!(parse(&bytes[..3]), Some(MidiMessage::Event(_))));
        assert_eq!(bytes[3], START);
        assert_eq!(*bytes.last().unwrap(), STOP);
        // One second at 120 BPM is two beats: 48 pulses, give or take.
        let pulses = bytes.iter().filter(|byte| **byte == CLOCK).count();
        assert!((44..=52).contains(&pulses), "{pulses}");
    }
}
