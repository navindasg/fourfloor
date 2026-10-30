//! The library database: opening it, reading file rows, and the edits a
//! user makes to a file (tags, rating, hand-set tempo and key).

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use fourfloor_analysis::{FeatureVector, Key};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Row};

use crate::blob::{decode_features, WaveformSummary};
use crate::error::LibraryError;
use crate::schema;
use crate::tags::normalise_tag;
use crate::types::{
    camelot_order, key_code, key_from_code, FileId, FileKind, FileRecord, Tag, TagSource,
};

/// The columns of a file row as [`file_from_row`] reads them, for a query
/// over `files AS f`.
pub(crate) const FILE_COLUMNS: &str = "f.id, f.path, f.filename, f.title, f.content_hash, \
    f.size_bytes, f.mtime_ns, f.kind, f.duration_seconds, f.sample_rate, f.channels, f.bpm, \
    f.bpm_confidence, f.first_beat_seconds, f.key, f.key_confidence, f.loudness_lufs, \
    f.true_peak_dbtp, f.rating, f.play_count, f.last_played_at, f.missing, f.locked, \
    f.added_at, f.analyzed_at, \
    (SELECT group_concat(t.name || char(31) || ft.source, char(30)) \
       FROM file_tags ft JOIN tags t ON t.id = ft.tag_id WHERE ft.file_id = f.id)";

/// A sample and track library kept in one SQLite file.
///
/// A `Library` owns its connection and is used from one thread at a time
/// (it is `Send`, not `Sync`). Scanning analyses files on a worker pool but
/// writes every row from the calling thread.
pub struct Library {
    pub(crate) conn: Connection,
    path: Option<PathBuf>,
}

impl std::fmt::Debug for Library {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Library").field("path", &self.path).finish()
    }
}

/// Tempo, key and kind set by hand. Applying one locks the file, so a
/// later scan keeps these values; [`Library::unlock`] gives them back to
/// the analysis.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ManualAnalysis {
    /// Tempo in BPM; `None` clears it.
    pub bpm: Option<f64>,
    /// Key; `None` clears it.
    pub key: Option<Key>,
    /// Kind; `None` keeps the current one.
    pub kind: Option<FileKind>,
}

