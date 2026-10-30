//! The host side of the CLAP interface: what a plugin can ask of fourfloor.
//!
//! Every request a plugin can make from the audio thread ends in an atomic
//! store and nothing else. The flags are picked up later on the plugin host
//! thread or through [`PluginControl`](crate::PluginControl).

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use clack_extensions::audio_ports::{AudioPortRescanFlags, HostAudioPorts, HostAudioPortsImpl};
use clack_extensions::latency::{HostLatency, HostLatencyImpl};
use clack_extensions::log::{HostLog, HostLogImpl, LogSeverity};
use clack_extensions::note_ports::{
    HostNotePorts, HostNotePortsImpl, NoteDialects, NotePortRescanFlags,
};
use clack_extensions::params::{
    HostParams, HostParamsImplMainThread, HostParamsImplShared, ParamClearFlags, ParamRescanFlags,
};
use clack_extensions::state::{HostState, HostStateImpl};
use clack_extensions::thread_check::{HostThreadCheck, HostThreadCheckImpl};
use clack_host::prelude::*;

use crate::info::PluginRequests;
use crate::thread::thread_token;

/// Flags and counters shared by the plugin's callbacks, the device on the
/// audio thread and the controller.
#[derive(Debug, Default)]
pub(crate) struct SharedFlags {
    /// The plugin asked for `on_main_thread`.
    pub callback: AtomicBool,
    /// The plugin asked for a parameter flush outside `process`.
    pub flush: AtomicBool,
    pub restart: AtomicBool,
    pub latency_changed: AtomicBool,
    pub state_dirty: AtomicBool,
    pub params_changed: AtomicBool,
    /// Blocks where `process` returned an error.
    pub failed_blocks: AtomicU32,
    /// Events that did not fit the preallocated buffer.
    pub dropped_events: AtomicU32,
    /// Latency reported at the last activation.
    pub latency: AtomicU32,
    /// Messages the plugin logged at warning level or above.
    pub log_warnings: AtomicU32,
}

impl SharedFlags {
    pub fn take_requests(&self) -> PluginRequests {
        PluginRequests {
            latency_changed: self.latency_changed.swap(false, Ordering::AcqRel),
            restart: self.restart.swap(false, Ordering::AcqRel),
            state_dirty: self.state_dirty.swap(false, Ordering::AcqRel),
            params_changed: self.params_changed.swap(false, Ordering::AcqRel),
        }
    }
}

/// Marker type tying the handler types together for clack.
pub(crate) struct FourfloorHost;

impl HostHandlers for FourfloorHost {
    type Shared<'a> = HostShared;
    type MainThread<'a> = HostMainThread<'a>;
    type AudioProcessor<'a> = ();

    fn declare_extensions(builder: &mut HostExtensions<Self>, _shared: &Self::Shared<'_>) {
        builder
            .register::<HostLog>()
            .register::<HostThreadCheck>()
            .register::<HostParams>()
            .register::<HostState>()
            .register::<HostLatency>()
            .register::<HostAudioPorts>()
            .register::<HostNotePorts>();
    }
}

/// Callbacks a plugin may make from any thread.
pub(crate) struct HostShared {
    pub flags: Arc<SharedFlags>,
    /// Token of the plugin host thread, for the thread-check extension.
    pub main_token: usize,
}

impl SharedHandler<'_> for HostShared {
    fn request_restart(&self) {
        self.flags.restart.store(true, Ordering::Release);
    }

    fn request_process(&self) {
        // The graph processes every device on every block already.
    }

    fn request_callback(&self) {
        self.flags.callback.store(true, Ordering::Release);
    }
}

impl HostLogImpl for HostShared {
    fn log(&self, severity: LogSeverity, _message: &str) {
        // Plugins may log from the audio thread, so the text is not kept:
        // storing it would allocate or lock.
        if !matches!(severity, LogSeverity::Debug | LogSeverity::Info) {
            self.flags.log_warnings.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl HostThreadCheckImpl for HostShared {
    fn is_main_thread(&self) -> bool {
        thread_token() == self.main_token
    }

    fn is_audio_thread(&self) -> bool {
        // Any thread that is not the plugin host thread may be driving the
        // graph: the device thread, an offline render or a test.
        thread_token() != self.main_token
    }
}

impl HostParamsImplShared for HostShared {
    fn request_flush(&self) {
        self.flags.flush.store(true, Ordering::Release);
    }
}

/// Callbacks a plugin makes on its main thread.
pub(crate) struct HostMainThread<'a> {
    pub shared: &'a HostShared,
}

impl<'a> MainThreadHandler<'a> for HostMainThread<'a> {}

impl HostParamsImplMainThread for HostMainThread<'_> {
    fn rescan(&self, _flags: ParamRescanFlags) {
        self.shared
            .flags
            .params_changed
            .store(true, Ordering::Release);
    }

    fn clear(&self, _param_id: ClapId, _flags: ParamClearFlags) {}
}

impl HostStateImpl for HostMainThread<'_> {
    fn mark_dirty(&self) {
        self.shared.flags.state_dirty.store(true, Ordering::Release);
    }
}

impl HostLatencyImpl for HostMainThread<'_> {
    fn changed(&self) {
        self.shared
            .flags
            .latency_changed
            .store(true, Ordering::Release);
    }
}

impl HostAudioPortsImpl for HostMainThread<'_> {
    fn is_rescan_flag_supported(&self, _flag: AudioPortRescanFlags) -> bool {
        false
    }

    fn rescan(&self, _flags: AudioPortRescanFlags) {
        // The port layout is read once per activation, so a change needs a
        // restart.
        self.shared.flags.restart.store(true, Ordering::Release);
    }
}

impl HostNotePortsImpl for HostMainThread<'_> {
    fn supported_dialects(&self) -> NoteDialects {
        NoteDialects::CLAP | NoteDialects::MIDI
    }

    fn rescan(&self, _flags: NotePortRescanFlags) {
        self.shared.flags.restart.store(true, Ordering::Release);
    }
}
