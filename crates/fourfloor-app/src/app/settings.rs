//! Settings intents: the model registry's download state and the hosted
//! plugin scan.

use fourfloor_ml::ModelCache;

use crate::project::JobOutput;
use crate::services::ml::{download_job, model_statuses};
use crate::services::plugins::{scan_job, PluginList};
use crate::state::SettingsIntent;

use super::FourfloorApp;

/// Job label of a plugin scan.
pub const PLUGIN_SCAN: &str = "Scanning plugins";
/// Prefix of a model download's job label.
pub const DOWNLOAD: &str = "Downloading ";

impl FourfloorApp {
    pub(super) fn settings_intent(&mut self, intent: SettingsIntent) {
        match intent {
            SettingsIntent::ScanPlugins => self.scan_plugins(),
            SettingsIntent::ReloadPlugins => {
                self.features.plugins = PluginList::load(self.features.paths.plugins.clone());
            }
        }
    }

    fn scan_plugins(&mut self) {
        if self.features.plugin_scan {
            return;
        }
        let Some(path) = self.features.paths.plugins.clone() else {
            self.ui_state
                .error("Plugins are off in this run (no scan cache path)");
            return;
        };
        self.features.plugin_scan = true;
        let options = fourfloor_host::ScanOptions::default();
        let job = scan_job(path, options);
        self.jobs.spawn(PLUGIN_SCAN, move |progress| {
            job(progress).map(JobOutput::Plugins)
        });
    }

    pub(super) fn plugins_scanned(&mut self, list: PluginList) {
        self.features.plugin_scan = false;
        let loadable = list.rows.iter().filter(|row| row.loadable).count();
        self.ui_state.info(format!(
            "Found {} plugins ({loadable} loadable); restart with --plugins to use new ones",
            list.rows.len()
        ));
        self.features.plugins = list;
    }

    /// Re-reads which models are in the cache.
    pub(super) fn refresh_models(&mut self) {
        let cache = ModelCache::new(self.features.paths.models.clone()).offline();
        self.features.models = model_statuses(&cache);
    }

    pub(super) fn download_model(&mut self, name: String) {
        if self.features.downloading.contains(&name) {
            return;
        }
        self.features.downloading.push(name.clone());
        let job = download_job(name.clone(), self.features.paths.models.clone());
        self.jobs
            .spawn(format!("{DOWNLOAD}{name}"), move |progress| {
                job(progress).map(JobOutput::ModelDownloaded)
            });
    }

    pub(super) fn model_downloaded(&mut self, name: &str) {
        self.features.downloading.retain(|n| n != name);
        self.refresh_models();
        self.ui_state
            .info(format!("Model {name} downloaded and verified"));
    }

    /// A settings job failed: clear its busy flag.
    pub(super) fn settings_job_failed(&mut self, label: &str) {
        if label == PLUGIN_SCAN {
            self.features.plugin_scan = false;
        }
        if let Some(name) = label.strip_prefix(DOWNLOAD) {
            self.features.downloading.retain(|n| n != name);
        }
    }
}
