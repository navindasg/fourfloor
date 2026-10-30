//! The plugin main thread.
//!
//! Every plugin format splits its API in two: calls that may run on the
//! audio thread (`process`, `reset`) and calls that must all come from one
//! "main" thread (create, activate, read parameters, save state, destroy).
//! A [`fourfloor_core::Device`] is `Send` and gets moved between threads by
//! the engine, so the device cannot be that main thread itself.
//!
//! [`HostThread`] is a worker thread that plays the main-thread role for
//! every hosted plugin. The main-thread half of a plugin lives in a
//! [`MainBound`]: the value is created, used and dropped on the host thread
//! only, while the `MainBound` handle can be held and called from anywhere.
//! Calls block until the host thread has run them, so they belong in
//! `prepare`, in project load/save and in drop, never in `process`.
//!
//! A plugin editor needs the operating system's UI thread instead. When the
//! app grows one, it will hand its UI thread's executor to the host in place
//! of this worker; see [`crate::gui`].

use std::cell::{Cell, RefCell, UnsafeCell};
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr::NonNull;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// How often the host thread wakes to service plugin callback requests
/// while nothing else is queued.
const IDLE_INTERVAL: Duration = Duration::from_millis(20);

type Job = Box<dyn FnOnce() + Send + 'static>;

/// Why a call could not run on the host thread.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostThreadError {
    /// The host thread could not be started or has exited.
    Stopped,
    /// The call panicked on the host thread.
    Panicked,
    /// The value is already in use further up this call stack, or an
    /// earlier call on it panicked.
    Busy,
}

impl fmt::Display for HostThreadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stopped => "the plugin host thread is not running",
            Self::Panicked => "a call on the plugin host thread panicked",
            Self::Busy => "the plugin is busy with another main-thread call",
        })
    }
}

impl std::error::Error for HostThreadError {}

/// A number that identifies the calling OS thread. Reading it never
/// allocates, so it is safe on the audio thread.
pub(crate) fn thread_token() -> usize {
    #[cfg(unix)]
    {
        extern "C" {
            fn pthread_self() -> usize;
        }
        // SAFETY: `pthread_self` has no preconditions and always succeeds.
        unsafe { pthread_self() }
    }
    #[cfg(windows)]
    {
        extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        // SAFETY: `GetCurrentThreadId` has no preconditions.
        unsafe { GetCurrentThreadId() as usize }
    }
    #[cfg(not(any(unix, windows)))]
    {
        0
    }
}

/// Something that wants a regular call on the host thread.
pub trait MainIdle {
    /// Called on the host thread between other calls, roughly every 20 ms.
    fn idle(&mut self) {}
}

thread_local! {
    /// Host thread only: every live [`MainBound`] value, for idle calls.
    static IDLERS: RefCell<Vec<(usize, NonNull<dyn IdleSlot>)>> = const { RefCell::new(Vec::new()) };
}

/// The worker thread that acts as the main thread for hosted plugins.
pub struct HostThread {
    sender: mpsc::Sender<Job>,
    token: usize,
}

impl fmt::Debug for HostThread {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HostThread").finish_non_exhaustive()
    }
}

impl HostThread {
    /// Starts a host thread.
    pub fn spawn() -> Result<Arc<Self>, HostThreadError> {
        let (sender, receiver) = mpsc::channel::<Job>();
        let (ready_tx, ready_rx) = mpsc::sync_channel::<usize>(1);
        std::thread::Builder::new()
            .name("fourfloor-plugin-host".to_owned())
            .spawn(move || {
                let _ = ready_tx.send(thread_token());
                worker(&receiver);
            })
            .map_err(|_| HostThreadError::Stopped)?;
        let token = ready_rx.recv().map_err(|_| HostThreadError::Stopped)?;
        Ok(Arc::new(Self { sender, token }))
    }

    /// The process-wide host thread, started on first use.
    pub fn global() -> Result<Arc<Self>, HostThreadError> {
        static GLOBAL: OnceLock<Result<Arc<HostThread>, HostThreadError>> = OnceLock::new();
        GLOBAL.get_or_init(Self::spawn).clone()
    }

    /// True when called from the host thread itself. Never allocates.
    pub fn is_current(&self) -> bool {
        thread_token() == self.token
    }

    /// Runs `f` on the host thread and waits for its result. Called from
    /// the host thread itself, `f` simply runs in place.
    pub fn run<'a, R, F>(&self, f: F) -> Result<R, HostThreadError>
    where
        R: Send + 'a,
        F: FnOnce() -> R + Send + 'a,
    {
        if self.is_current() {
            return Ok(f());
        }
        let (result_tx, result_rx) = mpsc::sync_channel::<R>(1);
        let job: Box<dyn FnOnce() + Send + 'a> = Box::new(move || {
            let _ = result_tx.send(f());
        });
        // SAFETY: only the lifetime is changed. This function does not
        // return until `result_rx.recv()` does, and that happens only once
        // the job has either run to its `send` or been dropped (which drops
        // `result_tx`). In both cases `f` and everything it borrowed are no
        // longer touched by the host thread afterwards.
        let job: Job = unsafe { std::mem::transmute(job) };
        self.sender
            .send(job)
            .map_err(|_| HostThreadError::Stopped)?;
        result_rx.recv().map_err(|_| HostThreadError::Panicked)
    }
}

