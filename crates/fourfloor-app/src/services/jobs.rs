//! Background jobs (exports, imports, decoding, analysis) on worker
//! threads, with progress the status bar can show.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::Arc;

/// Shared progress of one job, 0 to 1.
#[derive(Debug, Default)]
pub struct Progress {
    fraction: AtomicU32,
    cancelled: AtomicBool,
}

impl Progress {
    /// Sets the fraction done (clamped to 0..=1).
    pub fn set(&self, fraction: f32) {
        let value = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.fraction.store(value.to_bits(), Ordering::Relaxed);
    }

    /// The fraction done.
    pub fn get(&self) -> f32 {
        f32::from_bits(self.fraction.load(Ordering::Relaxed))
    }

    /// Asks the job to stop early.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// `true` once cancel was asked for.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

/// A running job as the UI sees it.
#[derive(Debug, Clone)]
pub struct JobInfo {
    /// Job number.
    pub id: u64,
    /// What it does, for the status bar.
    pub label: String,
    /// Its progress.
    pub progress: Arc<Progress>,
}

/// A finished job.
#[derive(Debug)]
pub struct Finished<T> {
    /// Job number.
    pub id: u64,
    /// The label it ran under.
    pub label: String,
    /// What it produced.
    pub result: Result<T, String>,
}

/// Runs jobs producing `T` on their own threads.
#[derive(Debug)]
pub struct Jobs<T> {
    tx: Sender<Finished<T>>,
    rx: Receiver<Finished<T>>,
    running: Vec<JobInfo>,
    next_id: u64,
}

impl<T: Send + 'static> Default for Jobs<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Send + 'static> Jobs<T> {
    /// An empty job list.
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            tx,
            rx,
            running: Vec::new(),
            next_id: 1,
        }
    }

    /// Starts `work` on a new thread. It gets the progress to update.
    pub fn spawn(
        &mut self,
        label: impl Into<String>,
        work: impl FnOnce(&Progress) -> Result<T, String> + Send + 'static,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let label = label.into();
        let progress = Arc::new(Progress::default());
        let info = JobInfo {
            id,
            label: label.clone(),
            progress: Arc::clone(&progress),
        };
        let tx = self.tx.clone();
        let thread_label = label.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("fourfloor-job-{id}"))
            .spawn(move || {
                // A job that panics reports an error instead of hanging
                // "running" forever (the app shows it as a toast).
                let result =
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| work(&progress)))
                        .unwrap_or_else(|panic| Err(format!("crashed: {}", panic_text(&*panic))));
                progress.set(1.0);
                let _ = tx.send(Finished {
                    id,
                    label: thread_label,
                    result,
                });
            });
        match spawned {
            Ok(_) => self.running.push(info),
            Err(error) => {
                let _ = self.tx.send(Finished {
                    id,
                    label,
                    result: Err(format!("could not start a worker thread: {error}")),
                });
            }
        }
        id
    }

    /// Jobs still running.
    pub fn running(&self) -> &[JobInfo] {
        &self.running
    }

    /// Collects finished jobs.
    pub fn poll(&mut self) -> Vec<Finished<T>> {
        let finished: Vec<Finished<T>> = self.rx.try_iter().collect();
        self.running
            .retain(|job| !finished.iter().any(|done| done.id == job.id));
        finished
    }

    /// Waits for the next finished job (tests and headless runs).
    pub fn wait(&mut self, timeout: std::time::Duration) -> Option<Finished<T>> {
        let done = self.rx.recv_timeout(timeout).ok()?;
        self.running.retain(|job| job.id != done.id);
        Some(done)
    }
}

/// The message of a caught panic.
fn panic_text(panic: &(dyn std::any::Any + Send)) -> String {
    panic
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| panic.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_panicking_job_reports_an_error() {
        let mut jobs: Jobs<u32> = Jobs::new();
        jobs.spawn("boom", |_| panic!("bad input"));
        let done = jobs.wait(Duration::from_secs(5)).expect("job finished");
        assert_eq!(done.result, Err("crashed: bad input".to_owned()));
        assert!(jobs.running().is_empty());
    }

    #[test]
    fn jobs_report_progress_and_results() {
        let mut jobs: Jobs<u32> = Jobs::new();
        let id = jobs.spawn("count", |progress| {
            progress.set(0.5);
            Ok(42)
        });
        jobs.spawn("fail", |_| Err("nope".to_owned()));
        let mut results = Vec::new();
        while results.len() < 2 {
            let done = jobs.wait(Duration::from_secs(5)).expect("job finished");
            results.push((done.id, done.result));
        }
        assert!(results.contains(&(id, Ok(42))));
        assert!(results.iter().any(|(_, r)| r == &Err("nope".to_owned())));
        assert!(jobs.running().is_empty());
        let progress = Progress::default();
        progress.set(7.0);
        assert_eq!(progress.get(), 1.0);
        progress.cancel();
        assert!(progress.is_cancelled());
    }
}
