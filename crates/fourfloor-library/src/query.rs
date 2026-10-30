//! Searching the library: text, tempo, key, tags, kind and collection
//! filters with a sort order.

use fourfloor_analysis::Key;
use rusqlite::types::Value;

use crate::error::LibraryError;
use crate::library::{file_from_row, Library, FILE_COLUMNS};
use crate::tags::normalise_tag;
use crate::types::{camelot_order, key_code, CrateId, FileKind, FileRecord, PlaylistId};

/// What to sort results by. Files without the value (no tempo, no key)
/// always come last.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SortKey {
    /// Title, ignoring case.
    #[default]
    Title,
    /// Full path.
    Path,
    /// Tempo.
    Bpm,
    /// Key, round the Camelot wheel (1A, 1B, 2A ...).
    Key,
    /// Length.
    Duration,
    /// Integrated loudness.
    Loudness,
    /// Star rating.
    Rating,
    /// Play count.
    PlayCount,
    /// Time last played.
    LastPlayed,
    /// Time first indexed.
    Added,
}

impl SortKey {
    fn column(self) -> &'static str {
        match self {
            Self::Title => "f.title COLLATE NOCASE",
            Self::Path => "f.path",
            Self::Bpm => "f.bpm",
            Self::Key => "f.key_camelot",
            Self::Duration => "f.duration_seconds",
            Self::Loudness => "f.loudness_lufs",
            Self::Rating => "f.rating",
            Self::PlayCount => "f.play_count",
            Self::LastPlayed => "f.last_played_at",
            Self::Added => "f.added_at",
        }
    }

    fn null_column(self) -> Option<&'static str> {
        match self {
            Self::Bpm => Some("f.bpm"),
            Self::Key => Some("f.key"),
            Self::Loudness => Some("f.loudness_lufs"),
            Self::LastPlayed => Some("f.last_played_at"),
            _ => None,
        }
    }
}

/// A library search. Every filter that is set must hold; an empty query
/// lists every file that is on disk.
///
/// ```
/// use fourfloor_library::{FileKind, Query, SortKey};
/// let query = Query::new()
///     .text("kick")
///     .bpm_range(120.0, 128.0)
///     .kind(FileKind::Loop)
///     .tag("drums")
///     .sort_by(SortKey::Bpm)
///     .limit(50);
/// assert_eq!(query.limit, Some(50));
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    /// Words that must each appear in the title, path or a tag.
    pub text: Option<String>,
    /// Tempo range in BPM, inclusive.
    pub bpm: Option<(f64, f64)>,
    /// Also match files at half or double the tempo range.
    pub half_double: bool,
    /// Keys allowed; empty allows any (and no key).
    pub keys: Vec<Key>,
    /// Tags every result must have.
    pub tags: Vec<String>,
    /// Tags no result may have.
    pub without_tags: Vec<String>,
    /// Kinds allowed; empty allows all.
    pub kinds: Vec<FileKind>,
    /// Only files in this crate.
    pub in_crate: Option<CrateId>,
    /// Only files in this playlist.
    pub in_playlist: Option<PlaylistId>,
    /// Lowest star rating.
    pub min_rating: Option<u8>,
    /// Longest duration in seconds.
    pub max_duration: Option<f64>,
    /// Include files the last scan did not find.
    pub include_missing: bool,
    /// Sort order.
    pub sort: SortKey,
    /// Sort largest first.
    pub descending: bool,
    /// At most this many results.
    pub limit: Option<usize>,
    /// Skip this many results first.
    pub offset: usize,
}

impl Query {
    /// A query that matches every file on disk.
    pub fn new() -> Self {
        Self::default()
    }

    /// Words to look for in title, path and tags.
    pub fn text(mut self, text: &str) -> Self {
        self.text = Some(text.to_owned());
        self
    }

    /// Tempo between `low` and `high` BPM (in either order).
    pub fn bpm_range(mut self, low: f64, high: f64) -> Self {
        // `min`/`max` would hide a NaN; keep it so the search refuses it.
        self.bpm = Some(if low <= high {
            (low, high)
        } else if high < low {
            (high, low)
        } else {
            (low, high)
        });
        self
    }

    /// Also accept files at half or double the tempo range.
    pub fn half_double(mut self) -> Self {
        self.half_double = true;
        self
    }

    /// Only files in `key`.
    pub fn key(mut self, key: Key) -> Self {
        self.keys.push(key);
        self
    }