fn worker(receiver: &mpsc::Receiver<Job>) {
    loop {
        let has_idlers = IDLERS.with(|idlers| !idlers.borrow().is_empty());
        let job = if has_idlers {
            match receiver.recv_timeout(IDLE_INTERVAL) {
                Ok(job) => Some(job),
                Err(RecvTimeoutError::Timeout) => None,
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            match receiver.recv() {
                Ok(job) => Some(job),
                Err(_) => break,
            }
        };
        if let Some(job) = job {
            // A panicking job drops its result sender, which the caller
            // sees as `HostThreadError::Panicked`; the thread lives on.
            let _ = catch_unwind(AssertUnwindSafe(job));
        }
        run_idlers();
    }
}

fn run_idlers() {
    let mut index = 0;
    while let Some(slot) = IDLERS.with(|idlers| idlers.borrow().get(index).map(|(_, slot)| *slot)) {
        // SAFETY: a slot is removed from the list, on this thread, before
        // it is freed, so the pointer is live. No job is running, so nothing
        // else is using the slot.
        let _ = catch_unwind(AssertUnwindSafe(|| unsafe { slot.as_ref().idle_slot() }));
        index += 1;
    }
}

trait IdleSlot {
    fn idle_slot(&self);
}

struct Slot<T> {
    value: UnsafeCell<T>,
    /// True while a call is using `value`, so a nested call cannot create a
    /// second mutable reference.
    busy: Cell<bool>,
}

impl<T: MainIdle> IdleSlot for Slot<T> {
    fn idle_slot(&self) {
        if self.busy.replace(true) {
            return;
        }
        // SAFETY: `busy` was false, so no other reference to the value
        // exists, and only the host thread ever reaches this code.
        unsafe { (*self.value.get()).idle() };
        self.busy.set(false);
    }
}

struct SendPtr<T>(NonNull<Slot<T>>);

impl<T> SendPtr<T> {
    fn get(&self) -> NonNull<Slot<T>> {
        self.0
    }
}

// SAFETY: the pointer is only ever dereferenced on the host thread; other
// threads just carry it around.
unsafe impl<T> Send for SendPtr<T> {}

/// A value that lives on the host thread.
///
/// The value is built, used and dropped there, so it does not need to be
/// `Send` (a CLAP plugin instance is not). The handle itself is `Send` and
/// `Sync`: every access goes through [`with`](Self::with), which runs on the
/// host thread, one call at a time.
pub struct MainBound<T: MainIdle + 'static> {
    slot: NonNull<Slot<T>>,
    thread: Arc<HostThread>,
}

// SAFETY: the value is only touched on the host thread, through `with`,
// `idle` and `drop`, and the `busy` flag stops two uses overlapping.
unsafe impl<T: MainIdle + 'static> Send for MainBound<T> {}
// SAFETY: as above; `with` serialises all access on the host thread.
unsafe impl<T: MainIdle + 'static> Sync for MainBound<T> {}

impl<T: MainIdle + 'static> MainBound<T> {
    /// Builds a value on the host thread with `make`.
    pub fn new<E, F>(thread: &Arc<HostThread>, make: F) -> Result<Self, E>
    where
        E: From<HostThreadError> + Send,
        F: FnOnce() -> Result<T, E> + Send,
    {
        let slot = thread.run(move || {
            let value = make()?;
            let slot = Box::new(Slot {
                value: UnsafeCell::new(value),
                busy: Cell::new(false),
            });
            let slot = NonNull::from(Box::leak(slot));
            let erased: NonNull<dyn IdleSlot> = slot;
            IDLERS.with(|idlers| {
                idlers.borrow_mut().push((slot.as_ptr() as usize, erased));
            });
            Ok::<_, E>(SendPtr(slot))
        })??;
        Ok(Self {
            slot: slot.get(),
            thread: Arc::clone(thread),
        })
    }

    /// The host thread this value lives on.
    pub fn thread(&self) -> &Arc<HostThread> {
        &self.thread
    }

    /// Runs `f` on the host thread with the value and waits for the result.
    pub fn with<'a, R, F>(&'a self, f: F) -> Result<R, HostThreadError>
    where
        R: Send + 'a,
        F: FnOnce(&mut T) -> R + Send + 'a,
    {
        let slot = SendPtr(self.slot);
        self.thread.run(move || {
            // SAFETY: the slot is freed only by `drop`, which cannot run
            // while `self` is borrowed here.
            let slot = unsafe { slot.get().as_ref() };
            if slot.busy.replace(true) {
                return Err(HostThreadError::Busy);
            }
            // SAFETY: `busy` was false, so this is the only reference, and
            // we are on the host thread. If `f` panics the flag stays set
            // and the value is never touched again.
            let result = f(unsafe { &mut *slot.value.get() });
            slot.busy.set(false);
            Ok(result)
        })?
    }
}

