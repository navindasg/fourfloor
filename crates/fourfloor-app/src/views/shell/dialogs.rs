//! Choosing paths: native dialogs with the `dialogs` feature, otherwise a
//! small window to type the path into.

use std::path::PathBuf;

use crate::state::{DjIntent, FileIntent, Intent, Intents, LibraryIntent, PathPurpose, UiState};

use super::super::ViewCtx;

/// The intent for a chosen path.
pub fn intent_for(purpose: PathPurpose, path: PathBuf) -> Intent {
    let file = match purpose {
        PathPurpose::ScanFolder => return LibraryIntent::Scan(path).into(),
        PathPurpose::LoadDeck(deck) => return DjIntent::LoadFile { deck, path }.into(),
        PathPurpose::RecordMix => return DjIntent::Record(path).into(),
        PathPurpose::Open => FileIntent::Open(path),
        PathPurpose::SaveAs => FileIntent::SaveAs(path),
        PathPurpose::ExportWav => FileIntent::ExportWav(path),
        PathPurpose::ExportStems => FileIntent::ExportStems(path),
        PathPurpose::ImportDawproject => {
            let stem = path
                .file_stem()
                .map_or_else(|| "imported".into(), |s| s.to_os_string());
            let dir = path.with_file_name(stem);
            FileIntent::ImportDawproject { file: path, dir }
        }
        PathPurpose::ExportDawproject => FileIntent::ExportDawproject(path),
        PathPurpose::Reference => FileIntent::LoadReference(path),
        PathPurpose::ImportAudio => FileIntent::ImportAudio(path),
    };
    Intent::File(file)
}

/// Title for a purpose.
pub fn title(purpose: PathPurpose) -> &'static str {
    match purpose {
        PathPurpose::Open => "Open project folder",
        PathPurpose::SaveAs => "Save project to folder",
        PathPurpose::ExportWav => "Export WAV file",
        PathPurpose::ExportStems => "Export stems to folder",
        PathPurpose::ImportDawproject => "Import .dawproject file",
        PathPurpose::ExportDawproject => "Export .dawproject file",
        PathPurpose::Reference => "Load reference track",
        PathPurpose::ImportAudio => "Import audio file",
        PathPurpose::ScanFolder => "Scan a sample folder",
        PathPurpose::LoadDeck(_) => "Load a track into the deck",
        PathPurpose::RecordMix => "Record the DJ mix to a WAV file",
    }
}

#[cfg(feature = "dialogs")]
fn native(purpose: PathPurpose) -> Option<PathBuf> {
    let dialog = rfd::FileDialog::new().set_title(title(purpose));
    match purpose {
        PathPurpose::Open
        | PathPurpose::SaveAs
        | PathPurpose::ExportStems
        | PathPurpose::ScanFolder => dialog.pick_folder(),
        PathPurpose::RecordMix => dialog
            .add_filter("WAV", &["wav"])
            .set_file_name("dj-mix.wav")
            .save_file(),
        PathPurpose::ExportWav => dialog
            .add_filter("WAV", &["wav"])
            .set_file_name("mixdown.wav")
            .save_file(),
        PathPurpose::ExportDawproject => dialog
            .add_filter("DAWproject", &["dawproject"])
            .set_file_name("project.dawproject")
            .save_file(),
        PathPurpose::ImportDawproject => {
            dialog.add_filter("DAWproject", &["dawproject"]).pick_file()
        }
        PathPurpose::Reference | PathPurpose::ImportAudio | PathPurpose::LoadDeck(_) => dialog
            .add_filter("Audio", &["wav", "flac", "aif", "aiff", "mp3", "ogg"])
            .pick_file(),
    }
}

/// Asks for a path for `purpose`: a native dialog when available and
/// `native_dialogs` is on, else the typed prompt.
pub fn choose_path(ctx: &mut ViewCtx<'_>, purpose: PathPurpose) {
    ask(ctx.ui, ctx.intents, purpose);
}

/// The same as [`choose_path`] without a view context (for the app).
pub fn ask(ui: &mut UiState, intents: &mut Intents, purpose: PathPurpose) {
    #[cfg(feature = "dialogs")]
    if ui.native_dialogs {
        if let Some(path) = native(purpose) {
            intents.push(intent_for(purpose, path));
        }
        return;
    }
    let _ = &intents;
    ui.prompt = Some((purpose, String::new()));
}

/// The typed-path window.
pub fn prompt_window(egui_ctx: &egui::Context, ctx: &mut ViewCtx<'_>) {
    let Some((purpose, mut text)) = ctx.ui.prompt.clone() else {
        return;
    };
    let mut open = true;
    let mut done = None;
    egui::Window::new(title(purpose))
        .open(&mut open)
        .collapsible(false)
        .show(egui_ctx, |ui| {
            ui.label("Path:");
            let response = ui.add(egui::TextEdit::singleline(&mut text).desired_width(360.0));
            let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            ui.horizontal(|ui| {
                if (ui.button("OK").clicked() || enter) && !text.trim().is_empty() {
                    done = Some(PathBuf::from(text.trim()));
                }
                if ui.button("Cancel").clicked() {
                    ctx.ui.prompt = None;
                }
            });
        });
    if let Some(path) = done {
        ctx.intents.push(intent_for(purpose, path));
        ctx.ui.prompt = None;
    } else if !open {
        ctx.ui.prompt = None;
    } else if ctx.ui.prompt.is_some() {
        ctx.ui.prompt = Some((purpose, text));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imports_go_into_a_folder_next_to_the_file() {
        let intent = intent_for(
            PathPurpose::ImportDawproject,
            PathBuf::from("/tmp/song.dawproject"),
        );
        assert_eq!(
            intent,
            Intent::File(FileIntent::ImportDawproject {
                file: PathBuf::from("/tmp/song.dawproject"),
                dir: PathBuf::from("/tmp/song"),
            })
        );
    }
}