impl Library {
    /// Opens (or creates) the library at `path` and brings its schema up to
    /// date. A database from a newer fourfloor is refused with
    /// [`LibraryError::SchemaTooNew`] and left untouched.
    pub fn open(path: &Path) -> Result<Self, LibraryError> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).map_err(|error| LibraryError::io(parent, error))?;
        }
        let conn = Connection::open(path)?;
        // WAL lets a UI read while a scan writes; not available in memory.
        conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))?;
        Self::from_connection(conn, Some(path.to_path_buf()))
    }

    /// A library that lives only in memory, for tests and previews.
    pub fn open_in_memory() -> Result<Self, LibraryError> {
        Self::from_connection(Connection::open_in_memory()?, None)
    }

    fn from_connection(mut conn: Connection, path: Option<PathBuf>) -> Result<Self, LibraryError> {
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        schema::migrate(&mut conn)?;
        Ok(Self { conn, path })
    }

    /// Where the database file is; `None` for an in-memory library.
    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The schema version of the open database.
    pub fn schema_version(&self) -> Result<i64, LibraryError> {
        schema::version(&self.conn)
    }

    /// Number of files in the library, including missing ones.
    pub fn len(&self) -> Result<usize, LibraryError> {
        let count: i64 = self
            .conn
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Whether the library has no files.
    pub fn is_empty(&self) -> Result<bool, LibraryError> {
        Ok(self.len()? == 0)
    }

    // ---- roots ---------------------------------------------------------

    /// Remembers a folder so [`Library::rescan`] scans it again.
    pub fn add_root(&self, folder: &Path) -> Result<(), LibraryError> {
        self.conn.execute(
            "INSERT OR IGNORE INTO roots (path, added_at) VALUES (?1, ?2)",
            params![path_text(folder)?, now_unix()],
        )?;
        Ok(())
    }

    /// Forgets a folder. Its files stay in the library until
    /// [`Library::remove_files_under`] or [`Library::purge_missing`].
    pub fn remove_root(&self, folder: &Path) -> Result<bool, LibraryError> {
        let removed = self
            .conn
            .execute("DELETE FROM roots WHERE path = ?1", [path_text(folder)?])?;
        Ok(removed > 0)
    }

    /// The folders the library scans, in the order they were added.
    pub fn roots(&self) -> Result<Vec<PathBuf>, LibraryError> {
        let mut statement = self.conn.prepare("SELECT path FROM roots ORDER BY id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    // ---- file rows -----------------------------------------------------

    /// The file with this id.
    pub fn file(&self, id: FileId) -> Result<FileRecord, LibraryError> {
        self.conn
            .query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.id = ?1"),
                [id.0],
                file_from_row,
            )
            .optional()?
            .ok_or_else(|| LibraryError::not_found("file", id))
    }

    /// The file at this path, if it is in the library.
    pub fn file_by_path(&self, path: &Path) -> Result<Option<FileRecord>, LibraryError> {
        Ok(self
            .conn
            .query_row(
                &format!("SELECT {FILE_COLUMNS} FROM files f WHERE f.path = ?1"),
                [path_text(path)?],
                file_from_row,
            )
            .optional()?)
    }

    /// The stored waveform overview of a file.
    pub fn waveform(&self, id: FileId) -> Result<Option<WaveformSummary>, LibraryError> {
        let blob = self.blob(id, "waveform")?;
        blob.map(|bytes| WaveformSummary::decode(&bytes))
            .transpose()
    }

    /// The stored feature vector of a file.
    pub fn features(&self, id: FileId) -> Result<Option<FeatureVector>, LibraryError> {
        let blob = self.blob(id, "features")?;
        blob.map(|bytes| decode_features(&bytes)).transpose()
    }

    pub(crate) fn blob(&self, id: FileId, column: &str) -> Result<Option<Vec<u8>>, LibraryError> {
        // `column` is always one of our own column names, never user input.
        let blob: Option<Option<Vec<u8>>> = self
            .conn
            .query_row(
                &format!("SELECT {column} FROM files WHERE id = ?1"),
                [id.0],
                |row| row.get(0),
            )
            .optional()?;
        blob.ok_or_else(|| LibraryError::not_found("file", id))
    }

    /// Removes a file from the library (not from disk), with its tags,
    /// history and crate and playlist entries.
    pub fn remove_file(&self, id: FileId) -> Result<(), LibraryError> {
        let removed = self
            .conn
            .execute("DELETE FROM files WHERE id = ?1", [id.0])?;
        if removed == 0 {
            return Err(LibraryError::not_found("file", id));
        }
        Ok(())
    }

    /// Removes every file the last scan did not find. Returns how many.
    pub fn purge_missing(&self) -> Result<usize, LibraryError> {
        Ok(self
            .conn
            .execute("DELETE FROM files WHERE missing = 1", [])?)
    }

    /// Removes every file below `folder`. Returns how many.
    pub fn remove_files_under(&self, folder: &Path) -> Result<usize, LibraryError> {
        let ids: Vec<i64> = self
            .file_paths()?
            .into_iter()
            .filter(|(_, path)| path.starts_with(folder))
            .map(|(id, _)| id.0)
            .collect();
        let tx = self.conn.unchecked_transaction()?;
        for id in &ids {
            tx.execute("DELETE FROM files WHERE id = ?1", [id])?;
        }
        tx.commit()?;
        Ok(ids.len())
    }

    pub(crate) fn file_paths(&self) -> Result<Vec<(FileId, PathBuf)>, LibraryError> {
        let mut statement = self.conn.prepare("SELECT id, path FROM files")?;
        let rows = statement.query_map([], |row| {
            Ok((FileId(row.get(0)?), PathBuf::from(row.get::<_, String>(1)?)))
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ---- user edits ----------------------------------------------------

    /// Adds a user tag. User tags survive re-analysis.
    pub fn add_tag(&self, id: FileId, name: &str) -> Result<(), LibraryError> {
        let name = normalise_tag(name).ok_or_else(|| LibraryError::invalid("tag", "empty"))?;
        self.file_exists(id)?;
        let tag = ensure_tag(&self.conn, &name)?;
        self.conn.execute(
            "INSERT INTO file_tags (file_id, tag_id, source) VALUES (?1, ?2, 'user')
             ON CONFLICT (file_id, tag_id) DO UPDATE SET source = 'user'",
            params![id.0, tag],
        )?;
        Ok(())
    }

    /// Removes a tag, whoever added it. Returns whether the file had it.
    /// An automatic tag removed this way comes back on the next analysis.
    pub fn remove_tag(&self, id: FileId, name: &str) -> Result<bool, LibraryError> {
        let Some(name) = normalise_tag(name) else {
            return Ok(false);
        };
        let removed = self.conn.execute(
            "DELETE FROM file_tags WHERE file_id = ?1
               AND tag_id = (SELECT id FROM tags WHERE name = ?2)",
            params![id.0, name],
        )?;
        Ok(removed > 0)
    }

    /// Every tag in use and how many files carry it, by name.
    pub fn all_tags(&self) -> Result<Vec<(String, usize)>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT t.name, count(*) FROM tags t JOIN file_tags ft ON ft.tag_id = t.id
             GROUP BY t.id ORDER BY t.name",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        Ok(rows
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(name, count)| (name, usize::try_from(count).unwrap_or(0)))
            .collect())
    }

    /// Sets the star rating, 0 to 5.
    pub fn set_rating(&self, id: FileId, rating: u8) -> Result<(), LibraryError> {
        if rating > 5 {
            return Err(LibraryError::invalid(
                "rating",
                format!("{rating} is above 5"),
            ));
        }
        let changed = self.conn.execute(
            "UPDATE files SET rating = ?2 WHERE id = ?1",
            params![id.0, rating],
        )?;
        changed_one(changed, id)
    }

    /// Sets tempo, key and kind by hand and locks them against re-analysis.
    pub fn set_manual_analysis(
        &self,
        id: FileId,
        manual: &ManualAnalysis,
    ) -> Result<(), LibraryError> {
        if let Some(bpm) = manual.bpm {
            if !(bpm.is_finite() && (20.0..=999.0).contains(&bpm)) {
                return Err(LibraryError::invalid("tempo", format!("{bpm} BPM")));
            }
        }
        let current = self.file(id)?;
        let kind = manual.kind.unwrap_or(current.kind);
        self.conn.execute(
            "UPDATE files SET bpm = ?2, bpm_confidence = NULL, key = ?3, key_camelot = ?4,
                key_confidence = NULL, kind = ?5, locked = 1 WHERE id = ?1",
            params![
                id.0,
                manual.bpm,
                manual.key.map(key_code),
                manual.key.map(camelot_order),
                kind.as_str()
            ],
        )?;
        Ok(())
    }

    /// Lets the next scan analyse tempo, key and kind again.
    pub fn unlock(&self, id: FileId) -> Result<(), LibraryError> {
        let changed = self
            .conn
            .execute("UPDATE files SET locked = 0 WHERE id = ?1", [id.0])?;
        changed_one(changed, id)
    }

    pub(crate) fn file_exists(&self, id: FileId) -> Result<(), LibraryError> {
        let found: Option<i64> = self
            .conn
            .query_row("SELECT id FROM files WHERE id = ?1", [id.0], |row| {
                row.get(0)
            })
            .optional()?;
        found
            .map(|_| ())
            .ok_or_else(|| LibraryError::not_found("file", id))
    }
}

fn changed_one(changed: usize, id: FileId) -> Result<(), LibraryError> {
    if changed == 0 {
        Err(LibraryError::not_found("file", id))
    } else {
        Ok(())
    }
}

/// The id of tag `name`, created if new. `name` must be normalised.
pub(crate) fn ensure_tag(conn: &Connection, name: &str) -> Result<i64, LibraryError> {
    conn.execute("INSERT OR IGNORE INTO tags (name) VALUES (?1)", [name])?;
    Ok(
        conn.query_row("SELECT id FROM tags WHERE name = ?1", [name], |row| {
            row.get(0)
        })?,
    )
}

/// A path as stored: absolute UTF-8 text.
pub(crate) fn path_text(path: &Path) -> Result<String, LibraryError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| LibraryError::NonUtf8Path(path.to_path_buf()))
}

