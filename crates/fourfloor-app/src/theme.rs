//! The dark theme and the colours the custom-painted views share.

use egui::{Color32, CornerRadius, Stroke, Visuals};

/// Window and panel background.
pub const BG: Color32 = Color32::from_rgb(0x17, 0x19, 0x1d);
/// Slightly raised surfaces (headers, strips).
pub const SURFACE: Color32 = Color32::from_rgb(0x20, 0x23, 0x29);
/// Raised controls.
pub const RAISED: Color32 = Color32::from_rgb(0x2b, 0x2f, 0x37);
/// The timeline and grid background.
pub const CANVAS: Color32 = Color32::from_rgb(0x12, 0x14, 0x17);
/// Bar lines.
pub const GRID_BAR: Color32 = Color32::from_rgb(0x3a, 0x3f, 0x48);
/// Beat lines.
pub const GRID_BEAT: Color32 = Color32::from_rgb(0x28, 0x2c, 0x33);
/// Subdivision lines.
pub const GRID_SUB: Color32 = Color32::from_rgb(0x1e, 0x21, 0x26);
/// Primary accent (selection, playhead).
pub const ACCENT: Color32 = Color32::from_rgb(0xff, 0x8a, 0x3d);
/// Secondary accent (loop brace, active launch).
pub const ACCENT_2: Color32 = Color32::from_rgb(0x3d, 0xd6, 0xc6);
/// Text.
pub const TEXT: Color32 = Color32::from_rgb(0xd8, 0xdc, 0xe3);
/// Dim text.
pub const TEXT_DIM: Color32 = Color32::from_rgb(0x8a, 0x91, 0x9c);
/// Meter green.
pub const METER_OK: Color32 = Color32::from_rgb(0x4c, 0xd1, 0x6e);
/// Meter amber.
pub const METER_HOT: Color32 = Color32::from_rgb(0xf2, 0xc1, 0x4e);
/// Meter red.
pub const METER_CLIP: Color32 = Color32::from_rgb(0xf0, 0x4e, 0x4e);
/// Mute button on.
pub const MUTE: Color32 = Color32::from_rgb(0xe0, 0x9a, 0x2a);
/// Solo button on.
pub const SOLO: Color32 = Color32::from_rgb(0x3d, 0x9c, 0xf0);
/// Arm button on.
pub const ARM: Color32 = Color32::from_rgb(0xe0, 0x45, 0x45);

/// Clip colours, picked per track.
pub const TRACK_COLORS: [Color32; 8] = [
    Color32::from_rgb(0xff, 0x8a, 0x3d),
    Color32::from_rgb(0xf2, 0xc1, 0x4e),
    Color32::from_rgb(0x8f, 0xd1, 0x4f),
    Color32::from_rgb(0x3d, 0xd6, 0xc6),
    Color32::from_rgb(0x3d, 0x9c, 0xf0),
    Color32::from_rgb(0x9b, 0x7b, 0xf2),
    Color32::from_rgb(0xe8, 0x6b, 0xc0),
    Color32::from_rgb(0xf0, 0x6e, 0x5c),
];

/// The colour of the `index`-th track.
pub fn track_color(index: usize) -> Color32 {
    TRACK_COLORS[index % TRACK_COLORS.len()]
}

/// A colour darkened towards the canvas by `amount` (0..1).
pub fn dim(color: Color32, amount: f32) -> Color32 {
    let keep = 1.0 - amount.clamp(0.0, 1.0);
    let mix = |c: u8, bg: u8| (f32::from(c) * keep + f32::from(bg) * (1.0 - keep)) as u8;
    Color32::from_rgb(
        mix(color.r(), CANVAS.r()),
        mix(color.g(), CANVAS.g()),
        mix(color.b(), CANVAS.b()),
    )
}

/// Applies the theme to every style egui keeps.
pub fn apply(ctx: &egui::Context) {
    let mut visuals = Visuals::dark();
    visuals.panel_fill = BG;
    visuals.window_fill = SURFACE;
    visuals.extreme_bg_color = CANVAS;
    visuals.faint_bg_color = SURFACE;
    visuals.override_text_color = Some(TEXT);
    visuals.selection.bg_fill = dim(ACCENT, 0.45);
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT_2;
    visuals.window_corner_radius = CornerRadius::same(6);
    visuals.widgets.noninteractive.bg_fill = SURFACE;
    visuals.widgets.inactive.bg_fill = RAISED;
    visuals.widgets.inactive.weak_bg_fill = RAISED;
    visuals.widgets.hovered.bg_fill = dim(ACCENT, 0.7);
    visuals.widgets.active.bg_fill = dim(ACCENT, 0.5);
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, visuals);
    ctx.style_mut_of(egui::Theme::Dark, |style| {
        // Solid scroll bars: floating ones hide until hovered, so wide
        // mixers and launchers looked cut off at laptop sizes.
        style.spacing.scroll = egui::style::ScrollStyle::solid();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimming_moves_towards_the_canvas() {
        assert_eq!(dim(ACCENT, 0.0), ACCENT);
        assert_eq!(dim(ACCENT, 1.0), CANVAS);
        assert_eq!(track_color(9), TRACK_COLORS[1]);
    }
}
