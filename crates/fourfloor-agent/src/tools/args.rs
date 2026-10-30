//! Argument types of the high-level tools, with their JSON Schemas.

use fourfloor_core::TrackId;
use fourfloor_model::{Project, SwingGrid};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::AgentError;
use crate::generate::{BassStyle, ChordStyle, DrumStyle};

/// A track, by id or by name.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum TrackRef {
    /// The track's id.
    Id(u64),
    /// The track's name. Matching ignores case; a unique partial match
    /// (`"hat"` for `"Hats"`) also works.
    Name(String),
}

impl From<TrackId> for TrackRef {
    fn from(id: TrackId) -> Self {
        Self::Id(id.get())
    }
}

impl From<&str> for TrackRef {
    fn from(name: &str) -> Self {
        Self::Name(name.to_owned())
    }
}

/// Lowercase, without a plural `s`, for loose name matching.
fn stem(word: &str) -> String {
    let lower = word.trim().to_lowercase();
    let trimmed = lower.strip_prefix("the ").unwrap_or(&lower).trim();
    let singular = trimmed
        .strip_suffix("es")
        .filter(|rest| rest.ends_with("sh") || rest.ends_with("ch"))
        .or_else(|| trimmed.strip_suffix('s'))
        .unwrap_or(trimmed);
    singular.replace(['-', '_'], " ")
}

/// Other words a track for a sound tends to be called.
fn synonyms(stemmed: &str) -> &'static [&'static str] {
    match stemmed {
        "hat" | "hihat" | "hi hat" | "closed hat" | "open hat" | "shaker" => &["hat", "drum"],
        "kick" | "bass drum" | "bd" => &["kick", "drum"],
        "clap" | "snare" | "rim" | "perc" | "percussion" => &["drum", "perc"],
        "bass" | "bassline" | "sub" => &["bass"],
        "chord" | "organ" | "stab" | "key" | "piano" => &["chord", "organ", "stab", "key", "pad"],
        "pad" | "string" => &["pad", "chord"],
        "beat" | "drum" | "drum kit" | "kit" => &["drum", "kit", "beat"],
        _ => &[],
    }
}

impl TrackRef {
    /// The id of the track this names in `project`.
    pub fn resolve(&self, project: &Project) -> Result<TrackId, AgentError> {
        match self {
            Self::Id(raw) => {
                let id = TrackId::new(*raw);
                project
                    .track(id)
                    .map(|track| track.id)
                    .ok_or_else(|| AgentError::NotFound(format!("no track has id {raw}")))
            }
            Self::Name(name) => resolve_name(project, name),
        }
    }
}

fn resolve_name(project: &Project, name: &str) -> Result<TrackId, AgentError> {
    let wanted = stem(name);
    if wanted.is_empty() {
        return Err(AgentError::NotFound("an empty track name".to_owned()));
    }
    let tracks: Vec<(TrackId, String)> = project
        .tracks
        .iter()
        .map(|track| (track.id, stem(&track.name)))
        .collect();
    // Exact, then the track name contains the word, then the word
    // contains the track name, then synonyms.
    let exact = tracks.iter().find(|(_, track)| *track == wanted);
    let contains = || {
        tracks
            .iter()
            .filter(|(_, track)| track.contains(&wanted))
            .min_by_key(|(_, track)| track.len())
    };
    let within = || {
        tracks
            .iter()
            .filter(|(_, track)| !track.is_empty() && wanted.contains(track.as_str()))
            .max_by_key(|(_, track)| track.len())
    };
    let synonym = || {
        synonyms(&wanted).iter().find_map(|word| {
            tracks
                .iter()
                .filter(|(_, track)| track.contains(word))
                .min_by_key(|(_, track)| track.len())
        })
    };
    exact
        .or_else(contains)
        .or_else(within)
        .or_else(synonym)
        .map(|(id, _)| *id)
        .ok_or_else(|| {
            let names: Vec<&str> = project.tracks.iter().map(|t| t.name.as_str()).collect();
            AgentError::NotFound(format!(
                "no track matches \"{name}\"; tracks are: {}",
                if names.is_empty() {
                    "(none)".to_owned()
                } else {
                    names.join(", ")
                }
            ))
        })
}

fn default_bars() -> u32 {
    4
}
fn default_density() -> f64 {
    0.5
}
fn default_seed() -> u64 {
    1
}
fn default_true() -> bool {
    true
}
fn default_key() -> String {
    "A minor".to_owned()
}
fn default_tail() -> f64 {
    1.0
}