/// Seconds since the Unix epoch.
pub(crate) fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

fn conversion_error(index: usize, error: LibraryError) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
}

/// Reads a row selected with [`FILE_COLUMNS`].
pub(crate) fn file_from_row(row: &Row<'_>) -> rusqlite::Result<FileRecord> {
    let kind_text: String = row.get(7)?;
    let kind = kind_text
        .parse::<FileKind>()
        .map_err(|error| conversion_error(7, error))?;
    let key = match row.get::<_, Option<i64>>(14)? {
        None => None,
        Some(code) => Some(key_from_code(code).ok_or_else(|| {
            conversion_error(
                14,
                LibraryError::UnknownValue {
                    column: "key",
                    value: code.to_string(),
                },
            )
        })?),
    };
    let tag_list: Option<String> = row.get(25)?;
    let mut tags: Vec<Tag> = tag_list
        .as_deref()
        .unwrap_or("")
        .split('\u{1e}')
        .filter(|entry| !entry.is_empty())
        .map(|entry| {
            let (name, source) = entry.split_once('\u{1f}').unwrap_or((entry, "auto"));
            Tag {
                name: name.to_owned(),
                source: TagSource::parse(source),
            }
        })
        .collect();
    tags.sort_by(|a, b| a.name.cmp(&b.name));
    let rating: i64 = row.get(18)?;
    let play_count: i64 = row.get(19)?;
    Ok(FileRecord {
        id: FileId(row.get(0)?),
        path: PathBuf::from(row.get::<_, String>(1)?),
        filename: row.get(2)?,
        title: row.get(3)?,
        content_hash: row.get(4)?,
        size_bytes: u64::try_from(row.get::<_, i64>(5)?).unwrap_or(0),
        mtime_ns: row.get(6)?,
        kind,
        duration_seconds: row.get(8)?,
        sample_rate: u32::try_from(row.get::<_, i64>(9)?).unwrap_or(0),
        channels: u16::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
        bpm: row.get(11)?,
        bpm_confidence: row.get::<_, Option<f64>>(12)?.map(|v| v as f32),
        first_beat_seconds: row.get(13)?,
        key,
        key_confidence: row.get::<_, Option<f64>>(15)?.map(|v| v as f32),
        loudness_lufs: row.get(16)?,
        true_peak_dbtp: row.get(17)?,
        tags,
        rating: u8::try_from(rating.clamp(0, 5)).unwrap_or(0),
        play_count: u32::try_from(play_count.max(0)).unwrap_or(u32::MAX),
        last_played_at: row.get(20)?,
        missing: row.get::<_, i64>(21)? != 0,
        locked: row.get::<_, i64>(22)? != 0,
        added_at: row.get(23)?,
        analyzed_at: row.get(24)?,
    })
}
