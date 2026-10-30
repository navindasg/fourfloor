//! Projects the app makes and the files it reads and writes.

pub mod demo;
pub mod files;

pub use demo::{build_demo_project, DemoError, DEMO_BARS, DEMO_BPM, DEMO_TRACKS};
pub use files::{JobOutput, ReferenceLoaded};
