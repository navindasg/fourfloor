//! Options and results of import and export.

use fourfloor_core::DeviceRegistry;
use fourfloor_model::Project;

use crate::container::DEFAULT_MAX_ENTRY_BYTES;
use crate::metadata::Metadata;
use crate::report::Report;

/// How a project is written.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ExportOptions<'a> {
    /// Descriptive fields for `metadata.xml`. An empty title is filled in
    /// with the project name.
    pub metadata: Option<&'a Metadata>,
    /// Write `fourfloor/extras.json`, which carries what DAWproject has no
    /// element for (swing, modulators, macros, sidechains, clip gain...).
    /// Other applications ignore it. On by default.
    pub extras: bool,
    /// Used to tell instruments, note effects and audio effects apart for
    /// `deviceRole`. Without it the role is guessed from the device kind.
    pub registry: Option<&'a DeviceRegistry>,
}

impl Default for ExportOptions<'_> {
    fn default() -> Self {
        Self {
            metadata: None,
            extras: true,
            registry: None,
        }
    }
}

impl<'a> ExportOptions<'a> {
    /// The defaults: extension file on, no metadata, no registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the metadata written to `metadata.xml`.
    pub fn with_metadata(self, metadata: &'a Metadata) -> Self {
        Self {
            metadata: Some(metadata),
            ..self
        }
    }

    /// Turns the extension file on or off.
    pub fn with_extras(self, extras: bool) -> Self {
        Self { extras, ..self }
    }

    /// Sets the registry used to find device roles.
    pub fn with_registry(self, registry: &'a DeviceRegistry) -> Self {
        Self {
            registry: Some(registry),
            ..self
        }
    }
}

/// The result of an export.
#[derive(Clone, Debug)]
pub struct Exported {
    /// The `.dawproject` file contents.
    pub bytes: Vec<u8>,
    /// What could not be written exactly.
    pub report: Report,
}

/// How a project is read.
#[derive(Clone, Copy, Debug)]
#[non_exhaustive]
pub struct ImportOptions {
    /// Read `fourfloor/extras.json` when the file has one. On by default.
    pub extras: bool,
    /// Largest archive entry accepted, uncompressed.
    pub max_entry_bytes: u64,
}

impl Default for ImportOptions {
    fn default() -> Self {
        Self {
            extras: true,
            max_entry_bytes: DEFAULT_MAX_ENTRY_BYTES,
        }
    }
}

impl ImportOptions {
    /// The defaults: extension file read, 4 GiB entry limit.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads or ignores the extension file.
    pub fn with_extras(self, extras: bool) -> Self {
        Self { extras, ..self }
    }

    /// Sets the entry size limit.
    pub fn with_max_entry_bytes(self, max_entry_bytes: u64) -> Self {
        Self {
            max_entry_bytes,
            ..self
        }
    }
}

/// The result of an import.
#[derive(Clone, Debug)]
pub struct Imported {
    /// The project. It passes [`Project::validate`].
    pub project: Project,
    /// The contents of `metadata.xml`, if the file had one.
    pub metadata: Metadata,
    /// What could not be read exactly.
    pub report: Report,
}
