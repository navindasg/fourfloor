//! DAWproject import and export: moves a session between fourfloor and other
//! DAWs through [`fourfloor_model::Project`], plus Standard MIDI File import
//! and export for clips ([`smf`]).
//!
//! DAWproject is Bitwig's open interchange format (MIT licensed): a ZIP
//! archive holding `project.xml`, `metadata.xml` and embedded media and
//! plugin state. The XML written here follows the official `Project.xsd`.
//!
//! Nothing is dropped silently. Every [`export_project`] and
//! [`import_project`] returns a [`Report`] listing what the target could not
//! hold exactly and what happened to it ([`Outcome`]). Parts of a fourfloor
//! project that DAWproject has no element for (swing, modulators, macros,
//! controller mappings, sidechain routing, clip gain) travel in an extension
//! file, `fourfloor/extras.json`, that other applications ignore; fourfloor
//! reads it back, so a fourfloor project survives its own round trip
//! unchanged.

#![warn(missing_docs)]

pub mod container;
mod error;
mod export;
mod extras;
mod import;
mod metadata;
mod naming;
mod options;
mod report;
pub mod smf;
mod units;
mod xml;

pub use error::DawprojectError;
pub use export::{export_project, export_to_file, APPLICATION_NAME, FORMAT_VERSION};
pub use import::{import_file, import_project};
pub use metadata::Metadata;
pub use options::{ExportOptions, Exported, ImportOptions, Imported};
pub use report::{Outcome, Report, ReportItem, Subject};

pub use fourfloor_core as core;
