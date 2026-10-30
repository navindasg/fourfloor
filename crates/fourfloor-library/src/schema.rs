//! The database schema and its migrations.
//!
//! The version lives in SQLite's `user_version`; `application_id` marks the
//! file as a fourfloor library. Opening a database runs every migration
//! newer than its version, each in its own transaction. A database from a
//! newer build is refused rather than guessed at.
//!
//! The layout follows Mixxx's: one row per file with its analysis, tags in
//! a join table, crates as unordered sets, playlists as ordered lists, and
//! a play history.

use rusqlite::Connection;

use crate::error::LibraryError;

/// `application_id` of a fourfloor library: the bytes `FFLB`.
pub const APPLICATION_ID: i64 = 0x4646_4C42;

/// The newest schema version this build reads and writes.
pub const SCHEMA_VERSION: i64 = MIGRATIONS.len() as i64;

/// Version of the analysis that fills a file row. Raising it makes the
/// next scan analyse every file again.
pub const ANALYSIS_VERSION: i64 = 1;

/// Migration `n` (1-based) takes a database from version `n - 1` to `n`.
/// Released migrations are never edited; changes go in a new one.
const MIGRATIONS: &[&str] = &[
    // 1: files, tags, play history, crates, playlists.
    "
    CREATE TABLE roots (
        id       INTEGER PRIMARY KEY,
        path     TEXT NOT NULL UNIQUE,
        added_at INTEGER NOT NULL
    );
    CREATE TABLE files (
        id                 INTEGER PRIMARY KEY,
        path               TEXT NOT NULL UNIQUE,
        filename           TEXT NOT NULL,
        title              TEXT NOT NULL,
        content_hash       TEXT NOT NULL,
        size_bytes         INTEGER NOT NULL,
        mtime_ns           INTEGER NOT NULL,
        kind               TEXT NOT NULL,
        duration_seconds   REAL NOT NULL,
        sample_rate        INTEGER NOT NULL,
        channels           INTEGER NOT NULL,
        bpm                REAL,
        bpm_confidence     REAL,
        first_beat_seconds REAL,
        key                INTEGER,
        key_camelot        INTEGER,
        key_confidence     REAL,
        loudness_lufs      REAL,
        true_peak_dbtp     REAL,
        waveform           BLOB,
        features           BLOB,
        rating             INTEGER NOT NULL DEFAULT 0,
        play_count         INTEGER NOT NULL DEFAULT 0,
        last_played_at     INTEGER,
        missing            INTEGER NOT NULL DEFAULT 0,
        locked             INTEGER NOT NULL DEFAULT 0,
        added_at           INTEGER NOT NULL,
        analyzed_at        INTEGER NOT NULL,
        analysis_version   INTEGER NOT NULL
    );
    CREATE INDEX files_hash ON files (content_hash);
    CREATE INDEX files_bpm ON files (bpm);
    CREATE INDEX files_kind ON files (kind);
    CREATE TABLE tags (
        id   INTEGER PRIMARY KEY,
        name TEXT NOT NULL UNIQUE
    );
    CREATE TABLE file_tags (
        file_id INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
        tag_id  INTEGER NOT NULL REFERENCES tags (id) ON DELETE CASCADE,
        source  TEXT NOT NULL DEFAULT 'auto',
        PRIMARY KEY (file_id, tag_id)
    );
    CREATE INDEX file_tags_tag ON file_tags (tag_id);
    CREATE TABLE play_history (
        id        INTEGER PRIMARY KEY,
        file_id   INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
        played_at INTEGER NOT NULL,
        context   TEXT NOT NULL DEFAULT ''
    );
    CREATE INDEX play_history_time ON play_history (played_at);
    CREATE TABLE crates (
        id         INTEGER PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE crate_files (
        crate_id INTEGER NOT NULL REFERENCES crates (id) ON DELETE CASCADE,
        file_id  INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE,
        PRIMARY KEY (crate_id, file_id)
    );
    CREATE TABLE playlists (
        id         INTEGER PRIMARY KEY,
        name       TEXT NOT NULL UNIQUE,
        created_at INTEGER NOT NULL
    );
    CREATE TABLE playlist_entries (
        id          INTEGER PRIMARY KEY,
        playlist_id INTEGER NOT NULL REFERENCES playlists (id) ON DELETE CASCADE,
        position    INTEGER NOT NULL,
        file_id     INTEGER NOT NULL REFERENCES files (id) ON DELETE CASCADE
    );
    CREATE INDEX playlist_entries_order ON playlist_entries (playlist_id, position);
    ",
    // 2: an embedding per file, filled by the ML crate for text search.
    "
    ALTER TABLE files ADD COLUMN embedding BLOB;
    ALTER TABLE files ADD COLUMN embedding_model TEXT;
    ",
];

/// The schema version of an open database (0 for a new, empty one).
pub fn version(conn: &Connection) -> Result<i64, LibraryError> {
    Ok(conn.pragma_query_value(None, "user_version", |row| row.get(0))?)
}

/// Brings a database up to [`SCHEMA_VERSION`], creating the schema in an
/// empty one. Returns the version it started from.
pub fn migrate(conn: &mut Connection) -> Result<i64, LibraryError> {
    migrate_to(conn, SCHEMA_VERSION)
}

/// Brings a database up to `target` and no further. Used by tests to make
/// databases as older builds left them.
pub fn migrate_to(conn: &mut Connection, target: i64) -> Result<i64, LibraryError> {
    let found = version(conn)?;
    let application: i64 = conn.pragma_query_value(None, "application_id", |row| row.get(0))?;
    if found > SCHEMA_VERSION {
        return Err(LibraryError::SchemaTooNew {
            found,
            supported: SCHEMA_VERSION,
        });
    }
    if found < 0 {
        return Err(LibraryError::NotALibrary(describe(conn)));
    }
    if found == 0 {
        let tables: i64 = conn.query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table'",
            [],
            |row| row.get(0),
        )?;
        if tables != 0 || (application != 0 && application != APPLICATION_ID) {
            return Err(LibraryError::NotALibrary(describe(conn)));
        }
    } else if application != APPLICATION_ID {
        return Err(LibraryError::NotALibrary(describe(conn)));
    }

    let target = target.clamp(0, SCHEMA_VERSION);
    for step in found..target {
        let Some(sql) = usize::try_from(step).ok().and_then(|i| MIGRATIONS.get(i)) else {
            break;
        };
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", step + 1)?;
        tx.pragma_update(None, "application_id", APPLICATION_ID)?;
        tx.commit()?;
    }
    Ok(found)
}