    /// Only files in a key that mixes with `key`: itself, its relative, a
    /// fifth up and a fifth down.
    pub fn compatible_with(mut self, key: Key) -> Self {
        self.keys.extend(key.compatible_keys());
        self
    }

    /// Results must carry `tag`.
    pub fn tag(mut self, tag: &str) -> Self {
        self.tags.push(tag.to_owned());
        self
    }

    /// Results must not carry `tag`.
    pub fn without_tag(mut self, tag: &str) -> Self {
        self.without_tags.push(tag.to_owned());
        self
    }

    /// Allow files of `kind` (call again to allow more kinds).
    pub fn kind(mut self, kind: FileKind) -> Self {
        self.kinds.push(kind);
        self
    }

    /// Only files in a crate.
    pub fn in_crate(mut self, id: CrateId) -> Self {
        self.in_crate = Some(id);
        self
    }

    /// Only files in a playlist.
    pub fn in_playlist(mut self, id: PlaylistId) -> Self {
        self.in_playlist = Some(id);
        self
    }

    /// At least `stars`.
    pub fn min_rating(mut self, stars: u8) -> Self {
        self.min_rating = Some(stars);
        self
    }

    /// At most `seconds` long.
    pub fn max_duration(mut self, seconds: f64) -> Self {
        self.max_duration = Some(seconds);
        self
    }

    /// Include files that were not found on disk.
    pub fn include_missing(mut self) -> Self {
        self.include_missing = true;
        self
    }

    /// Sort by `key`, smallest first.
    pub fn sort_by(mut self, key: SortKey) -> Self {
        self.sort = key;
        self
    }

    /// Sort largest first.
    pub fn descending(mut self) -> Self {
        self.descending = true;
        self
    }

    /// At most `count` results.
    pub fn limit(mut self, count: usize) -> Self {
        self.limit = Some(count);
        self
    }

    /// Skip `count` results.
    pub fn offset(mut self, count: usize) -> Self {
        self.offset = count;
        self
    }

    /// The WHERE clause and its parameters.
    fn filter(&self) -> Result<(String, Vec<Value>), LibraryError> {
        let mut clauses: Vec<String> = Vec::new();
        let mut values: Vec<Value> = Vec::new();
        let bind = bind_value;

        if !self.include_missing {
            clauses.push("f.missing = 0".to_owned());
        }
        if let Some(text) = &self.text {
            for word in text.split_whitespace() {
                let pattern = format!("%{}%", escape_like(&word.to_lowercase()));
                let p = bind(&mut values, Value::Text(pattern));
                clauses.push(format!(
                    "(lower(f.title) LIKE {p} ESCAPE '\\' OR lower(f.path) LIKE {p} ESCAPE '\\' \
                     OR EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id \
                                WHERE ft.file_id = f.id AND t.name LIKE {p} ESCAPE '\\'))"
                ));
            }
        }
        if let Some((low, high)) = self.bpm {
            if !(low.is_finite() && high.is_finite()) {
                return Err(LibraryError::invalid("tempo range", "not finite"));
            }
            let mut ranges = vec![(low, high)];
            if self.half_double {
                ranges.push((low / 2.0, high / 2.0));
                ranges.push((low * 2.0, high * 2.0));
            }
            let parts: Vec<String> = ranges
                .into_iter()
                .map(|(lo, hi)| {
                    let a = bind(&mut values, Value::Real(lo));
                    let b = bind(&mut values, Value::Real(hi));
                    format!("f.bpm BETWEEN {a} AND {b}")
                })
                .collect();
            clauses.push(format!("({})", parts.join(" OR ")));
        }
        if !self.keys.is_empty() {
            let mut codes: Vec<i64> = self.keys.iter().map(|&key| key_code(key)).collect();
            codes.sort_unstable();
            codes.dedup();
            let list = join(
                codes
                    .into_iter()
                    .map(|code| bind(&mut values, Value::Integer(code))),
            );
            clauses.push(format!("f.key IN ({list})"));
        }
        for tag in self.tags.iter().filter_map(|tag| normalise_tag(tag)) {
            let p = bind(&mut values, Value::Text(tag));
            clauses.push(format!(
                "EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id \
                         WHERE ft.file_id = f.id AND t.name = {p})"
            ));
        }
        for tag in self
            .without_tags
            .iter()
            .filter_map(|tag| normalise_tag(tag))
        {
            let p = bind(&mut values, Value::Text(tag));
            clauses.push(format!(
                "NOT EXISTS (SELECT 1 FROM file_tags ft JOIN tags t ON t.id = ft.tag_id \
                             WHERE ft.file_id = f.id AND t.name = {p})"
            ));
        }
        if !self.kinds.is_empty() {
            let list = join(
                self.kinds
                    .iter()
                    .map(|kind| bind(&mut values, Value::Text(kind.as_str().to_owned()))),
            );
            clauses.push(format!("f.kind IN ({list})"));
        }
        if let Some(id) = self.in_crate {
            let p = bind(&mut values, Value::Integer(id.0));
            clauses.push(format!(
                "f.id IN (SELECT file_id FROM crate_files WHERE crate_id = {p})"
            ));
        }
        if let Some(id) = self.in_playlist {
            let p = bind(&mut values, Value::Integer(id.0));
            clauses.push(format!(
                "f.id IN (SELECT file_id FROM playlist_entries WHERE playlist_id = {p})"
            ));
        }
        if let Some(stars) = self.min_rating {
            let p = bind(&mut values, Value::Integer(i64::from(stars)));
            clauses.push(format!("f.rating >= {p}"));
        }
        if let Some(seconds) = self.max_duration {
            let p = bind(&mut values, Value::Real(seconds));
            clauses.push(format!("f.duration_seconds <= {p}"));
        }
        let clause = if clauses.is_empty() {
            "1".to_owned()
        } else {
            clauses.join(" AND ")
        };
        Ok((clause, values))
    }
}

