//! Searching a scanned library of rendered sounds, and the user's
//! collections.

mod common;

use common::{scanned, stored, write_material};
use fourfloor_analysis::Key;
use fourfloor_library::{FileKind, Library, LibraryError, Query, SortKey};

fn names(records: &[fourfloor_library::FileRecord]) -> Vec<String> {
    records.iter().map(|r| r.filename.clone()).collect()
}

fn setup() -> (tempfile::TempDir, Library) {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (library, _) = scanned(dir.path());
    (dir, library)
}

#[test]
fn filters_by_kind_tempo_key_and_tags() {
    let (_dir, library) = setup();
    let loops = library
        .search(&Query::new().kind(FileKind::Loop))
        .expect("search");
    assert_eq!(names(&loops), ["Bass Line 122bpm Am.wav", "groove_a.wav"]);

    let near_128 = library
        .search(&Query::new().bpm_range(126.0, 130.0))
        .expect("search");
    assert_eq!(names(&near_128), ["groove_a.wav"]);
    // 61-65 matches 122 and 128 only at double speed.
    let half = Query::new().bpm_range(61.0, 65.0);
    assert!(library.search(&half).expect("search").is_empty());
    assert_eq!(
        library.search(&half.half_double()).expect("search").len(),
        2
    );

    let a_minor = library
        .search(&Query::new().key(Key::minor(9)))
        .expect("search");
    assert_eq!(names(&a_minor), ["Bass Line 122bpm Am.wav"]);
    // C# minor mixes with F# minor (a fifth up): the texture comes back.
    let compatible = library
        .search(&Query::new().compatible_with(Key::minor(1)))
        .expect("search");
    assert_eq!(names(&compatible), ["texture.wav"]);

    let kicks = library.search(&Query::new().tag("kick")).expect("search");
    assert_eq!(kicks.len(), 3, "{:?}", names(&kicks));
    let one_shot_drums = Query::new()
        .tag("drums")
        .kind(FileKind::OneShot)
        .without_tag("kick");
    assert_eq!(
        names(&library.search(&one_shot_drums).expect("search")),
        ["Clap.wav", "Hat Open.wav", "sample_003.wav"]
    );
    assert_eq!(library.count(&one_shot_drums).expect("count"), 3);
    let short = library
        .search(&Query::new().max_duration(0.5))
        .expect("search");
    assert_eq!(names(&short), ["sample_003.wav"]);
}

#[test]
fn text_matches_names_folders_and_tags_word_by_word() {
    let (_dir, library) = setup();
    // "sample_001" is a kick only by its sound; the tag finds it.
    let kicks = library.search(&Query::new().text("KICK")).expect("search");
    assert_eq!(kicks.len(), 3);
    let soft = library
        .search(&Query::new().text("kick soft"))
        .expect("search");
    assert_eq!(names(&soft), ["Kick Soft.wav"]);
    let folder = library
        .search(&Query::new().text("loops/"))
        .expect("search");
    assert_eq!(folder.len(), 2);
    // LIKE wildcards are literal.
    assert!(library
        .search(&Query::new().text("%"))
        .expect("search")
        .is_empty());
    assert!(
        library
            .search(&Query::new().text("sample_00"))
            .expect("search")
            .len()
            == 3
    );
}

#[test]
fn sorting_puts_unknowns_last_and_pages() {
    let (_dir, library) = setup();
    let by_bpm = library
        .search(&Query::new().sort_by(SortKey::Bpm))
        .expect("search");
    assert_eq!(by_bpm[0].bpm, Some(122.0));
    assert!(by_bpm[1].bpm.is_some_and(|bpm| (bpm - 128.0).abs() < 0.1));
    assert!(by_bpm[2..].iter().all(|r| r.bpm.is_none()));
    let descending = library
        .search(&Query::new().sort_by(SortKey::Bpm).descending())
        .expect("search");
    assert!(descending[0].bpm.is_some_and(|bpm| bpm > 127.0));
    assert!(descending.last().is_some_and(|r| r.bpm.is_none()));

    let by_length = library
        .search(&Query::new().sort_by(SortKey::Duration))
        .expect("search");
    assert!(by_length
        .windows(2)
        .all(|pair| pair[0].duration_seconds <= pair[1].duration_seconds));

    let all = library.search(&Query::new()).expect("search");
    let page = library
        .search(&Query::new().offset(2).limit(3))
        .expect("search");
    assert_eq!(names(&page), names(&all[2..5]));
    assert_eq!(
        library.count(&Query::new().limit(1)).expect("count"),
        all.len()
    );

    let by_key = library
        .search(&Query::new().sort_by(SortKey::Key))
        .expect("search");
    // F# minor is 11A, A minor 8A.
    assert_eq!(
        names(&by_key[..2]),
        ["Bass Line 122bpm Am.wav", "texture.wav"]
    );
}

