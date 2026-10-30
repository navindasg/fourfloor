//! Shared by the CLAP integration tests: builds the fixture plugins in
//! `test-plugins/` and hands out the path of the resulting `.clap` file.

#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use fourfloor_core::{
    Event, EventBuffer, PrepareConfig, ProcessIo, SampleRate, Tempo, TransportContext,
};

pub const GAIN_ID: &str = "org.fourfloor.test.gain";
pub const DELAY_ID: &str = "org.fourfloor.test.delay";
pub const SINE_ID: &str = "org.fourfloor.test.sine";
/// The delay plugin's delay and reported latency, in frames.
pub const DELAY_SAMPLES: u32 = 441;
/// Parameter id of the gain plugin's gain and of the sine plugin's level.
pub const PARAM: fourfloor_core::ParamId = fourfloor_core::ParamId(1);

pub const RATE: SampleRate = SampleRate::HZ_48000;

/// A directory holding `fourfloor-test-plugins.clap`, and that file.
pub struct Fixture {
    pub dir: PathBuf,
    pub bundle: PathBuf,
}

/// Builds the fixture plugins (once per test binary) and returns where the
/// `.clap` file is.
///
/// The fixture is its own Cargo workspace, built into Cargo's per-crate
/// temporary directory so nothing lands in the source tree. The library is
/// copied to a `.clap` name in a directory of its own, so a directory scan
/// finds exactly one bundle.
pub fn fixture() -> &'static Fixture {
    static FIXTURE: OnceLock<Fixture> = OnceLock::new();
    FIXTURE.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("test-plugins/Cargo.toml");
        let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("fourfloor-test-plugins");
        let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
        let output = Command::new(cargo)
            .arg("build")
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&target)
            // The outer build's settings must not leak into the fixture's.
            .env_remove("CARGO_TARGET_DIR")
            .env_remove("RUSTFLAGS")
            .env_remove("CARGO_ENCODED_RUSTFLAGS")
            .output()
            .expect("cargo could not be started to build the fixture plugins");
        assert!(
            output.status.success(),
            "building the fixture plugins failed:\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let library = target.join("debug").join(format!(
            "{}fourfloor_test_plugins{}",
            std::env::consts::DLL_PREFIX,
            std::env::consts::DLL_SUFFIX
        ));
        assert!(library.exists(), "{} was not built", library.display());

        // One directory per test binary: binaries may run at the same time.
        let dir = target.join(format!("bundles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("cannot create the bundle directory");
        let bundle = dir.join("fourfloor-test-plugins.clap");
        std::fs::copy(&library, &bundle).expect("cannot copy the fixture library");
        Fixture { dir, bundle }
    })
}

pub fn tempo() -> Tempo {
    Tempo::new(120.0).expect("120 BPM is a valid tempo")
}

/// Runs one stereo block through an already prepared device.
pub fn process_block(
    device: &mut dyn fourfloor_core::Device,
    input: Option<(&[f32], &[f32])>,
    events: &[Event],
    frames: usize,
) -> (Vec<f32>, Vec<f32>) {
    let (mut left, mut right) = (vec![f32::NAN; frames], vec![f32::NAN; frames]);
    let mut events_out = EventBuffer::with_capacity(64);
    let inputs: Vec<&[f32]> = input.map_or_else(Vec::new, |(l, r)| vec![l, r]);
    {
        let mut outputs: [&mut [f32]; 2] = [&mut left, &mut right];
        let mut io = ProcessIo {
            inputs: &inputs,
            sidechain: None,
            outputs: &mut outputs,
            events,
            events_out: &mut events_out,
        };
        device.process(&TransportContext::playing_at(RATE, tempo(), 0), &mut io);
    }
    (left, right)
}

pub fn prepare(device: &mut dyn fourfloor_core::Device, max_block: usize) {
    device.prepare(&PrepareConfig::stereo(RATE, max_block));
}

/// Resident memory of this process in bytes, read with `ps`; `None` when
/// `ps` is not available.
pub fn resident_bytes() -> Option<u64> {
    let output = Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    let kilobytes: u64 = String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse()
        .ok()?;
    Some(kilobytes * 1024)
}
