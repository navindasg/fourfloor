//! MIDI controller input. Messages arrive on the OS MIDI thread and are
//! queued for the UI thread, which routes them (notes to the armed
//! track's instrument, controllers through the project's mappings).
//!
//! Routing on the UI thread adds up to one UI frame of latency (about
//! 16 ms); playing notes live is usable but not tight.

use std::sync::mpsc::{self, Receiver};

/// Names of the MIDI input ports.
#[cfg(feature = "midi")]
pub fn input_port_names() -> Vec<String> {
    fourfloor_engine::midi::ports::input_names().unwrap_or_default()
}

/// Names of the MIDI input ports (none without the `midi` feature).
#[cfg(not(feature = "midi"))]
pub fn input_port_names() -> Vec<String> {
    Vec::new()
}

/// An open input port.
pub struct MidiInput {
    #[cfg(feature = "midi")]
    _port: fourfloor_engine::midi::ports::MidiIn,
    rx: Receiver<Vec<u8>>,
    /// The port's name.
    pub name: String,
}

impl std::fmt::Debug for MidiInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MidiInput")
            .field("name", &self.name)
            .finish()
    }
}

impl MidiInput {
    /// Opens the first port whose name contains `name`. `wake` is called
    /// from the MIDI thread after each message (to request a repaint).
    #[cfg(feature = "midi")]
    pub fn open(name: &str, wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let (tx, rx) = mpsc::channel();
        let port = fourfloor_engine::midi::ports::MidiIn::open(name, move |_, bytes| {
            let _ = tx.send(bytes.to_vec());
            wake();
        })
        .map_err(|error| error.to_string())?;
        Ok(Self {
            name: port.name().to_owned(),
            _port: port,
            rx,
        })
    }

    /// Without the `midi` feature no port can be opened.
    #[cfg(not(feature = "midi"))]
    pub fn open(name: &str, _wake: impl Fn() + Send + 'static) -> Result<Self, String> {
        let (_tx, rx) = mpsc::channel::<Vec<u8>>();
        let _ = rx;
        Err(format!("MIDI support is not built in (port \"{name}\")"))
    }

    /// Messages received since the last call.
    pub fn drain(&self) -> Vec<Vec<u8>> {
        self.rx.try_iter().collect()
    }
}