#[test]
fn user_tags_ratings_and_history() {
    let (dir, library) = setup();
    let clap = library
        .file_by_path(&stored(dir.path(), "drums/Clap.wav"))
        .expect("lookup")
        .expect("clap");
    library.add_tag(clap.id, "  Big Room ").expect("tag");
    assert!(library.add_tag(clap.id, "   ").is_err());
    let tagged = library
        .search(&Query::new().tag("big room"))
        .expect("search");
    assert_eq!(names(&tagged), ["Clap.wav"]);
    assert!(library
        .all_tags()
        .expect("tags")
        .contains(&("big room".to_owned(), 1)));
    assert!(library.remove_tag(clap.id, "Big Room").expect("untag"));
    assert!(!library.remove_tag(clap.id, "big room").expect("untag"));

    library.set_rating(clap.id, 5).expect("rate");
    assert!(library.set_rating(clap.id, 6).is_err());
    assert_eq!(
        names(&library.search(&Query::new().min_rating(4)).expect("s")),
        ["Clap.wav"]
    );

    library
        .record_play_at(clap.id, "preview", 100)
        .expect("play");
    library
        .record_play_at(clap.id, "deck-a", 300)
        .expect("play");
    library
        .record_play_at(clap.id, "deck-b", 200)
        .expect("play");
    let record = library.file(clap.id).expect("file");
    assert_eq!((record.play_count, record.last_played_at), (3, Some(300)));
    let history = library.play_history(2).expect("history");
    assert_eq!(
        history
            .iter()
            .map(|e| (e.played_at, e.context.as_str()))
            .collect::<Vec<_>>(),
        [(300, "deck-a"), (200, "deck-b")]
    );
    let played = library
        .search(&Query::new().sort_by(SortKey::LastPlayed).descending())
        .expect("search");
    assert_eq!(played[0].id, clap.id);
    assert!(matches!(
        library.record_play(fourfloor_library::FileId(9_999), "x"),
        Err(LibraryError::NotFound { .. })
    ));
}

#[test]
fn crates_and_playlists() {
    let (_dir, library) = setup();
    let all = library.search(&Query::new()).expect("search");
    let (a, b, c) = (all[0].id, all[1].id, all[2].id);

    let favourites = library.create_crate("Favourites").expect("crate");
    assert!(library.create_crate("Favourites").is_err());
    assert!(library.create_crate(" ").is_err());
    library.add_to_crate(favourites, a).expect("add");
    library.add_to_crate(favourites, a).expect("add twice");
    library.add_to_crate(favourites, c).expect("add");
    assert_eq!(library.crate_files(favourites).expect("files").len(), 2);
    assert_eq!(library.crates().expect("crates")[0].len, 2);
    let in_crate = library
        .search(&Query::new().in_crate(favourites))
        .expect("search");
    assert_eq!(in_crate.len(), 2);
    assert!(library.remove_from_crate(favourites, a).expect("remove"));
    library.rename_crate(favourites, "Keepers").expect("rename");
    assert_eq!(
        library.crate_by_name("Keepers").expect("find"),
        Some(favourites)
    );
    library.delete_crate(favourites).expect("delete");
    assert!(library.crates().expect("crates").is_empty());

    let set = library.create_playlist("Warm-up").expect("playlist");
    library.append_to_playlist(set, a).expect("append");
    library.append_to_playlist(set, b).expect("append");
    library.insert_into_playlist(set, 0, c).expect("insert");
    library.append_to_playlist(set, a).expect("again");
    let order = |library: &Library| {
        library
            .playlist_files(set)
            .expect("files")
            .iter()
            .map(|r| r.id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(&library), [c, a, b, a]);
    library.move_in_playlist(set, 0, 3).expect("move");
    assert_eq!(order(&library), [a, b, a, c]);
    library.move_in_playlist(set, 3, 1).expect("move");
    assert_eq!(order(&library), [a, c, b, a]);
    library.remove_from_playlist(set, 0).expect("remove");
    assert_eq!(order(&library), [c, b, a]);
    assert!(library.remove_from_playlist(set, 3).is_err());
    assert_eq!(library.playlists().expect("lists")[0].len, 3);
    // Removing a file takes it out of every playlist.
    library.remove_file(b).expect("remove file");
    assert_eq!(order(&library), [c, a]);
    library.delete_playlist(set).expect("delete");
}

#[test]
fn embeddings_from_another_model_search_by_cosine() {
    let (_dir, library) = setup();
    let all = library.search(&Query::new()).expect("search");
    assert_eq!(
        library
            .files_without_embedding("clap-text")
            .expect("todo")
            .len(),
        all.len()
    );
    library
        .set_embedding(all[0].id, "clap-text", &[1.0, 0.0, 0.0])
        .expect("set");
    library
        .set_embedding(all[1].id, "clap-text", &[0.7, 0.7, 0.0])
        .expect("set");
    library
        .set_embedding(all[2].id, "clap-text", &[0.0, 0.0, 1.0])
        .expect("set");
    library
        .set_embedding(all[3].id, "other", &[1.0, 0.0, 0.0])
        .expect("set");
    assert!(library
        .set_embedding(all[4].id, "clap-text", &[f32::NAN])
        .is_err());
    assert!(library.set_embedding(all[4].id, "clap-text", &[]).is_err());

    let found = library
        .search_embedding("clap-text", &[1.0, 0.1, 0.0], &Query::new(), 10)
        .expect("search");
    let ids: Vec<_> = found.iter().map(|m| m.file.id).collect();
    assert_eq!(ids, [all[0].id, all[1].id, all[2].id]);
    assert!(found[0].score > 0.99 && found[2].score.abs() < 1e-6);
    assert_eq!(
        library.embedding(all[1].id).expect("get"),
        Some(("clap-text".to_owned(), vec![0.7, 0.7, 0.0]))
    );
    assert_eq!(
        library
            .files_without_embedding("clap-text")
            .expect("todo")
            .len(),
        all.len() - 3
    );
}
