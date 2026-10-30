//! Compiles projects to engine graphs on a worker thread, so building
//! devices (decoding samples, warping audio, loading plugins) never stalls
//! the UI. Requests coalesce: only the newest waiting project is compiled.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use fourfloor_compile::{compile_project, CompileOptions, CompiledProject};
use fourfloor_core::{DeviceRegistry, SampleRate};
use fourfloor_model::Project;

/// One compile request.
#[derive(Debug)]
pub struct CompileRequest {
    /// The project to compile.
    pub project: Project,
    /// The command-bus revision it came from.
    pub revision: u64,
    /// Project folder, for audio and plugin state.
    pub project_dir: Option<PathBuf>,
}

/// One finished compile.
#[derive(Debug)]
pub struct CompileResult {
    /// The revision that was compiled.
    pub revision: u64,
    /// The graph and its layout, or why it failed.
    pub outcome: Result<CompiledProject, String>,
}

/// The worker thread and its queues.
#[derive(Debug)]
pub struct CompileWorker {
    tx: Option<Sender<CompileRequest>>,
    rx: Receiver<CompileResult>,
    thread: Option<JoinHandle<()>>,
}

impl CompileWorker {
    /// Starts the worker for graphs at `sample_rate` and `max_block`.
    pub fn spawn(registry: Arc<DeviceRegistry>, sample_rate: SampleRate, max_block: usize) -> Self {
        let (tx, requests) = mpsc::channel::<CompileRequest>();
        let (results, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("fourfloor-compile".to_owned())
            .spawn(move || {
                while let Ok(mut request) = requests.recv() {
                    // Skip to the newest request.
                    while let Ok(newer) = requests.try_recv() {
                        request = newer;
                    }
                    let mut options = CompileOptions::new(sample_rate, max_block);
                    if let Some(dir) = &request.project_dir {
                        options = options.with_project_dir(dir);
                    }
                    let outcome = compile_project(&request.project, &registry, &options)
                        .map_err(|error| error.to_string());
                    let result = CompileResult {
                        revision: request.revision,
                        outcome,
                    };
                    if results.send(result).is_err() {
                        break;
                    }
                }
            })
            .ok();
        Self {
            tx: Some(tx),
            rx,
            thread,
        }
    }

    /// Queues a compile. Returns `false` if the worker has stopped.
    pub fn request(&self, request: CompileRequest) -> bool {
        self.tx.as_ref().is_some_and(|tx| tx.send(request).is_ok())
    }

    /// The newest finished compile, if any (older finished ones are
    /// dropped).
    pub fn try_latest(&self) -> Option<CompileResult> {
        let mut latest = None;
        while let Ok(result) = self.rx.try_recv() {
            latest = Some(result);
        }
        latest
    }

    /// Waits up to `timeout` for the next finished compile.
    pub fn wait(&self, timeout: Duration) -> Option<CompileResult> {
        self.rx.recv_timeout(timeout).ok()
    }
}

impl Drop for CompileWorker {
    fn drop(&mut self) {
        // Closing the request queue ends the thread's loop.
        self.tx = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
