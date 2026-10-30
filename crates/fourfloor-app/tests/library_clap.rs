//! Description search with CLAP embeddings (feature `clap-search`). Uses
//! the model from the user's model cache read-only and skips, saying so,
//! when it has not been downloaded.

#![cfg(feature = "clap-search")]

mod library_common;

use fourfloor_app::state::LibraryIntent;
use fourfloor_ml::{ModelCache, ModelRegistry, CLAP_GENERAL};
use library_common::{install_factory, run, temp_dir};

#[test]
fn descriptions_find_the_sound_they_describe() {
    let models = ModelCache::default_dir();
    let registry = ModelRegistry::builtin();
    let spec = registry.get(CLAP_GENERAL).expect("registered");
    if !ModelCache::new(&models).is_cached(spec) {
        eprintln!("skipped: {CLAP_GENERAL} is not in {}", models.display());
        return;
    }
    let (_guard, dir) = temp_dir();
    let mut app = library_common::library_app(&dir);
    app.features.paths.models = models;
    install_factory(&mut app);
    run(&mut app, LibraryIntent::Embed);
    assert!(!app.features.library.embedding);
    for (text, word) in [
        ("a kick drum", "kick"),
        ("hi-hat cymbal", "hat"),
        ("hand clap", "clap"),
    ] {
        run(&mut app, LibraryIntent::TextSearch(text.to_owned()));
        let titles: Vec<String> = app
            .features
            .library
            .rows
            .iter()
            .map(|r| r.record.title.to_lowercase())
            .collect();
        assert_eq!(titles.len(), 18, "every file is ranked");
        assert!(
            titles[0].contains(word),
            "\"{text}\" ranks {} first: {titles:?}",
            titles[0]
        );
    }
}
