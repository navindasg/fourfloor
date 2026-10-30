//! Sample browser logic: the filter bar as a library [`Query`], column
//! sorting, and the labels the table shows.

use fourfloor_analysis::{Key, Peak};
use fourfloor_core::{SampleRate, Tempo};
use fourfloor_library::{
    plan_preview, FileKind, FileRecord, PreviewMode, PreviewTarget, Query, SortKey,
};

/// The browser's filter bar.
#[derive(Clone, Debug, PartialEq)]
pub struct BrowserFilters {
    /// Words to find in names, folders and tags.
    pub text: String,
    /// Tempo range, when on.
    pub bpm: Option<(f64, f64)>,
    /// Count 62 BPM as 124.
    pub half_double: bool,
    /// Only this key.
    pub key: Option<Key>,
    /// Keys that mix with `key` too.
    pub compatible: bool,
    /// Only files with this tag.
    pub tag: Option<String>,
    /// Only this kind.
    pub kind: Option<FileKind>,
    /// Sort column.
    pub sort: SortKey,
    /// Sort descending.
    pub descending: bool,
}

impl Default for BrowserFilters {
    fn default() -> Self {
        Self {
            text: String::new(),
            bpm: None,
            half_double: true,
            key: None,
            compatible: false,
            tag: None,
            kind: None,
            sort: SortKey::Title,
            descending: false,
        }
    }
}

impl BrowserFilters {
    /// The library query these filters make.
    pub fn to_query(&self) -> Query {
        let mut query = Query::new().sort_by(self.sort);
        let text = self.text.trim();
        if !text.is_empty() {
            query = query.text(text);
        }
        if let Some((low, high)) = self.bpm {
            query = query.bpm_range(low.min(high), low.max(high));
            if self.half_double {
                query = query.half_double();
            }
        }
        if let Some(key) = self.key {
            query = if self.compatible {
                query.compatible_with(key)
            } else {
                query.key(key)
            };
        }
        if let Some(tag) = self.tag.as_deref().filter(|t| !t.is_empty()) {
            query = query.tag(tag);
        }
        if let Some(kind) = self.kind {
            query = query.kind(kind);
        }
        if self.descending {
            query = query.descending();
        }
        query
    }

    /// Clicking a column header: sort by it, or flip the order when it is
    /// already the sort column.
    pub fn click_column(&mut self, column: SortKey) {
        if self.sort == column {
            self.descending = !self.descending;
        } else {
            self.sort = column;
            self.descending = false;
        }
    }
}

/// Short label of a file kind.
pub fn kind_label(kind: FileKind) -> &'static str {
    match kind {
        FileKind::OneShot => "one-shot",
        FileKind::Loop => "loop",
        FileKind::Track => "track",
    }
}

/// A key as `"Am · 8A"`, or a dash.
pub fn key_label(key: Option<Key>) -> String {
    key.map_or_else(
        || "-".to_owned(),
        |key| format!("{} · {}", key.short_name(), key.camelot()),
    )
}

/// A tempo with one decimal, or a dash.
pub fn bpm_label(bpm: Option<f64>) -> String {
    bpm.map_or_else(|| "-".to_owned(), |bpm| format!("{bpm:.1}"))
}

/// Seconds as `m:ss` (or `s.s s` under ten seconds).
pub fn duration_label(seconds: f64) -> String {
    if seconds < 10.0 {
        format!("{seconds:.1} s")
    } else {
        let whole = seconds.round() as u64;
        format!("{}:{:02}", whole / 60, whole % 60)
    }
}

/// The tags of a record, comma separated.
pub fn tags_label(record: &FileRecord) -> String {
    record.tag_names().join(", ")
}

/// How an audition follows the project.
pub fn mode_label(mode: PreviewMode) -> &'static str {
    match mode {
        PreviewMode::Original => "Original",
        PreviewMode::Varispeed => "Varispeed",
        PreviewMode::KeyLocked => "Key-locked",
    }
}