fn describe(conn: &Connection) -> String {
    conn.path()
        .filter(|path| !path.is_empty())
        .unwrap_or("the in-memory database")
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(conn: &Connection, table: &str) -> Vec<String> {
        let mut statement = conn
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .expect("prepare");
        let names = statement
            .query_map([], |row| row.get::<_, String>(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows");
        names
    }

    #[test]
    fn a_new_database_gets_the_current_schema() {
        let mut conn = Connection::open_in_memory().expect("open");
        assert_eq!(version(&conn).expect("version"), 0);
        assert_eq!(migrate(&mut conn).expect("migrate"), 0);
        assert_eq!(version(&conn).expect("version"), SCHEMA_VERSION);
        assert!(columns(&conn, "files").contains(&"embedding".to_owned()));
        // Running it again changes nothing.
        assert_eq!(migrate(&mut conn).expect("migrate"), SCHEMA_VERSION);
    }

    #[test]
    fn version_one_has_no_embedding_and_upgrades_in_place() {
        let mut conn = Connection::open_in_memory().expect("open");
        migrate_to(&mut conn, 1).expect("v1");
        assert_eq!(version(&conn).expect("version"), 1);
        assert!(!columns(&conn, "files").contains(&"embedding".to_owned()));
        assert_eq!(migrate(&mut conn).expect("migrate"), 1);
        assert!(columns(&conn, "files").contains(&"embedding_model".to_owned()));
    }

    #[test]
    fn a_newer_database_is_refused_untouched() {
        let mut conn = Connection::open_in_memory().expect("open");
        migrate(&mut conn).expect("migrate");
        conn.pragma_update(None, "user_version", SCHEMA_VERSION + 3)
            .expect("bump");
        match migrate(&mut conn) {
            Err(LibraryError::SchemaTooNew { found, supported }) => {
                assert_eq!(found, SCHEMA_VERSION + 3);
                assert_eq!(supported, SCHEMA_VERSION);
            }
            other => panic!("expected SchemaTooNew, got {other:?}"),
        }
        assert_eq!(version(&conn).expect("version"), SCHEMA_VERSION + 3);
    }

    #[test]
    fn a_foreign_database_is_refused() {
        let mut conn = Connection::open_in_memory().expect("open");
        conn.execute_batch("CREATE TABLE library (id INTEGER PRIMARY KEY)")
            .expect("foreign table");
        assert!(matches!(
            migrate(&mut conn),
            Err(LibraryError::NotALibrary(_))
        ));

        let mut versioned = Connection::open_in_memory().expect("open");
        versioned
            .pragma_update(None, "user_version", 1)
            .expect("version");
        assert!(matches!(
            migrate(&mut versioned),
            Err(LibraryError::NotALibrary(_))
        ));
    }
}
