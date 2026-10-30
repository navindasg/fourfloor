//! Loads and unloads Apple's stock Audio Units many times and checks the
//! process does not grow.
//!
//! This lives in its own test binary on purpose. Resident memory is a
//! process-wide number, so any test running beside it (a full Audio Unit
//! scan loads every installed unit, including large third-party synths)
//! shows up as a "leak". Cargo runs test binaries one at a time, so here it
//! has the process to itself.

#![cfg(target_os = "macos")]

mod common;

use common::{prepare, process_block, resident_bytes};
use fourfloor_host::au::AuDevice;
use fourfloor_host::thread::HostThread;

const DELAY: &str = "aufx.dely.appl";
const LOWPASS: &str = "aufx.lpas.appl";

#[test]
fn fifty_load_and_unload_cycles_do_not_crash_or_leak() {
    let thread = HostThread::spawn().unwrap();
    let ones = vec![0.5f32; 256];
    let cycle = |id: &str| {
        let mut device = AuDevice::load(&thread, id, None).unwrap();
        prepare(&mut device, 256);
        let (left, _) = process_block(&mut device, Some((&ones, &ones)), &[], 256);
        assert!(left.iter().all(|s| s.is_finite()));
        assert_eq!(device.controller().failed_blocks(), 0);
    };
    for _ in 0..10 {
        cycle(DELAY);
        cycle(LOWPASS);
    }
    let before = resident_bytes();
    for _ in 0..50 {
        cycle(DELAY);
        cycle(LOWPASS);
    }
    if let (Some(before), Some(after)) = (before, resident_bytes()) {
        let grown = after.saturating_sub(before);
        println!("resident memory grew by {grown} bytes over 100 instances");
        assert!(grown < 16 * 1024 * 1024, "grew by {grown} bytes");
    }
}