/// The waveform's columns resampled to `width` pixel columns: the lowest
/// and highest sample under each pixel.
pub fn waveform_columns(peaks: &[Peak], width: usize) -> Vec<(f32, f32)> {
    if peaks.is_empty() || width == 0 {
        return Vec::new();
    }
    (0..width)
        .map(|column| {
            let start = column * peaks.len() / width;
            let end = ((column + 1) * peaks.len() / width).max(start + 1);
            peaks[start..end.min(peaks.len())]
                .iter()
                .fold((0.0f32, 0.0f32), |(low, high), peak| {
                    (low.min(peak.min), high.max(peak.max))
                })
        })
        .collect()
}

/// What an audition of `record` will do to fit the project, in words:
/// `"plays at 128.0 BPM (x1.032), +3 st"`.
pub fn audition_note(
    record: &FileRecord,
    tempo: Tempo,
    key: Option<Key>,
    mode: PreviewMode,
    sample_rate: SampleRate,
) -> String {
    let plan = plan_preview(
        record,
        &PreviewTarget::new(Some(tempo), key, sample_rate).with_mode(mode),
    );
    let mut parts = Vec::new();
    if let Some(bpm) = plan.target_bpm {
        parts.push(format!("plays at {bpm:.1} BPM (x{:.3})", plan.speed));
    } else if (plan.speed - 1.0).abs() > 1e-6 {
        parts.push(format!("speed x{:.3}", plan.speed));
    }
    if plan.pitch_semitones.abs() > 1e-6 {
        parts.push(format!("{:+.0} st", plan.pitch_semitones));
    }
    if parts.is_empty() {
        "plays as recorded".to_owned()
    } else {
        parts.join(", ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_become_a_query() {
        let filters = BrowserFilters {
            text: "  kick ".to_owned(),
            bpm: Some((128.0, 120.0)),
            key: Some(Key::minor(9)),
            compatible: true,
            tag: Some("drums".to_owned()),
            kind: Some(FileKind::Loop),
            ..BrowserFilters::default()
        };
        let query = filters.to_query();
        assert_eq!(query.text.as_deref(), Some("kick"));
        assert_eq!(query.bpm, Some((120.0, 128.0)));
        assert!(query.half_double);
        assert_eq!(query.keys.len(), 4, "A minor and its compatible keys");
        assert_eq!(query.tags, vec!["drums".to_owned()]);
        assert_eq!(query.kinds, vec![FileKind::Loop]);
        let empty = BrowserFilters::default().to_query();
        assert!(empty.text.is_none() && empty.keys.is_empty() && empty.bpm.is_none());
    }

    #[test]
    fn column_clicks_sort_then_flip() {
        let mut filters = BrowserFilters::default();
        filters.click_column(SortKey::Bpm);
        assert_eq!((filters.sort, filters.descending), (SortKey::Bpm, false));
        filters.click_column(SortKey::Bpm);
        assert!(filters.descending);
        filters.click_column(SortKey::Key);
        assert_eq!((filters.sort, filters.descending), (SortKey::Key, false));
        assert!(filters.to_query().sort == SortKey::Key);
    }

    #[test]
    fn labels_read_well() {
        assert_eq!(key_label(Some(Key::minor(9))), "Am · 8A");
        assert_eq!(key_label(None), "-");
        assert_eq!(bpm_label(Some(124.04)), "124.0");
        assert_eq!(duration_label(3.25), "3.2 s");
        assert_eq!(duration_label(125.0), "2:05");
        assert_eq!(kind_label(FileKind::OneShot), "one-shot");
    }

    #[test]
    fn waveform_columns_keep_the_extremes() {
        let peaks: Vec<Peak> = (0..8)
            .map(|i| Peak {
                min: -(i as f32) / 8.0,
                max: i as f32 / 8.0,
                rms: 0.0,
            })
            .collect();
        let columns = waveform_columns(&peaks, 4);
        assert_eq!(columns.len(), 4);
        assert_eq!(columns[0], (-0.125, 0.125));
        assert_eq!(columns[3], (-0.875, 0.875));
        let wide = waveform_columns(&peaks, 16);
        assert_eq!(wide.len(), 16);
        assert_eq!(wide[15], (-0.875, 0.875));
        assert!(waveform_columns(&[], 10).is_empty());
    }
}
