//! Similarity ordering on rendered drums.

mod common;

use common::{scanned, stored, write, write_material};
use fourfloor_library::{FileKind, Query};

#[test]
fn kicks_are_nearer_kicks_than_hats() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (library, _) = scanned(dir.path());
    let kick = library
        .file_by_path(&stored(dir.path(), "drums/sample_001.wav"))
        .expect("lookup")
        .expect("kick");
    let ranked = library
        .similar(kick.id, &Query::new(), 100)
        .expect("similar");
    assert!(
        ranked.iter().all(|m| m.file.id != kick.id),
        "the file itself is left out"
    );
    assert!(ranked.windows(2).all(|pair| pair[0].score >= pair[1].score));
    let names: Vec<&str> = ranked.iter().map(|m| m.file.filename.as_str()).collect();
    // The two other kicks come first, the hats are below every kick.
    let mut top2 = names[..2].to_vec();
    top2.sort_unstable();
    assert_eq!(top2, ["Kick Soft.wav", "sample_002.wav"], "{names:?}");
    let position = |name: &str| names.iter().position(|n| *n == name).expect(name);
    for hat in ["sample_003.wav", "Hat Open.wav"] {
        assert!(position(hat) > position("sample_002.wav"), "{names:?}");
        assert!(position(hat) > position("Kick Soft.wav"), "{names:?}");
    }
    // Level does not matter much: the quieter render of the same kick
    // scores near 1.
    let soft = ranked
        .iter()
        .find(|m| m.file.filename == "Kick Soft.wav")
        .expect("soft");
    assert!(soft.score > 0.95, "{}", soft.score);
    let hat_score = ranked
        .iter()
        .find(|m| m.file.filename == "Hat Open.wav")
        .expect("hat")
        .score;
    assert!(hat_score < 0.0, "{hat_score}");
}

#[test]
fn hats_find_hats_and_filters_apply() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    write(dir.path(), "drums/sample_004.wav", &common::hat(42, 0.4));
    let (library, _) = scanned(dir.path());
    let hat = library
        .file_by_path(&stored(dir.path(), "drums/sample_003.wav"))
        .expect("lookup")
        .expect("hat");
    let ranked = library.similar(hat.id, &Query::new(), 2).expect("similar");
    let mut names: Vec<&str> = ranked.iter().map(|m| m.file.filename.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["Hat Open.wav", "sample_004.wav"]);

    // Only loops: the drum loop beats the bass loop for a hat.
    let loops = library
        .similar(hat.id, &Query::new().kind(FileKind::Loop), 10)
        .expect("similar");
    assert_eq!(loops.len(), 2);
    assert_eq!(loops[0].file.filename, "groove_a.wav");
}

#[test]
fn a_sound_outside_the_library_can_be_matched() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_material(dir.path());
    let (library, _) = scanned(dir.path());
    let outside = common::kick_808();
    let file = fourfloor_library::analyse_buffer(
        "dropped in.wav",
        &fourfloor_audio::AudioBuffer::new(outside.sample_rate, vec![outside.left, outside.right]),
    )
    .expect("analyse");
    let features = fourfloor_library::blob::decode_features(&file.features).expect("features");
    let ranked = library
        .similar_to_features(&features, &Query::new(), 1, None)
        .expect("similar");
    assert_eq!(ranked[0].file.filename, "sample_002.wav");
    assert!(ranked[0].score > 0.999, "same audio: {}", ranked[0].score);
}
