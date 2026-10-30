//! Crates (unordered sets), playlists (ordered lists), the play history
//! and the embedding column the ML crate fills.

use rusqlite::{params, OptionalExtension};

use crate::blob::{decode_f32s, encode_f32s};
use crate::error::LibraryError;
use crate::library::{file_from_row, now_unix, Library, FILE_COLUMNS};
use crate::types::{Collection, CrateId, FileId, FileRecord, PlayEvent, PlaylistId};

fn clean_name(name: &str) -> Result<String, LibraryError> {
    let name = name.trim();
    if name.is_empty() {
        Err(LibraryError::invalid("name", "empty"))
    } else {
        Ok(name.to_owned())
    }
}

fn is_unique_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(failure, _)
            if failure.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn count(value: i64) -> usize {
    usize::try_from(value).unwrap_or(0)
}

impl Library {
    // ---- crates --------------------------------------------------------

    /// Creates a crate. Names are unique.
    pub fn create_crate(&self, name: &str) -> Result<CrateId, LibraryError> {
        let name = clean_name(name)?;
        match self.conn.execute(
            "INSERT INTO crates (name, created_at) VALUES (?1, ?2)",
            params![name, now_unix()],
        ) {
            Ok(_) => Ok(CrateId(self.conn.last_insert_rowid())),
            Err(error) if is_unique_violation(&error) => Err(LibraryError::invalid(
                "crate name",
                format!("{name:?} is taken"),
            )),
            Err(error) => Err(error.into()),
        }
    }

    /// Renames a crate.
    pub fn rename_crate(&self, id: CrateId, name: &str) -> Result<(), LibraryError> {
        let name = clean_name(name)?;
        match self.conn.execute(
            "UPDATE crates SET name = ?2 WHERE id = ?1",
            params![id.0, name],
        ) {
            Ok(0) => Err(LibraryError::not_found("crate", id.0)),
            Ok(_) => Ok(()),
            Err(error) if is_unique_violation(&error) => Err(LibraryError::invalid(
                "crate name",
                format!("{name:?} is taken"),
            )),
            Err(error) => Err(error.into()),
        }
    }

    /// Deletes a crate (not its files).
    pub fn delete_crate(&self, id: CrateId) -> Result<(), LibraryError> {
        match self
            .conn
            .execute("DELETE FROM crates WHERE id = ?1", [id.0])?
        {
            0 => Err(LibraryError::not_found("crate", id.0)),
            _ => Ok(()),
        }
    }

    /// Every crate, by name.
    pub fn crates(&self) -> Result<Vec<Collection<CrateId>>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT c.id, c.name, (SELECT count(*) FROM crate_files cf WHERE cf.crate_id = c.id)
             FROM crates c ORDER BY c.name",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Collection {
                id: CrateId(row.get(0)?),
                name: row.get(1)?,
                len: count(row.get(2)?),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The crate called `name`, if any.
    pub fn crate_by_name(&self, name: &str) -> Result<Option<CrateId>, LibraryError> {
        Ok(self
            .conn
            .query_row(
                "SELECT id FROM crates WHERE name = ?1",
                [name.trim()],
                |row| row.get(0).map(CrateId),
            )
            .optional()?)
    }

    /// Puts a file in a crate. Adding it twice changes nothing.
    pub fn add_to_crate(&self, id: CrateId, file: FileId) -> Result<(), LibraryError> {
        self.crate_exists(id)?;
        self.file_exists(file)?;
        self.conn.execute(
            "INSERT OR IGNORE INTO crate_files (crate_id, file_id) VALUES (?1, ?2)",
            params![id.0, file.0],
        )?;
        Ok(())
    }

    /// Takes a file out of a crate. Returns whether it was in it.
    pub fn remove_from_crate(&self, id: CrateId, file: FileId) -> Result<bool, LibraryError> {
        Ok(self.conn.execute(
            "DELETE FROM crate_files WHERE crate_id = ?1 AND file_id = ?2",
            params![id.0, file.0],
        )? > 0)
    }