/// Writes a generated house drum pattern: a kick track and a drum-kit track
/// (claps, hats, percussion, fills). One undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddHouseDrumPatternArgs {
    /// Length in bars, 1 to 64. Defaults to 4.
    #[serde(default = "default_bars")]
    pub bars: u32,
    /// Flavour: classic, deep, tech or jackin. Defaults to classic.
    #[serde(default)]
    pub style: DrumStyle,
    /// How busy hats and percussion are, 0 to 1. Defaults to 0.5.
    #[serde(default = "default_density")]
    pub density: f64,
    /// Snare fills at phrase ends. Defaults to true.
    #[serde(default = "default_true")]
    pub fills: bool,
    /// Random seed: the same seed gives the same pattern. Defaults to 1.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Bar the pattern starts on, counting from 0. Defaults to 0.
    #[serde(default)]
    pub start_bar: u32,
    /// Existing MIDI track for the kick. Omit to create a "Kick" track
    /// with a 909 kick.
    #[serde(default)]
    pub kick_track: Option<TrackRef>,
    /// Existing MIDI track for the other drums. Omit to create a "Drums"
    /// track with the drum kit.
    #[serde(default)]
    pub drum_track: Option<TrackRef>,
}

/// Writes a generated bassline. One undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddBasslineArgs {
    /// Length in bars, 1 to 64. Defaults to 4.
    #[serde(default = "default_bars")]
    pub bars: u32,
    /// Key, such as "A minor", "F#m" or "C dorian". Defaults to A minor.
    #[serde(default = "default_key")]
    pub key: String,
    /// offbeat, rolling, octave or walking. Defaults to offbeat.
    #[serde(default)]
    pub style: BassStyle,
    /// Octave of the root (1 puts A at MIDI 33). Omit for the usual range.
    #[serde(default)]
    pub octave: Option<i32>,
    /// Scale degree per bar (0 is the tonic), repeating. Omit to stay on
    /// the tonic; pass a chord part's progression to follow it.
    #[serde(default)]
    pub progression: Vec<i32>,
    /// How busy the line is, 0 to 1. Defaults to 0.5.
    #[serde(default = "default_density")]
    pub density: f64,
    /// Random seed. Defaults to 1.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Bar the line starts on, counting from 0. Defaults to 0.
    #[serde(default)]
    pub start_bar: u32,
    /// Existing MIDI track. Omit to create a "Bass" track.
    #[serde(default)]
    pub track: Option<TrackRef>,
    /// Instrument for a new track. Defaults to synth.acid303.
    #[serde(default)]
    pub instrument: Option<String>,
}

/// Writes a generated chord part (organ stabs, pads, pulses). One undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddChordsArgs {
    /// Length in bars, 1 to 64. Defaults to 4.
    #[serde(default = "default_bars")]
    pub bars: u32,
    /// Key, such as "A minor". Defaults to A minor.
    #[serde(default = "default_key")]
    pub key: String,
    /// organ, stab, sustained or pulse. Defaults to organ.
    #[serde(default)]
    pub style: ChordStyle,
    /// Notes per chord, 3 to 5. Defaults to 4.
    #[serde(default)]
    pub voices: Option<usize>,
    /// Octave of the chords, 2 to 6. Defaults to 3.
    #[serde(default)]
    pub octave: Option<i32>,
    /// Scale degree per bar. Omit to generate a progression.
    #[serde(default)]
    pub progression: Vec<i32>,
    /// How busy the rhythm is, 0 to 1. Defaults to 0.5.
    #[serde(default = "default_density")]
    pub density: f64,
    /// Random seed. Defaults to 1.
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Bar the part starts on. Defaults to 0.
    #[serde(default)]
    pub start_bar: u32,
    /// Existing MIDI track. Omit to create a "Chords" track.
    #[serde(default)]
    pub track: Option<TrackRef>,
    /// Instrument for a new track. Defaults to fm.organ for organ and stab,
    /// poly.pad for sustained and pulse.
    #[serde(default)]
    pub instrument: Option<String>,
}

/// Sets swing. With no track: the project's swing (used by every pattern
/// written later) and, unless `retime` is false, every MIDI clip is
/// re-timed. With a track: only that track's clips are re-timed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSwingArgs {
    /// Swing from 50 (straight) to 75 percent; 66.7 is triplet feel.
    pub percent: f64,
    /// eighth or sixteenth. Omit to keep the project's grid.
    #[serde(default)]
    pub grid: Option<SwingGrid>,
    /// Only re-time this track.
    #[serde(default)]
    pub track: Option<TrackRef>,
    /// Re-time notes already written. Defaults to true. Notes up to a
    /// third of a step after an off-step are treated as swung off-steps.
    #[serde(default = "default_true")]
    pub retime: bool,
}