impl<T: MainIdle + 'static> Drop for MainBound<T> {
    fn drop(&mut self) {
        let slot = SendPtr(self.slot);
        // If the host thread is gone the value is leaked: dropping a plugin
        // on the wrong thread would be worse.
        let _ = self.thread.run(move || {
            let slot = slot.get();
            let key = slot.as_ptr() as usize;
            IDLERS.with(|idlers| idlers.borrow_mut().retain(|(k, _)| *k != key));
            // SAFETY: the slot came from `Box::leak` in `new`, is dropped
            // exactly once, here, on the host thread, and was just removed
            // from the idle list.
            drop(unsafe { Box::from_raw(slot.as_ptr()) });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Not `Send`: proves the value never leaves the host thread.
    struct Local {
        token: usize,
        _not_send: Rc<()>,
        idles: Arc<AtomicU32>,
        drops: Arc<AtomicU32>,
    }

    impl MainIdle for Local {
        fn idle(&mut self) {
            assert_eq!(thread_token(), self.token);
            self.idles.fetch_add(1, Ordering::SeqCst);
        }
    }

    impl Drop for Local {
        fn drop(&mut self) {
            assert_eq!(thread_token(), self.token, "dropped off the host thread");
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }

    fn local(thread: &Arc<HostThread>) -> (MainBound<Local>, Arc<AtomicU32>, Arc<AtomicU32>) {
        let idles = Arc::new(AtomicU32::new(0));
        let drops = Arc::new(AtomicU32::new(0));
        let (i, d) = (Arc::clone(&idles), Arc::clone(&drops));
        let bound = MainBound::new(thread, move || {
            Ok::<_, HostThreadError>(Local {
                token: thread_token(),
                _not_send: Rc::new(()),
                idles: i,
                drops: d,
            })
        })
        .unwrap();
        (bound, idles, drops)
    }

    #[test]
    fn run_executes_on_the_host_thread_and_borrows_locals() {
        let thread = HostThread::spawn().unwrap();
        let here = thread_token();
        let mut seen = 0usize;
        let label = String::from("borrowed");
        let length = thread
            .run(|| {
                seen = thread_token();
                label.len()
            })
            .unwrap();
        assert_eq!(length, 8);
        assert_ne!(seen, here);
        assert!(!thread.is_current());
        assert!(thread.run(|| thread.is_current()).unwrap());
    }

    #[test]
    fn nested_run_on_the_host_thread_does_not_deadlock() {
        let thread = HostThread::spawn().unwrap();
        let inner = Arc::clone(&thread);
        assert_eq!(thread.run(move || inner.run(|| 7)).unwrap(), Ok(7));
    }

    #[test]
    fn a_panicking_job_is_reported_and_the_thread_survives() {
        let thread = HostThread::spawn().unwrap();
        let result = thread.run(|| -> u32 { panic!("boom") });
        assert_eq!(result, Err(HostThreadError::Panicked));
        assert_eq!(thread.run(|| 3), Ok(3));
    }

    #[test]
    fn a_bound_value_is_built_used_idled_and_dropped_on_the_host_thread() {
        let thread = HostThread::spawn().unwrap();
        let (bound, idles, drops) = local(&thread);
        let host_token = bound.with(|value| value.token).unwrap();
        assert_ne!(host_token, thread_token());

        std::thread::sleep(IDLE_INTERVAL * 5);
        assert!(idles.load(Ordering::SeqCst) >= 1, "idle was never called");

        // Usable, and droppable, from another thread.
        std::thread::spawn(move || {
            assert_eq!(bound.with(|value| value.token).unwrap(), host_token);
            drop(bound);
        })
        .join()
        .unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_nested_use_of_the_same_value_is_refused() {
        let thread = HostThread::spawn().unwrap();
        let (bound, _, _) = local(&thread);
        let nested = bound.with(|_| bound.with(|_| ())).unwrap();
        assert_eq!(nested, Err(HostThreadError::Busy));
        assert!(bound.with(|_| ()).is_ok(), "the outer call released it");
    }

    #[test]
    fn a_failing_constructor_returns_its_error() {
        #[derive(Debug, PartialEq)]
        enum Failure {
            Thread,
            Custom,
        }
        impl From<HostThreadError> for Failure {
            fn from(_: HostThreadError) -> Self {
                Self::Thread
            }
        }
        struct Never;
        impl MainIdle for Never {}
        let thread = HostThread::spawn().unwrap();
        let result = MainBound::<Never>::new(&thread, || Err(Failure::Custom));
        assert_eq!(result.err(), Some(Failure::Custom));
    }

    #[test]
    fn the_global_thread_is_shared() {
        let a = HostThread::global().unwrap();
        let b = HostThread::global().unwrap();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