/// Adds a parameter and returns its placeholder.
fn bind_value(values: &mut Vec<Value>, value: Value) -> String {
    values.push(value);
    format!("?{}", values.len())
}

fn join(parts: impl Iterator<Item = String>) -> String {
    parts.collect::<Vec<_>>().join(", ")
}

/// Escapes `%`, `_` and `\` for a LIKE pattern with `ESCAPE '\'`.
fn escape_like(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '%' | '_' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
}

impl Library {
    /// The files matching `query`, in its order.
    pub fn search(&self, query: &Query) -> Result<Vec<FileRecord>, LibraryError> {
        let (clause, mut values) = query.filter()?;
        let direction = if query.descending { "DESC" } else { "ASC" };
        let nulls_last = query
            .sort
            .null_column()
            .map(|column| format!("{column} IS NULL, "))
            .unwrap_or_default();
        values.push(Value::Integer(
            query
                .limit
                .and_then(|limit| i64::try_from(limit).ok())
                .unwrap_or(-1),
        ));
        let limit = values.len();
        values.push(Value::Integer(i64::try_from(query.offset).unwrap_or(0)));
        let offset = values.len();
        let sql = format!(
            "SELECT {FILE_COLUMNS} FROM files f WHERE {clause} \
             ORDER BY {nulls_last}{} {direction}, f.id {direction} LIMIT ?{limit} OFFSET ?{offset}",
            query.sort.column()
        );
        let mut statement = self.conn.prepare(&sql)?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), file_from_row)?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// How many files match `query`, ignoring its limit and offset.
    pub fn count(&self, query: &Query) -> Result<usize, LibraryError> {
        let (clause, values) = query.filter()?;
        let total: i64 = self.conn.query_row(
            &format!("SELECT count(*) FROM files f WHERE {clause}"),
            rusqlite::params_from_iter(values),
            |row| row.get(0),
        )?;
        Ok(usize::try_from(total).unwrap_or(0))
    }

    /// The ids of the files matching `query`, unsorted and unlimited.
    pub(crate) fn matching_ids(&self, query: &Query) -> Result<Vec<i64>, LibraryError> {
        let (clause, values) = query.filter()?;
        let mut statement = self
            .conn
            .prepare(&format!("SELECT f.id FROM files f WHERE {clause}"))?;
        let rows = statement.query_map(rusqlite::params_from_iter(values), |row| row.get(0))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }
}

/// The Camelot sort position of a key, as [`SortKey::Key`] orders it.
pub fn camelot_position(key: Key) -> i64 {
    camelot_order(key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn like_patterns_are_escaped() {
        assert_eq!(escape_like("100%_a\\b"), "100\\%\\_a\\\\b");
    }

    #[test]
    fn an_empty_query_only_hides_missing_files() {
        let (clause, values) = Query::new().filter().expect("filter");
        assert_eq!(clause, "f.missing = 0");
        assert!(values.is_empty());
        let (clause, _) = Query::new().include_missing().filter().expect("filter");
        assert_eq!(clause, "1");
    }

    #[test]
    fn a_non_finite_tempo_range_is_refused() {
        assert!(Query::new().bpm_range(f64::NAN, 120.0).filter().is_err());
    }
}