/// Ducks a track whenever another track hits: adds a sidechain compressor
/// to `track` (or reuses one) keyed from `key_track`. One undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SidechainArgs {
    /// The track to duck, such as the pad or bass.
    pub track: TrackRef,
    /// The track that keys the ducking, usually the kick.
    pub key_track: TrackRef,
    /// How hard it ducks, 0 to 1. Defaults to 0.75.
    #[serde(default)]
    pub depth: Option<f64>,
    /// Release in milliseconds, 5 to 2000. Defaults to 160.
    #[serde(default)]
    pub release_ms: Option<f32>,
    /// Key from the source before its fader, so it ducks even when the
    /// key track is muted. Defaults to false.
    #[serde(default)]
    pub pre_fader: bool,
}

/// Which way a filter riser sweeps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RiserMode {
    /// A high-pass whose cutoff rises, thinning the sound out.
    #[default]
    HighPass,
    /// A low-pass that opens up, brightening the sound.
    LowPass,
}

/// Adds a filter sweep over the last `bars` before `end_bar`, resetting at
/// `end_bar` (the drop). One undo step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AddFilterRiserArgs {
    /// Length of the sweep in bars, 1 to 64.
    pub bars: u32,
    /// The track to filter. Omit to filter every track that feeds the
    /// master, like a filter across the whole mix.
    #[serde(default)]
    pub track: Option<TrackRef>,
    /// Bar where the sweep ends and the filter resets. Defaults to the end
    /// of the arrangement.
    #[serde(default)]
    pub end_bar: Option<u32>,
    /// high_pass or low_pass. Defaults to high_pass.
    #[serde(default)]
    pub mode: RiserMode,
    /// Filter resonance during the sweep, 0 to 1. Defaults to 0.45.
    #[serde(default)]
    pub resonance: Option<f32>,
}

/// Audio file format of renders.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AudioFormat {
    /// 32-bit float.
    #[default]
    Float32,
    /// 24-bit integer.
    Pcm24,
    /// 16-bit integer, without dither.
    Pcm16,
}

/// Renders the project to a stereo WAV file in the project folder.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenderArgs {
    /// File path relative to the project folder. Defaults to
    /// `renders/<project name>.wav`. Existing files are overwritten.
    #[serde(default)]
    pub path: Option<String>,
    /// Render this many bars instead of the whole arrangement.
    #[serde(default)]
    pub bars: Option<u32>,
    /// Seconds rendered after the end, for reverb and release tails.
    /// Defaults to 1.
    #[serde(default = "default_tail")]
    pub tail_seconds: f64,
    /// Normalise to this integrated loudness in LUFS (true peak kept under
    /// -1 dBTP). Omit to leave the level alone.
    #[serde(default)]
    pub normalize_lufs: Option<f64>,
    /// File format. Defaults to float32.
    #[serde(default)]
    pub format: AudioFormat,
}

/// Renders every track that feeds the master to its own WAV file, plus the
/// master, in one aligned pass.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ExportStemsArgs {
    /// Folder relative to the project folder. Defaults to "stems".
    #[serde(default)]
    pub dir: Option<String>,
    /// Render this many bars instead of the whole arrangement.
    #[serde(default)]
    pub bars: Option<u32>,
    /// Seconds rendered after the end. Defaults to 1.
    #[serde(default = "default_tail")]
    pub tail_seconds: f64,
    /// Normalise the master to this loudness and scale the stems with it.
    #[serde(default)]
    pub normalize_lufs: Option<f64>,
    /// File format. Defaults to float32.
    #[serde(default)]
    pub format: AudioFormat,
}

/// Renders the project and reports loudness, peaks, stereo and spectral
/// balance for the master and every stem.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MixReportArgs {
    /// Analyse this many bars instead of the whole arrangement.
    #[serde(default)]
    pub bars: Option<u32>,
}

/// Runs a plain-English instruction through the built-in phrase parser
/// ("set tempo to 124", "swing the hats to 56%", "sidechain the pad to the
/// kick"). No language model is involved.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InstructArgs {
    /// The instruction.
    pub text: String,
}

/// A tool with no arguments.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NoArgs {}