    /// The files in a crate, by title.
    pub fn crate_files(&self, id: CrateId) -> Result<Vec<FileRecord>, LibraryError> {
        self.crate_exists(id)?;
        let mut statement = self.conn.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM files f JOIN crate_files cf ON cf.file_id = f.id
             WHERE cf.crate_id = ?1 ORDER BY f.title COLLATE NOCASE, f.id"
        ))?;
        let rows = statement.query_map([id.0], file_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn crate_exists(&self, id: CrateId) -> Result<(), LibraryError> {
        let found: Option<i64> = self
            .conn
            .query_row("SELECT id FROM crates WHERE id = ?1", [id.0], |row| {
                row.get(0)
            })
            .optional()?;
        found
            .map(|_| ())
            .ok_or_else(|| LibraryError::not_found("crate", id.0))
    }

    // ---- playlists -----------------------------------------------------

    /// Creates an empty playlist. Names are unique.
    pub fn create_playlist(&self, name: &str) -> Result<PlaylistId, LibraryError> {
        let name = clean_name(name)?;
        match self.conn.execute(
            "INSERT INTO playlists (name, created_at) VALUES (?1, ?2)",
            params![name, now_unix()],
        ) {
            Ok(_) => Ok(PlaylistId(self.conn.last_insert_rowid())),
            Err(error) if is_unique_violation(&error) => Err(LibraryError::invalid(
                "playlist name",
                format!("{name:?} is taken"),
            )),
            Err(error) => Err(error.into()),
        }
    }

    /// Deletes a playlist (not its files).
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<(), LibraryError> {
        match self
            .conn
            .execute("DELETE FROM playlists WHERE id = ?1", [id.0])?
        {
            0 => Err(LibraryError::not_found("playlist", id.0)),
            _ => Ok(()),
        }
    }

    /// Every playlist, by name.
    pub fn playlists(&self) -> Result<Vec<Collection<PlaylistId>>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT p.id, p.name,
                    (SELECT count(*) FROM playlist_entries e WHERE e.playlist_id = p.id)
             FROM playlists p ORDER BY p.name",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(Collection {
                id: PlaylistId(row.get(0)?),
                name: row.get(1)?,
                len: count(row.get(2)?),
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// The files of a playlist in order. A file can appear more than once.
    pub fn playlist_files(&self, id: PlaylistId) -> Result<Vec<FileRecord>, LibraryError> {
        self.playlist_len(id)?;
        let mut statement = self.conn.prepare(&format!(
            "SELECT {FILE_COLUMNS} FROM playlist_entries e JOIN files f ON f.id = e.file_id
             WHERE e.playlist_id = ?1 ORDER BY e.position"
        ))?;
        let rows = statement.query_map([id.0], file_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Inserts a file at `position` (0 is the top); positions past the end
    /// append.
    pub fn insert_into_playlist(
        &self,
        id: PlaylistId,
        position: usize,
        file: FileId,
    ) -> Result<(), LibraryError> {
        let len = self.playlist_len(id)?;
        self.file_exists(file)?;
        let position = position.min(len) as i64;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE playlist_entries SET position = position + 1
             WHERE playlist_id = ?1 AND position >= ?2",
            params![id.0, position],
        )?;
        tx.execute(
            "INSERT INTO playlist_entries (playlist_id, position, file_id) VALUES (?1, ?2, ?3)",
            params![id.0, position, file.0],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Appends a file to a playlist.
    pub fn append_to_playlist(&self, id: PlaylistId, file: FileId) -> Result<(), LibraryError> {
        self.insert_into_playlist(id, usize::MAX, file)
    }

    /// Removes the entry at `position`.
    pub fn remove_from_playlist(
        &self,
        id: PlaylistId,
        position: usize,
    ) -> Result<(), LibraryError> {
        let len = self.playlist_len(id)?;
        if position >= len {
            return Err(LibraryError::invalid(
                "playlist position",
                format!("{position} is past the end ({len} entries)"),
            ));
        }
        let position = position as i64;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "DELETE FROM playlist_entries WHERE playlist_id = ?1 AND position = ?2",
            params![id.0, position],
        )?;
        tx.execute(
            "UPDATE playlist_entries SET position = position - 1
             WHERE playlist_id = ?1 AND position > ?2",
            params![id.0, position],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Moves the entry at `from` so it ends up at `to`.
    pub fn move_in_playlist(
        &self,
        id: PlaylistId,
        from: usize,
        to: usize,
    ) -> Result<(), LibraryError> {
        let len = self.playlist_len(id)?;
        if from >= len || to >= len {
            return Err(LibraryError::invalid(
                "playlist position",
                format!("{from} -> {to} with {len} entries"),
            ));
        }
        let (from, to) = (from as i64, to as i64);
        let tx = self.conn.unchecked_transaction()?;
        let entry: i64 = tx.query_row(
            "SELECT id FROM playlist_entries WHERE playlist_id = ?1 AND position = ?2",
            params![id.0, from],
            |row| row.get(0),
        )?;
        if from < to {
            tx.execute(
                "UPDATE playlist_entries SET position = position - 1
                 WHERE playlist_id = ?1 AND position > ?2 AND position <= ?3",
                params![id.0, from, to],
            )?;
        } else {
            tx.execute(
                "UPDATE playlist_entries SET position = position + 1
                 WHERE playlist_id = ?1 AND position >= ?3 AND position < ?2",
                params![id.0, from, to],
            )?;
        }
        tx.execute(
            "UPDATE playlist_entries SET position = ?2 WHERE id = ?1",
            params![entry, to],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn playlist_len(&self, id: PlaylistId) -> Result<usize, LibraryError> {
        let found: Option<i64> = self
            .conn
            .query_row(
                "SELECT (SELECT count(*) FROM playlist_entries WHERE playlist_id = p.id)
                 FROM playlists p WHERE p.id = ?1",
                [id.0],
                |row| row.get(0),
            )
            .optional()?;
        found
            .map(count)
            .ok_or_else(|| LibraryError::not_found("playlist", id.0))
    }

    // ---- play history --------------------------------------------------

    /// Records that a file was played now, from `context` (`preview`,
    /// `deck-a`, a project name).
    pub fn record_play(&self, file: FileId, context: &str) -> Result<(), LibraryError> {
        self.record_play_at(file, context, now_unix())
    }

    /// [`Self::record_play`] with an explicit time (Unix seconds).
    pub fn record_play_at(&self, file: FileId, context: &str, at: i64) -> Result<(), LibraryError> {
        self.file_exists(file)?;
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO play_history (file_id, played_at, context) VALUES (?1, ?2, ?3)",
            params![file.0, at, context],
        )?;
        tx.execute(
            "UPDATE files SET play_count = play_count + 1,
                last_played_at = max(coalesce(last_played_at, ?2), ?2) WHERE id = ?1",
            params![file.0, at],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// The latest plays, newest first.
    pub fn play_history(&self, limit: usize) -> Result<Vec<PlayEvent>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT file_id, played_at, context FROM play_history
             ORDER BY played_at DESC, id DESC LIMIT ?1",
        )?;
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        let rows = statement.query_map([limit], |row| {
            Ok(PlayEvent {
                file: FileId(row.get(0)?),
                played_at: row.get(1)?,
                context: row.get(2)?,
            })
        })?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    // ---- embeddings ----------------------------------------------------

    /// Stores an embedding made by `model` (for example a text-audio
    /// model). Replaces any earlier one.
    pub fn set_embedding(
        &self,
        file: FileId,
        model: &str,
        vector: &[f32],
    ) -> Result<(), LibraryError> {
        let model = clean_name(model)?;
        if vector.is_empty() || vector.iter().any(|value| !value.is_finite()) {
            return Err(LibraryError::invalid(
                "embedding",
                "must be non-empty and finite",
            ));
        }
        let changed = self.conn.execute(
            "UPDATE files SET embedding = ?2, embedding_model = ?3 WHERE id = ?1",
            params![file.0, encode_f32s(vector), model],
        )?;
        if changed == 0 {
            return Err(LibraryError::not_found("file", file));
        }
        Ok(())
    }

    /// The stored embedding and the model that made it.
    pub fn embedding(&self, file: FileId) -> Result<Option<(String, Vec<f32>)>, LibraryError> {
        let row: Option<(Option<String>, Option<Vec<u8>>)> = self
            .conn
            .query_row(
                "SELECT embedding_model, embedding FROM files WHERE id = ?1",
                [file.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match row {
            None => Err(LibraryError::not_found("file", file)),
            Some((Some(model), Some(bytes))) => Ok(Some((model, decode_f32s(&bytes)?))),
            Some(_) => Ok(None),
        }
    }

    /// Files that have no embedding from `model` yet: the ML crate's to-do
    /// list.
    pub fn files_without_embedding(&self, model: &str) -> Result<Vec<FileId>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT id FROM files WHERE missing = 0
               AND (embedding IS NULL OR embedding_model IS NOT ?1) ORDER BY id",
        )?;
        let rows = statement.query_map([model], |row| row.get(0).map(FileId))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub(crate) fn embeddings_for(
        &self,
        model: &str,
    ) -> Result<Vec<(FileId, Vec<f32>)>, LibraryError> {
        let mut statement = self.conn.prepare(
            "SELECT id, embedding FROM files WHERE embedding_model = ?1 AND embedding IS NOT NULL",
        )?;
        let rows = statement.query_map([model], |row| {
            Ok((FileId(row.get(0)?), row.get::<_, Vec<u8>>(1)?))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .map(|(id, bytes)| Ok((id, decode_f32s(&bytes)?)))
            .collect()
    }
}
