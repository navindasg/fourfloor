//! Opening databases left by older and newer builds, and foreign files.

use fourfloor_library::schema::{migrate_to, APPLICATION_ID, SCHEMA_VERSION};
use fourfloor_library::{FileId, Library, LibraryError, Query};
use rusqlite::Connection;

/// A database as a version 1 build left it, with one file and a tag.
fn version_one(path: &std::path::Path) {
    let mut conn = Connection::open(path).expect("open");
    migrate_to(&mut conn, 1).expect("v1");
    conn.execute_batch(
        "INSERT INTO files (id, path, filename, title, content_hash, size_bytes, mtime_ns, kind,
            duration_seconds, sample_rate, channels, bpm, key, key_camelot, added_at,
            analyzed_at, analysis_version)
         VALUES (7, '/old/Kick.wav', 'Kick.wav', 'Kick', 'abc', 10, 1, 'one-shot', 0.5,
            44100, 2, NULL, NULL, NULL, 100, 100, 1);
         INSERT INTO tags (id, name) VALUES (1, 'kick');
         INSERT INTO file_tags (file_id, tag_id, source) VALUES (7, 1, 'user');",
    )
    .expect("old rows");
}

#[test]
fn an_old_database_is_upgraded_in_place_and_keeps_its_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("library.db");
    version_one(&path);
    let library = Library::open(&path).expect("open");
    assert_eq!(library.schema_version().expect("version"), SCHEMA_VERSION);
    let record = library.file(FileId(7)).expect("old row");
    assert_eq!(record.title, "Kick");
    assert!(record.has_tag("kick"));
    // The new column works.
    library
        .set_embedding(FileId(7), "m", &[1.0, 2.0])
        .expect("embedding");
    drop(library);
    // Reopening is a no-op.
    let library = Library::open(&path).expect("reopen");
    assert_eq!(
        library.embedding(FileId(7)).expect("get").map(|e| e.1),
        Some(vec![1.0, 2.0])
    );
    let conn = Connection::open(&path).expect("raw");
    let application: i64 = conn
        .pragma_query_value(None, "application_id", |row| row.get(0))
        .expect("id");
    assert_eq!(application, APPLICATION_ID);
}

#[test]
fn a_newer_database_is_refused_and_not_touched() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("future.db");
    drop(Library::open(&path).expect("create"));
    let conn = Connection::open(&path).expect("raw");
    conn.pragma_update(None, "user_version", SCHEMA_VERSION + 1)
        .expect("bump");
    drop(conn);
    match Library::open(&path) {
        Err(LibraryError::SchemaTooNew { found, supported }) => {
            assert_eq!((found, supported), (SCHEMA_VERSION + 1, SCHEMA_VERSION));
        }
        other => panic!("expected SchemaTooNew, got {other:?}"),
    }
    let conn = Connection::open(&path).expect("raw");
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .expect("version");
    assert_eq!(version, SCHEMA_VERSION + 1);
}

#[test]
fn foreign_files_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sqlite = dir.path().join("mixxxdb.sqlite");
    Connection::open(&sqlite)
        .expect("open")
        .execute_batch("CREATE TABLE library (id INTEGER PRIMARY KEY)")
        .expect("table");
    assert!(matches!(
        Library::open(&sqlite),
        Err(LibraryError::NotALibrary(_))
    ));

    let text = dir.path().join("notes.db");
    std::fs::write(
        &text,
        "this is not a database, just some text that is long enough",
    )
    .expect("write");
    assert!(matches!(
        Library::open(&text),
        Err(LibraryError::Database(_))
    ));
}

#[test]
fn a_new_library_on_disk_is_empty_and_persists() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested/folder/library.db");
    let library = Library::open(&path).expect("open");
    assert!(library.is_empty().expect("empty"));
    let id = library.create_crate("Keep").expect("crate");
    drop(library);
    let library = Library::open(&path).expect("reopen");
    assert_eq!(library.crates().expect("crates")[0].id, id);
    assert!(library.search(&Query::new()).expect("search").is_empty());
}
