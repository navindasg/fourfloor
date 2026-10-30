//! Messages from the UI side to the audio thread.
//!
//! [`AudioMessage`] is designed to travel through a lock-free
//! single-producer single-consumer queue: it is `Copy`, has no destructor,
//! owns no heap memory and is at most 24 bytes. Receiving and applying one
//! never allocates.

use crate::event::EventKind;
use crate::id::{DeviceId, ParamId};
use crate::time::{Beats, Tempo};

/// Opaque handle naming a compiled audio graph that the engine has already
/// built off the audio thread.
///
/// Placeholder: the graph itself is not carried in the message. The engine
/// decides how a handle maps to a graph (for example a slot in a
/// pre-allocated table, with the old graph handed back for freeing off the
/// audio thread).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GraphHandle(pub u32);

/// A real-time-safe instruction for the audio thread.
///
/// Messages take effect at the start of the next processed block.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AudioMessage {
    /// Set one parameter on one device. `value` is in the device's own
    /// plain (not normalized) units.
    SetParam {
        /// Target device.
        device: DeviceId,
        /// Parameter within that device.
        param: ParamId,
        /// New value.
        value: f32,
    },
    /// Deliver a note or controller event to one device at the start of
    /// the next block: live input from a keyboard, the piano roll's
    /// audition or the agent. Works whether or not the transport rolls.
    SendEvent {
        /// Target device, normally an instrument.
        device: DeviceId,
        /// The event.
        event: EventKind,
    },
    /// Start the transport from its current position.
    Play,
    /// Stop the transport, keeping its position.
    Stop,
    /// Move the transport to a musical position.
    Seek {
        /// Destination, in beats from the start of the timeline.
        position: Beats,
    },
    /// Change the tempo.
    SetTempo {
        /// New tempo.
        tempo: Tempo,
    },
    /// Loop playback between two musical positions: reaching `end` jumps
    /// back to `start`. A region that is empty or reversed is ignored.
    SetLoop {
        /// First beat of the loop (inclusive).
        start: Beats,
        /// End of the loop (exclusive).
        end: Beats,
    },
    /// Remove the loop region.
    ClearLoop,
    /// Switch to a previously prepared graph at the next block boundary.
    /// Placeholder; see [`GraphHandle`].
    SwapGraph {
        /// The prepared graph to switch to.
        graph: GraphHandle,
    },
}

// Compile-time contract: the message must stay trivially movable across
// threads and small enough to queue cheaply.
const _: () = {
    const fn assert_rt_safe<T: Copy + Send + Sync + 'static>() {}
    assert_rt_safe::<AudioMessage>();
    assert!(core::mem::size_of::<AudioMessage>() <= 24);
    assert!(!core::mem::needs_drop::<AudioMessage>());
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_small_and_has_no_destructor() {
        assert!(core::mem::size_of::<AudioMessage>() <= 24);
        assert!(!core::mem::needs_drop::<AudioMessage>());
    }

    #[test]
    fn is_copy_and_comparable() {
        let msg = AudioMessage::SetParam {
            device: DeviceId(1),
            param: ParamId(2),
            value: 0.5,
        };
        let copy = msg;
        assert_eq!(msg, copy);
        assert_ne!(msg, AudioMessage::Play);
        assert_ne!(AudioMessage::Play, AudioMessage::Stop);
    }

    #[test]
    fn carries_transport_payloads() {
        let seek = AudioMessage::Seek {
            position: Beats(16.0),
        };
        let tempo = AudioMessage::SetTempo {
            tempo: Tempo::new(126.0).unwrap(),
        };
        let swap = AudioMessage::SwapGraph {
            graph: GraphHandle(3),
        };
        match (seek, tempo, swap) {
            (
                AudioMessage::Seek { position },
                AudioMessage::SetTempo { tempo },
                AudioMessage::SwapGraph { graph },
            ) => {
                assert_eq!(position, Beats(16.0));
                assert_eq!(tempo.bpm(), 126.0);
                assert_eq!(graph, GraphHandle(3));
            }
            other => panic!("unexpected variants: {other:?}"),
        }
    }

    #[test]
    fn carries_live_events() {
        let message = AudioMessage::SendEvent {
            device: DeviceId(4),
            event: EventKind::note_on(60, 100),
        };
        match message {
            AudioMessage::SendEvent { device, event } => {
                assert_eq!(device, DeviceId(4));
                assert!(event.is_note_on());
            }
            other => panic!("unexpected variant: {other:?}"),
        }
    }

    #[test]
    fn carries_loop_commands() {
        let set = AudioMessage::SetLoop {
            start: Beats(4.0),
            end: Beats(8.0),
        };
        assert_eq!(
            set,
            AudioMessage::SetLoop {
                start: Beats(4.0),
                end: Beats(8.0),
            }
        );
        assert_ne!(set, AudioMessage::ClearLoop);
    }

    #[test]
    fn crosses_a_thread_boundary() {
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(AudioMessage::Play).unwrap());
        assert_eq!(rx.recv().unwrap(), AudioMessage::Play);
    }
}
