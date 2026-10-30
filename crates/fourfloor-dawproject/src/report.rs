//! The structured report every import and export returns: what could not
//! be carried across exactly, and what happened to it instead.

use std::fmt;

use serde::Serialize;

/// What happened to something the target format cannot hold as it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// Kept, but outside the standard part of the format: in fourfloor's
    /// extension file on export (only fourfloor reads it back), or as a
    /// placeholder device on import.
    Preserved,
    /// Converted to the nearest thing the target has; the result may sound
    /// or look different.
    Approximated,
    /// Left out.
    Dropped,
    /// A file the project refers to could not be found, so what uses it
    /// was left out.
    Missing,
}

/// The kind of thing a report entry is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Subject {
    /// Tempo, time signature and their automation.
    Transport,
    /// A track or the track structure.
    Track,
    /// Volume, pan, mute, solo or routing of a channel.
    Channel,
    /// A send.
    Send,
    /// A device in a chain, or its parameters.
    Device,
    /// A plugin state blob.
    PluginState,
    /// A clip's placement, loop or fades.
    Clip,
    /// Notes of a MIDI clip.
    Note,
    /// An audio file or audio clip content.
    Audio,
    /// Warp markers.
    Warp,
    /// An automation lane.
    Automation,
    /// Launcher scenes and slots.
    Scene,
    /// Arrangement markers.
    Marker,
    /// Modulators, macros and controller mappings.
    Modulation,
    /// Groove and other project-wide settings.
    Project,
    /// Anything else, including elements this importer does not know.
    Other,
}

/// One entry of a [`Report`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct ReportItem {
    /// What happened.
    pub outcome: Outcome,
    /// What it happened to.
    pub subject: Subject,
    /// Where: a track, clip or device name, or an XML element path.
    pub location: String,
    /// A sentence a user can act on.
    pub message: String,
}

impl fmt::Display for ReportItem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let outcome = match self.outcome {
            Outcome::Preserved => "preserved",
            Outcome::Approximated => "approximated",
            Outcome::Dropped => "dropped",
            Outcome::Missing => "missing",
        };
        write!(f, "[{outcome}] {}: {}", self.location, self.message)
    }
}

/// Everything an import or export could not carry across exactly.
///
/// An empty report means the conversion lost nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Report {
    /// The entries, in the order they were found.
    pub items: Vec<ReportItem>,
}

impl Report {
    /// Whether nothing was preserved out of band, approximated or dropped.
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Number of entries.
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// Whether nothing audible or visible was lost: every entry, if any,
    /// is [`Outcome::Preserved`].
    pub fn is_lossless(&self) -> bool {
        self.items
            .iter()
            .all(|item| item.outcome == Outcome::Preserved)
    }

    /// The entries with one outcome.
    pub fn with_outcome(&self, outcome: Outcome) -> impl Iterator<Item = &ReportItem> {
        self.items
            .iter()
            .filter(move |item| item.outcome == outcome)
    }

    /// The entries about one kind of thing.
    pub fn about(&self, subject: Subject) -> impl Iterator<Item = &ReportItem> {
        self.items
            .iter()
            .filter(move |item| item.subject == subject)
    }

    /// Whether there is an entry with this outcome about this subject.
    pub fn has(&self, outcome: Outcome, subject: Subject) -> bool {
        self.items
            .iter()
            .any(|item| item.outcome == outcome && item.subject == subject)
    }

    pub(crate) fn push(
        &mut self,
        outcome: Outcome,
        subject: Subject,
        location: impl Into<String>,
        message: impl Into<String>,
    ) {
        self.items.push(ReportItem {
            outcome,
            subject,
            location: location.into(),
            message: message.into(),
        });
    }

    pub(crate) fn preserved(
        &mut self,
        subject: Subject,
        at: impl Into<String>,
        msg: impl Into<String>,
    ) {
        self.push(Outcome::Preserved, subject, at, msg);
    }

    pub(crate) fn approximated(
        &mut self,
        subject: Subject,
        at: impl Into<String>,
        msg: impl Into<String>,
    ) {
        self.push(Outcome::Approximated, subject, at, msg);
    }

    pub(crate) fn dropped(
        &mut self,
        subject: Subject,
        at: impl Into<String>,
        msg: impl Into<String>,
    ) {
        self.push(Outcome::Dropped, subject, at, msg);
    }

    pub(crate) fn missing(
        &mut self,
        subject: Subject,
        at: impl Into<String>,
        msg: impl Into<String>,
    ) {
        self.push(Outcome::Missing, subject, at, msg);
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.items.is_empty() {
            return f.write_str("nothing was lost");
        }
        for (index, item) in self.items.iter().enumerate() {
            if index > 0 {
                f.write_str("\n")?;
            }
            write!(f, "{item}")?;
        }
        Ok(())
    }
}
