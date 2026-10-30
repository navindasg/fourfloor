//! Fixture CLAP plugins for the `fourfloor-host` tests.
//!
//! One library, three plugins:
//!
//! | id | what it does |
//! | --- | --- |
//! | `org.fourfloor.test.gain` | Stereo gain with one parameter and saved state. |
//! | `org.fourfloor.test.delay` | Delays its input by exactly [`DELAY_SAMPLES`] and reports that as latency. |
//! | `org.fourfloor.test.sine` | Polyphonic sine instrument driven by CLAP or MIDI notes. |
//!
//! The library also exports [`fourfloor_test_live_instances`], so a leak test
//! can check that every instance it created was destroyed.

mod delay;
mod gain;
mod sine;

use std::ffi::CStr;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};

use clack_plugin::entry::prelude::*;
use clack_plugin::prelude::*;

pub use delay::DELAY_SAMPLES;

/// Plugin instances that exist right now, across all three plugins.
static LIVE_INSTANCES: AtomicI64 = AtomicI64::new(0);

/// Returns how many plugin instances from this library are alive.
#[no_mangle]
pub extern "C" fn fourfloor_test_live_instances() -> i64 {
    LIVE_INSTANCES.load(Ordering::SeqCst)
}

/// Counts one live instance for as long as it exists.
pub(crate) struct InstanceGuard;

impl InstanceGuard {
    pub(crate) fn new() -> Self {
        LIVE_INSTANCES.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for InstanceGuard {
    fn drop(&mut self) {
        LIVE_INSTANCES.fetch_sub(1, Ordering::SeqCst);
    }
}

/// An `f64` shared between the main thread and the audio thread.
pub(crate) struct AtomicF64(AtomicU64);

impl AtomicF64 {
    pub(crate) fn new(value: f64) -> Self {
        Self(AtomicU64::new(value.to_bits()))
    }

    pub(crate) fn get(&self) -> f64 {
        f64::from_bits(self.0.load(Ordering::Relaxed))
    }

    pub(crate) fn set(&self, value: f64) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// The library entry: a factory listing the three plugins.
pub struct TestEntry {
    factory: PluginFactoryWrapper<TestFactory>,
}

impl Entry for TestEntry {
    fn new(_bundle_path: Option<&CStr>) -> Result<Self, EntryLoadError> {
        Ok(Self {
            factory: PluginFactoryWrapper::new(TestFactory {
                gain: gain::descriptor(),
                delay: delay::descriptor(),
                sine: sine::descriptor(),
            }),
        })
    }

    fn declare_factories<'a>(&'a self, builder: &mut EntryFactories<'a>) {
        builder.register_factory(&self.factory);
    }
}

struct TestFactory {
    gain: PluginDescriptor,
    delay: PluginDescriptor,
    sine: PluginDescriptor,
}

impl PluginFactoryImpl for TestFactory {
    fn plugin_count(&self) -> u32 {
        3
    }

    fn plugin_descriptor(&self, index: u32) -> Option<&PluginDescriptor> {
        match index {
            0 => Some(&self.gain),
            1 => Some(&self.delay),
            2 => Some(&self.sine),
            _ => None,
        }
    }

    fn create_plugin<'a>(
        &'a self,
        host_info: HostInfo<'a>,
        plugin_id: &CStr,
    ) -> Option<PluginInstance<'a>> {
        let is = |descriptor: &PluginDescriptor| descriptor.id() == Some(plugin_id);
        if is(&self.gain) {
            Some(PluginInstance::new::<gain::GainPlugin>(
                host_info,
                &self.gain,
                |_host| Ok(gain::GainShared::new()),
                |_host, shared| Ok(gain::GainMainThread::new(shared)),
            ))
        } else if is(&self.delay) {
            Some(PluginInstance::new::<delay::DelayPlugin>(
                host_info,
                &self.delay,
                |_host| Ok(()),
                |_host, _shared| Ok(delay::DelayMainThread::new()),
            ))
        } else if is(&self.sine) {
            Some(PluginInstance::new::<sine::SinePlugin>(
                host_info,
                &self.sine,
                |_host| Ok(sine::SineShared::new()),
                |_host, shared| Ok(sine::SineMainThread::new(shared)),
            ))
        } else {
            None
        }
    }
}

clack_export_entry!(TestEntry);
