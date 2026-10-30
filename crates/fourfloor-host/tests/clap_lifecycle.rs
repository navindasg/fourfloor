//! Plugin lifetime: plugins can be loaded and dropped over and over, and
//! every instance is destroyed.
//!
//! This is a test binary of its own, with a single test, because it reads
//! the fixture library's process-wide count of live instances.

mod common;

use common::{
    fixture, prepare, process_block, resident_bytes, DELAY_ID, DELAY_SAMPLES, GAIN_ID, SINE_ID,
};
use fourfloor_core::Event;
use fourfloor_host::thread::HostThread;
use fourfloor_host::ClapDevice;
use fourfloor_testkit::AllocDisabler;

#[global_allocator]
static ALLOCATOR: AllocDisabler = AllocDisabler;

/// Reads the fixture library's own count of live plugin instances.
fn live_instances(library: &libloading::Library) -> i64 {
    // SAFETY: the symbol is `extern "C" fn() -> i64` in the fixture crate.
    unsafe {
        let count: libloading::Symbol<'_, unsafe extern "C" fn() -> i64> =
            library.get(b"fourfloor_test_live_instances").unwrap();
        count()
    }
}

#[test]
fn fifty_load_and_unload_cycles_destroy_every_instance_and_do_not_leak() {
    // Holding the library open keeps its instance counter alive between
    // the host's own loads and unloads of the same file.
    // SAFETY: the fixture library has no initialisers beyond Rust's own.
    let library = unsafe { libloading::Library::new(&fixture().bundle) }.unwrap();
    let thread = HostThread::spawn().unwrap();
    let ones = vec![0.5f32; 256];
    let start = live_instances(&library);

    let cycle = |id: &str| {
        let mut device = ClapDevice::load(&thread, &fixture().bundle, id, None).unwrap();
        let controller = device.controller();
        assert_eq!(live_instances(&library), start + 1);
        prepare(&mut device, 256);
        let events = [Event::note_on(0, 69, 100)];
        let (left, _) = process_block(&mut device, Some((&ones, &ones)), &events, 256);
        assert!(left.iter().all(|s| s.is_finite()));
        assert_eq!(controller.failed_blocks(), 0);
        // Prepared twice: deactivate and activate again.
        prepare(&mut device, 128);
        if id == DELAY_ID {
            assert_eq!(controller.latency_samples(), DELAY_SAMPLES);
        }
        // The instance lives until the device and its handles are gone.
        drop(device);
        assert_eq!(live_instances(&library), start + 1);
        drop(controller);
        assert_eq!(live_instances(&library), start);
    };
    for _ in 0..10 {
        cycle(GAIN_ID);
    }
    let before = resident_bytes();
    for round in 0..50 {
        cycle([GAIN_ID, DELAY_ID, SINE_ID][round % 3]);
    }
    assert_eq!(live_instances(&library), start);
    if let (Some(before), Some(after)) = (before, resident_bytes()) {
        let grown = after.saturating_sub(before);
        println!("resident memory grew by {grown} bytes over 50 instances");
        assert!(grown < 16 * 1024 * 1024, "grew by {grown} bytes");
    }

    // Dropped on a thread other than the one that loaded it, and many
    // alive at once.
    let devices: Vec<ClapDevice> = (0..8)
        .map(|_| ClapDevice::load(&thread, &fixture().bundle, SINE_ID, None).unwrap())
        .collect();
    assert_eq!(live_instances(&library), start + 8);
    std::thread::spawn(move || drop(devices)).join().unwrap();
    assert_eq!(live_instances(&library), start);
}
