//! Pitch on the vertical axis of the piano roll: note to row and back, and
//! note names.

/// Names of the twelve pitch classes, C first.
pub const PITCH_NAMES: [&str; 12] = [
    "C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B",
];

/// Maps MIDI pitches to rows. Higher pitches are higher on screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeyMap {
    /// Screen y of the top edge of the row of `top_pitch`.
    pub top_y: f32,
    /// Height of one row in pixels.
    pub row_height: f32,
    /// Pitch shown in the top row.
    pub top_pitch: u8,
}

impl Default for KeyMap {
    fn default() -> Self {
        Self {
            top_y: 0.0,
            row_height: 12.0,
            top_pitch: 96,
        }
    }
}

impl KeyMap {
    /// Screen y of the top edge of `pitch`'s row.
    pub fn y_of(&self, pitch: u8) -> f32 {
        self.top_y + (f32::from(self.top_pitch) - f32::from(pitch)) * self.row_height
    }

    /// The pitch whose row contains screen `y`, clamped to 0..=127.
    pub fn pitch_at(&self, y: f32) -> u8 {
        let rows = ((y - self.top_y) / self.row_height).floor();
        (f32::from(self.top_pitch) - rows).clamp(0.0, 127.0) as u8
    }

    /// Scrolls by whole rows (positive shows lower notes), clamped so the
    /// top row stays a valid pitch.
    pub fn scrolled_rows(&self, rows: i32) -> Self {
        let top = (i32::from(self.top_pitch) - rows).clamp(12, 127);
        Self {
            top_pitch: top as u8,
            ..*self
        }
    }

    /// Scrolled so the pitches `low..=high` sit in the middle of a view
    /// `height` pixels tall, with the same clamping as [`scrolled_rows`].
    ///
    /// [`scrolled_rows`]: Self::scrolled_rows
    pub fn centred_on(&self, low: u8, high: u8, height: f32) -> Self {
        let rows = (height / self.row_height.max(1.0)).max(1.0);
        let middle = (f32::from(low.min(high)) + f32::from(low.max(high))) / 2.0;
        let top = (middle + rows / 2.0).round() as i32;
        Self {
            top_pitch: top.clamp(12, 127) as u8,
            ..*self
        }
    }
}

/// `C4`-style name of a MIDI pitch (60 is C4).
pub fn pitch_name(pitch: u8) -> String {
    let octave = i32::from(pitch) / 12 - 1;
    format!("{}{octave}", PITCH_NAMES[usize::from(pitch % 12)])
}

/// `true` for the black keys of a piano.
pub fn is_black_key(pitch: u8) -> bool {
    matches!(pitch % 12, 1 | 3 | 6 | 8 | 10)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_and_pitches_round_trip() {
        let map = KeyMap {
            top_y: 40.0,
            row_height: 10.0,
            top_pitch: 84,
        };
        assert_eq!(map.y_of(84), 40.0);
        assert_eq!(map.y_of(60), 280.0);
        for pitch in [0u8, 36, 60, 84] {
            assert_eq!(map.pitch_at(map.y_of(pitch) + 5.0), pitch);
        }
        assert_eq!(map.pitch_at(map.y_of(60) + 9.9), 60);
        assert_eq!(map.pitch_at(map.y_of(60) + 10.1), 59);
        assert_eq!(map.pitch_at(-10_000.0), 127);
        assert_eq!(map.pitch_at(10_000.0), 0);
    }

    #[test]
    fn centring_puts_a_low_kick_line_in_view() {
        let map = KeyMap {
            top_y: 0.0,
            row_height: 12.0,
            top_pitch: 84,
        };
        // A kick on C2 (MIDI 36) in a 480 px view (40 rows).
        let centred = map.centred_on(36, 36, 480.0);
        assert_eq!(centred.top_pitch, 56);
        let row = centred.pitch_at(centred.y_of(36) + 1.0);
        assert_eq!(row, 36);
        assert!(centred.y_of(36) > 0.0 && centred.y_of(36) < 480.0);
        // Far out of range clamps like scrolling does.
        assert_eq!(map.centred_on(127, 127, 480.0).top_pitch, 127);
        assert_eq!(map.centred_on(0, 0, 480.0).top_pitch, 20);
    }

    #[test]
    fn scrolling_clamps_the_top_row() {
        let map = KeyMap::default();
        assert_eq!(map.scrolled_rows(12).top_pitch, 84);
        assert_eq!(map.scrolled_rows(-100).top_pitch, 127);
        assert_eq!(map.scrolled_rows(1000).top_pitch, 12);
    }

    #[test]
    fn pitches_have_names_and_colours() {
        assert_eq!(pitch_name(60), "C4");
        assert_eq!(pitch_name(57), "A3");
        assert_eq!(pitch_name(0), "C-1");
        assert!(is_black_key(61));
        assert!(!is_black_key(64));
    }
}
