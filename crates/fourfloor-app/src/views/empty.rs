//! One look for "nothing here yet": a dim title and a line saying what to
//! do, centred near the top of the view.

use egui::{RichText, Ui};

use crate::theme;

/// Space above an empty-state message.
const TOP_SPACE: f32 = 48.0;

/// Draws an empty state.
pub fn empty_state(ui: &mut Ui, title: &str, detail: &str) {
    ui.add_space(TOP_SPACE);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(title).size(18.0).color(theme::TEXT));
        ui.add_space(4.0);
        ui.label(RichText::new(detail).color(theme::TEXT_DIM));
    });
}
