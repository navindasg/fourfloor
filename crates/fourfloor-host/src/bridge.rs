//! The bridge from the plugin database to the device registry.
//!
//! [`PluginHost::register_database`] adds one registry entry per loadable
//! plugin, under the kind `plugin.<format>:<id>`. From then on a project
//! device with that kind compiles into a hosted [`Device`] exactly like a
//! built-in one: the compiler calls the registry, the registry calls the
//! factory registered here, and the factory loads the plugin and restores
//! its saved state.

use std::sync::{Arc, Mutex, PoisonError, Weak};

use fourfloor_core::{Device, DeviceRegistry, RegistryError};

use crate::clap::ClapDevice;
use crate::database::PluginDatabase;
use crate::error::HostError;
use crate::info::{PluginControl, PluginController, PluginFormat, PluginInfo};
use crate::thread::HostThread;

/// A loaded plugin: the device for the audio graph and the handle that
/// stays behind with the app.
pub struct HostedPlugin {
    /// The device. Not yet prepared.
    pub device: Box<dyn Device>,
    /// Main-thread access to the same plugin.
    pub controller: PluginController,
}

/// Loads plugins and keeps track of the ones that are alive.
pub struct PluginHost {
    thread: Arc<HostThread>,
    live: Mutex<Vec<Weak<dyn PluginControl>>>,
}

impl PluginHost {
    /// A host on the process-wide plugin host thread.
    pub fn new() -> Result<Arc<Self>, HostError> {
        Ok(Self::with_thread(HostThread::global()?))
    }

    /// A host on its own thread.
    pub fn with_thread(thread: Arc<HostThread>) -> Arc<Self> {
        Arc::new(Self {
            thread,
            live: Mutex::new(Vec::new()),
        })
    }

    /// The thread this host's plugins use as their main thread.
    pub fn thread(&self) -> &Arc<HostThread> {
        &self.thread
    }

    /// Loads the plugin `info` describes, restoring `state` if given.
    pub fn load(&self, info: &PluginInfo, state: Option<&[u8]>) -> Result<HostedPlugin, HostError> {
        let hosted = match info.format {
            PluginFormat::Clap => {
                let device = ClapDevice::load(&self.thread, &info.path, &info.id, state)?;
                let controller = device.controller();
                HostedPlugin {
                    device: Box::new(device),
                    controller,
                }
            }
            PluginFormat::Au => self.load_audio_unit(info, state)?,
            PluginFormat::Vst3 => {
                // Always an error today; kept so the call site is ready.
                let device = crate::vst3::load(&self.thread, info, state)?;
                return Err(HostError::Unsupported(format!(
                    "no controller for VST3 device with {} parameters",
                    device.params().len()
                )));
            }
        };
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.retain(|weak| weak.strong_count() > 0);
        live.push(Arc::downgrade(&hosted.controller));
        Ok(hosted)
    }

    #[cfg(target_os = "macos")]
    fn load_audio_unit(
        &self,
        info: &PluginInfo,
        state: Option<&[u8]>,
    ) -> Result<HostedPlugin, HostError> {
        let device = crate::au::AuDevice::load(&self.thread, &info.id, state)?;
        let controller = device.controller();
        Ok(HostedPlugin {
            device: Box::new(device),
            controller,
        })
    }

    #[cfg(not(target_os = "macos"))]
    fn load_audio_unit(
        &self,
        info: &PluginInfo,
        _state: Option<&[u8]>,
    ) -> Result<HostedPlugin, HostError> {
        Err(HostError::Unsupported(format!(
            "Audio Units exist only on macOS, so \"{}\" cannot be loaded",
            info.name
        )))
    }

    /// Controllers of the plugins this host loaded that are still alive, in
    /// the order they were loaded.
    ///
    /// A controller lives as long as its device (or any clone of the
    /// controller) does. The app uses this list to save plugin state: the
    /// project compiler builds devices in track and chain order, so the
    /// n-th controller of a kind belongs to the n-th device of that kind.
    pub fn controllers(&self) -> Vec<PluginController> {
        let mut live = self.live.lock().unwrap_or_else(PoisonError::into_inner);
        live.retain(|weak| weak.strong_count() > 0);
        live.iter().filter_map(Weak::upgrade).collect()
    }

    /// Registers every loadable plugin in `database` with `registry` and
    /// returns how many were added.
    ///
    /// A plugin whose kind is already registered (the database was
    /// registered twice, or two formats collide) is skipped. Plugins this
    /// build cannot load, such as VST3, are not registered, so a project
    /// that uses one compiles with an "unknown kind" warning instead of a
    /// device that can never be built.
    pub fn register_database(
        self: &Arc<Self>,
        registry: &mut DeviceRegistry,
        database: &PluginDatabase,
    ) -> Result<usize, RegistryError> {
        let mut added = 0;
        for info in database.plugins.iter().filter(|info| info.loadable) {
            let descriptor = info.descriptor();
            if registry.contains(&descriptor.kind) {
                continue;
            }
            let host = Arc::clone(self);
            let info = info.clone();
            registry.register(descriptor, move |init| {
                host.load(&info, init.state)
                    .map(|hosted| hosted.device)
                    .map_err(Into::into)
            })?;
            added += 1;
        }
        Ok(added)
    }
}

/// Registers the plugins in the cached database at
/// [`PluginDatabase::default_path`] on the process-wide plugin host.
///
/// This never scans: with no cache file it registers nothing, so it is
/// cheap enough to call at every start. Run a scan
/// ([`PluginDatabase::rescan`]) and save the result to fill the cache.
pub fn register(registry: &mut DeviceRegistry) -> Result<(), RegistryError> {
    let Some(path) = PluginDatabase::default_path() else {
        return Ok(());
    };
    let database = PluginDatabase::load_or_empty(&path);
    if database.plugins.is_empty() {
        return Ok(());
    }
    // Without a host thread no plugin could be loaded anyway.
    let Ok(host) = PluginHost::new() else {
        return Ok(());
    };
    host.register_database(registry, &database).map(|_| ())
}
