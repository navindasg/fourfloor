//! The ZIP container of a `.dawproject` file.
//!
//! A DAWproject file is a ZIP archive holding `project.xml`, `metadata.xml`
//! and any embedded media and plugin state files. This module reads and
//! writes such archives as a list of named entries held in memory.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::error::DawprojectError;

/// Name of the project document inside the archive.
pub const PROJECT_XML: &str = "project.xml";
/// Name of the metadata document inside the archive.
pub const METADATA_XML: &str = "metadata.xml";
/// Name of fourfloor's extension file inside the archive.
pub const EXTRAS_JSON: &str = "fourfloor/extras.json";

/// Default largest entry an import accepts: 4 GiB uncompressed.
pub const DEFAULT_MAX_ENTRY_BYTES: u64 = 4 << 30;

/// The entries of a DAWproject archive, by name.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Container {
    entries: BTreeMap<String, Vec<u8>>,
}

fn zip_error(error: impl std::fmt::Display) -> DawprojectError {
    DawprojectError::Zip(error.to_string())
}

/// Normalizes an entry path: forward slashes, no leading `./` or `/`.
/// `None` for a path that tries to leave the archive (`..`) or is empty.
pub fn normalize_path(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return None,
            other => parts.push(other),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

impl Container {
    /// An empty archive.
    pub fn new() -> Self {
        Self::default()
    }

    /// Reads an archive, refusing any entry larger than `max_entry_bytes`.
    /// Directory entries are skipped.
    pub fn read(bytes: &[u8], max_entry_bytes: u64) -> Result<Self, DawprojectError> {
        let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(zip_error)?;
        let mut entries = BTreeMap::new();
        for index in 0..archive.len() {
            let mut file = archive.by_index(index).map_err(zip_error)?;
            if file.is_dir() {
                continue;
            }
            let raw_name = file.name().map_err(zip_error)?.into_owned();
            let Some(name) = normalize_path(&raw_name) else {
                continue;
            };
            let size = file.size();
            if size > max_entry_bytes {
                return Err(DawprojectError::EntryTooLarge {
                    name,
                    size,
                    limit: max_entry_bytes,
                });
            }
            // The declared size may lie; never read past the limit.
            let mut data = Vec::with_capacity(usize::try_from(size).unwrap_or(0).min(1 << 26));
            (&mut file)
                .take(max_entry_bytes.saturating_add(1))
                .read_to_end(&mut data)
                .map_err(zip_error)?;
            if data.len() as u64 > max_entry_bytes {
                return Err(DawprojectError::EntryTooLarge {
                    name,
                    size: data.len() as u64,
                    limit: max_entry_bytes,
                });
            }
            entries.insert(name, data);
        }
        Ok(Self { entries })
    }

    /// Adds or replaces an entry. The name is normalized; a name that
    /// cannot be stored is reported as an error.
    pub fn insert(&mut self, name: &str, data: Vec<u8>) -> Result<(), DawprojectError> {
        let name = normalize_path(name)
            .ok_or_else(|| DawprojectError::Zip(format!("invalid entry name `{name}`")))?;
        self.entries.insert(name, data);
        Ok(())
    }

    /// An entry's contents.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        normalize_path(name).and_then(|name| self.entries.get(&name).map(Vec::as_slice))
    }

    /// Whether the archive holds this entry.
    pub fn contains(&self, name: &str) -> bool {
        self.get(name).is_some()
    }

    /// The entry names, sorted.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.entries.keys().map(String::as_str)
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the archive is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Writes the archive. `project.xml` and `metadata.xml` come first,
    /// then the rest by name, so the same entries always give the same bytes.
    pub fn to_bytes(&self) -> Result<Vec<u8>, DawprojectError> {
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        let first = [PROJECT_XML, METADATA_XML];
        let ordered = first
            .iter()
            .filter_map(|name| self.entries.get_key_value(*name))
            .chain(
                self.entries
                    .iter()
                    .filter(|(name, _)| !first.contains(&name.as_str())),
            );
        for (name, data) in ordered {
            let options = SimpleFileOptions::default()
                .compression_method(CompressionMethod::Deflated)
                .large_file(data.len() as u64 >= u64::from(u32::MAX));
            writer
                .start_file(name.as_str(), options)
                .map_err(zip_error)?;
            writer.write_all(data).map_err(zip_error)?;
        }
        Ok(writer.finish().map_err(zip_error)?.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_and_bad_names_are_refused() {
        let mut container = Container::new();
        container.insert("audio/a b.wav", vec![1, 2, 3]).unwrap();
        container
            .insert("./project.xml", b"<Project/>".to_vec())
            .unwrap();
        assert!(container.insert("../evil", Vec::new()).is_err());
        let bytes = container.to_bytes().unwrap();
        let back = Container::read(&bytes, 1 << 20).unwrap();
        assert_eq!(back, container);
        assert_eq!(back.get("project.xml"), Some(&b"<Project/>"[..]));
        assert_eq!(back.get("audio\\a b.wav"), Some(&[1, 2, 3][..]));
        assert_eq!(container.to_bytes().unwrap(), bytes, "deterministic");
    }

    #[test]
    fn oversized_entries_and_junk_are_errors() {
        let mut container = Container::new();
        container.insert("big.bin", vec![0; 4096]).unwrap();
        let bytes = container.to_bytes().unwrap();
        assert!(matches!(
            Container::read(&bytes, 1000),
            Err(DawprojectError::EntryTooLarge { size: 4096, .. })
        ));
        assert!(matches!(
            Container::read(b"not a zip", 1000),
            Err(DawprojectError::Zip(_))
        ));
    }
}
