//! A deterministic parser for common studio phrases. It needs no model:
//! fixed word patterns turn text into tool calls.
//!
//! It understands, among others: "set tempo to 124", "swing the hats to
//! 56%", "sidechain the pad to the kick", "make a 16 bar deep house beat",
//! "add a rolling bassline in F minor", "add organ chords", "add an 8 bar
//! filter riser", "mute the clap", "turn the bass down 3 db", "pan the hats
//! left 30%", "render", "export stems", "undo". Clauses joined by "and" or
//! "then" become several calls.

use fourfloor_model::Project;
use serde_json::{json, Map, Value};

use crate::generate::Key;
use crate::tools::TrackRef;

/// One tool call: a tool name and its JSON arguments.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolCall {
    /// The tool.
    pub name: String,
    /// Its arguments object.
    pub args: Value,
}

impl ToolCall {
    fn new(name: &str, args: Value) -> Self {
        Self {
            name: name.to_owned(),
            args,
        }
    }
}

/// Why a phrase was not understood.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ParseError(pub String);

const HINT: &str = "try \"set tempo to 124\", \"swing the hats to 56%\", \
\"sidechain the pad to the kick\", \"add a 16 bar house beat\", \"add a bassline in A minor\", \
\"add organ chords\", \"add an 8 bar filter riser\", \"mute the clap\", \"render\" or \"undo\"";

/// Words dropped before matching.
const FILLER: &[&str] = &[
    "please", "can", "could", "would", "you", "kindly", "just", "now", "let's", "lets", "i",
    "want", "wanna", "to", "me",
];

const ADD_VERBS: &[&str] = &[
    "add", "make", "create", "give", "write", "generate", "program", "build", "lay", "start",
    "put", "need", "drop",
];

fn word_number(word: &str) -> Option<f64> {
    Some(match word {
        "one" | "a" | "an" => 1.0,
        "two" => 2.0,
        "three" => 3.0,
        "four" => 4.0,
        "five" => 5.0,
        "six" => 6.0,
        "eight" => 8.0,
        "ten" => 10.0,
        "twelve" => 12.0,
        "sixteen" => 16.0,
        "twenty" => 20.0,
        "thirty-two" | "thirtytwo" => 32.0,
        "sixty-four" | "sixtyfour" => 64.0,
        "half" => 50.0,
        _ => return None,
    })
}

/// Reads "124", "56%", "-3db", "124bpm", "8-bar", "16bars".
fn number(token: &str) -> Option<f64> {
    let cleaned = token.trim_matches(|c: char| c == ',' || c == '(' || c == ')');
    let lower = cleaned.to_lowercase();
    let stripped = [
        "%", "percent", "bpm", "db", "-bars", "-bar", "bars", "bar", "hz", "lufs", "ms",
    ]
    .iter()
    .fold(lower.as_str(), |text, suffix| {
        text.strip_suffix(suffix).unwrap_or(text)
    });
    let stripped = stripped.strip_prefix('+').unwrap_or(stripped);
    stripped
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// A phrase in lowercase words, with the original words kept for names.
#[derive(Clone, Debug)]
struct Words {
    lower: Vec<String>,
    original: Vec<String>,
}

impl Words {
    fn new(text: &str) -> Self {
        let original: Vec<String> = text
            .split_whitespace()
            .map(|word| {
                word.trim_end_matches(['.', '!', '?', ',', ';', ':'])
                    .to_owned()
            })
            .filter(|word| !word.is_empty())
            .collect();
        let lower = original.iter().map(|word| word.to_lowercase()).collect();
        Self { lower, original }
    }

    fn has(&self, word: &str) -> bool {
        self.lower.iter().any(|w| w == word)
    }

    fn has_any(&self, words: &[&str]) -> bool {
        words.iter().any(|word| self.has(word))
    }

    fn contains_text(&self, text: &str) -> bool {
        self.lower.join(" ").contains(text)
    }

    fn position(&self, word: &str) -> Option<usize> {
        self.lower.iter().position(|w| w == word)
    }

    fn position_any(&self, words: &[&str]) -> Option<usize> {
        self.lower.iter().position(|w| words.contains(&w.as_str()))
    }

    /// The first number in the phrase within `range`.
    fn number_in(&self, range: std::ops::RangeInclusive<f64>) -> Option<f64> {
        self.lower
            .iter()
            .filter_map(|w| number(w))
            .find(|value| range.contains(value))
    }

    /// The number right before "bar"/"bars" (or written "16-bar").
    fn bars(&self) -> Option<u32> {
        for (index, word) in self.lower.iter().enumerate() {
            if word.ends_with("bar") || word.ends_with("bars") {
                if let Some(value) = number(word) {
                    return Some(value as u32);
                }
                if index > 0 {
                    let before = &self.lower[index - 1];
                    if let Some(value) = number(before).or_else(|| word_number(before)) {
                        if value >= 1.0 {
                            return Some(value as u32);
                        }
                    }
                }
            }
        }
        None
    }

    /// A number following `marker` ("seed 42").
    fn after(&self, marker: &str) -> Option<f64> {
        let index = self.position(marker)?;
        self.lower.get(index + 1).and_then(|w| number(w))
    }

    /// Original-case words from `from` to the end, joined.
    fn tail_original(&self, from: usize) -> String {
        self.original[from.min(self.original.len())..].join(" ")
    }
}

/// Strips articles and generic nouns from a track phrase.
fn track_phrase(words: &[String]) -> Option<String> {
    let kept: Vec<&str> = words
        .iter()
        .map(String::as_str)
        .filter(|w| {
            !matches!(
                *w,
                "the" | "a" | "an" | "my" | "our" | "track" | "channel" | "part" | "sound" | "line"
            )
        })
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" "))
    }
}

/// Words between index `start` (exclusive) and the first of `stops`.
fn until(words: &Words, start: usize, stops: &[&str]) -> (Vec<String>, Option<usize>) {
    let mut taken = Vec::new();
    for (index, word) in words.lower.iter().enumerate().skip(start + 1) {
        if stops.contains(&word.as_str()) || number(word).is_some() {
            return (taken, Some(index));
        }
        taken.push(word.clone());
    }
    (taken, None)
}

fn key_in(words: &Words) -> Option<String> {
    let index = words.position("in")?;
    // Try the longest key name first: "c harmonic minor", "f# minor", "am".
    for take in (1..=3).rev() {
        if index + take < words.lower.len() + 1 {
            let candidate =
                words.lower[index + 1..(index + 1 + take).min(words.lower.len())].join(" ");
            if candidate.parse::<Key>().is_ok() {
                return Some(candidate);
            }
        }
    }
    None
}

fn seed(words: &Words) -> Option<u64> {
    words
        .after("seed")
        .filter(|value| *value >= 0.0)
        .map(|value| value as u64)
}

fn resolve(project: &Project, phrase: &str) -> Result<u64, ParseError> {
    TrackRef::Name(phrase.to_owned())
        .resolve(project)
        .map(|id| id.get())
        .map_err(|error| ParseError(error.to_string()))
}

type Matcher = fn(&Words, &Project) -> Option<Result<Vec<ToolCall>, ParseError>>;

fn undo_redo(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    match words.lower.first().map(String::as_str) {
        Some("undo") => Some(Ok(vec![ToolCall::new("undo", json!({}))])),
        Some("redo") => Some(Ok(vec![ToolCall::new("redo", json!({}))])),
        _ => None,
    }
}

fn save(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    (words.lower.first().map(String::as_str) == Some("save"))
        .then(|| Ok(vec![ToolCall::new("save_project", json!({}))]))
}

fn rename(words: &Words, project: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let first = words.lower.first()?;
    if !matches!(first.as_str(), "rename" | "call" | "name") {
        return None;
    }
    let to = words.position_any(&["to", "as"]).or_else(|| {
        (first != "rename").then(|| {
            // "call the project Deep Dive": the name follows "project".
            words.position("project")
        })?
    })?;
    let name = words.tail_original(to + 1);
    if name.is_empty() {
        return Some(Err(ParseError("rename to what?".to_owned())));
    }
    let subject = &words.lower[1..to];
    if subject.iter().any(|w| w == "project" || w == "song") || subject.is_empty() {
        return Some(Ok(vec![ToolCall::new(
            "rename_project",
            json!({ "name": name }),
        )]));
    }
    let phrase = track_phrase(subject)?;
    Some(resolve(project, &phrase).map(|track| {
        vec![ToolCall::new(
            "rename_track",
            json!({ "track": track, "name": name }),
        )]
    }))
}

fn tempo(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    if !words.has_any(&["tempo", "bpm"]) && !words.lower.iter().any(|w| w.ends_with("bpm")) {
        return None;
    }
    if words.has_any(ADD_VERBS) && words.has_any(&["beat", "drums", "drum", "groove", "bassline"]) {
        return None;
    }
    Some(
        words
            .number_in(20.0..=999.0)
            .map(|bpm| vec![ToolCall::new("set_tempo", json!({ "bpm": bpm }))])
            .ok_or_else(|| ParseError("what tempo? say \"set tempo to 124\"".to_owned())),
    )
}

fn swing(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let at = words.position_any(&["swing", "shuffle"])?;
    let Some(percent) = words.number_in(50.0..=75.0) else {
        return Some(Err(ParseError(
            "swing takes 50 to 75 percent, as in \"swing the hats to 56%\"".to_owned(),
        )));
    };
    let mut args = Map::new();
    args.insert("percent".to_owned(), json!(percent));
    // "swing the hats to 56%": the words between "swing" and "to".
    let (between, _) = until(words, at, &["to", "at", "by", "of", "on", "for"]);
    let mut track = track_phrase(&between).filter(|phrase| {
        !matches!(
            phrase.as_str(),
            "amount" | "it" | "everything" | "all" | "percent" | "feel"
        )
    });
    // "add 56% swing to the hats", "set swing to 58% on the drums".
    if track.is_none() {
        let number_at = words.lower.iter().rposition(|w| number(w).is_some());
        if let Some(index) = words
            .lower
            .iter()
            .enumerate()
            .skip(number_at.unwrap_or(at).max(at))
            .find(|(_, w)| matches!(w.as_str(), "to" | "on" | "for"))
            .map(|(index, _)| index)
        {
            track = track_phrase(&words.lower[index + 1..]);
        }
    }
    if words.has("eighths") || words.contains_text("8th") || words.has("eighth") {
        args.insert("grid".to_owned(), json!("eighth"));
    } else if words.has("sixteenths") || words.contains_text("16th") {
        args.insert("grid".to_owned(), json!("sixteenth"));
    }
    if let Some(track) = track {
        args.insert("track".to_owned(), json!(track));
    }
    Some(Ok(vec![ToolCall::new("set_swing", Value::Object(args))]))
}

fn sidechain(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let joined = words.lower.join(" ");
    let verb = if joined.contains("side chain") {
        words.position("side").map(|i| i + 1)
    } else {
        words.position_any(&["sidechain", "side-chain", "duck", "ducks", "pump"])
    }?;
    let connectors = ["to", "from", "with", "by", "off", "under", "against", "on"];
    let (target, connector) = until(words, verb, &connectors);
    let (track, key) = match connector {
        Some(index) => (
            track_phrase(&target),
            track_phrase(&words.lower[index + 1..]),
        ),
        // "make the kick duck the pad": the key comes before the verb.
        None => (
            track_phrase(&target),
            track_phrase(
                &words.lower[..verb]
                    .iter()
                    .filter(|w| !ADD_VERBS.contains(&w.as_str()) && *w != "let")
                    .cloned()
                    .collect::<Vec<_>>(),
            ),
        ),
    };
    let (Some(track), Some(key)) = (track, key) else {
        return Some(Err(ParseError(
            "sidechain what to what? say \"sidechain the pad to the kick\"".to_owned(),
        )));
    };
    let mut args = Map::new();
    args.insert("track".to_owned(), json!(track));
    args.insert(
        "key_track".to_owned(),
        json!(key.trim_end_matches(" hard").trim_end_matches(" lightly")),
    );
    if words.has_any(&["hard", "heavy", "heavily", "lots"]) {
        args.insert("depth".to_owned(), json!(0.95));
    } else if words.has_any(&["light", "lightly", "subtle", "gentle", "gently", "bit"]) {
        args.insert("depth".to_owned(), json!(0.4));
    }
    Some(Ok(vec![ToolCall::new("sidechain", Value::Object(args))]))
}

fn mute_solo(words: &Words, project: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let first = words.lower.first()?.as_str();
    let (name, field, value) = match first {
        "mute" => ("set_track_mute", "mute", true),
        "unmute" => ("set_track_mute", "mute", false),
        "solo" => ("set_track_solo", "solo", true),
        "unsolo" => ("set_track_solo", "solo", false),
        _ => return None,
    };
    let phrase = track_phrase(&words.lower[1..])?;
    Some(
        resolve(project, &phrase)
            .map(|track| vec![ToolCall::new(name, json!({ "track": track, field: value }))]),
    )
}

fn gain(words: &Words, project: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let louder = words.has_any(&["up", "louder", "boost", "raise"]);
    let quieter = words.has_any(&["down", "quieter", "softer", "lower", "cut", "reduce"]);
    let absolute = words.has_any(&["volume", "gain", "level", "fader"]) && words.has("to");
    if !(words.has("turn") || words.has("make") || louder || quieter || absolute) {
        return None;
    }
    if !(louder || quieter || absolute) {
        return None;
    }
    let skip = [
        "turn", "make", "up", "down", "louder", "quieter", "softer", "boost", "raise", "lower",
        "cut", "reduce", "set", "volume", "gain", "level", "fader", "by", "to", "of", "db", "the",
        "a", "bit", "little",
    ];
    let phrase_words: Vec<String> = words
        .lower
        .iter()
        .filter(|w| !skip.contains(&w.as_str()) && number(w).is_none())
        .cloned()
        .collect();
    let phrase = track_phrase(&phrase_words)?;
    let track = match resolve(project, &phrase) {
        Ok(track) => track,
        Err(error) => return Some(Err(error)),
    };
    let current = project
        .track(fourfloor_core::TrackId::new(track))
        .map_or(0.0, |t| f64::from(t.gain_db));
    let amount = words.number_in(-96.0..=24.0);
    let gain_db = if absolute && !(louder || quieter) {
        match amount {
            Some(value) => value,
            None => return Some(Err(ParseError("set the level to how many dB?".to_owned()))),
        }
    } else {
        let step = amount.map_or(3.0, f64::abs);
        if quieter {
            current - step
        } else {
            current + step
        }
    };
    Some(Ok(vec![ToolCall::new(
        "set_track_gain",
        json!({ "track": track, "gain_db": gain_db.clamp(-96.0, 24.0) }),
    )]))
}

fn pan(words: &Words, project: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    if words.lower.first().map(String::as_str) != Some("pan") {
        return None;
    }
    let side = if words.has("left") {
        -1.0
    } else if words.has("right") {
        1.0
    } else if words.has_any(&["center", "centre", "middle"]) {
        0.0
    } else {
        return Some(Err(ParseError("pan left, right or centre?".to_owned())));
    };
    let amount = words.number_in(0.0..=100.0).map_or(50.0, |v| v) / 100.0;
    let skip = [
        "pan", "left", "right", "center", "centre", "middle", "to", "the", "hard", "by", "a", "bit",
    ];
    let phrase_words: Vec<String> = words
        .lower
        .iter()
        .filter(|w| !skip.contains(&w.as_str()) && number(w).is_none())
        .cloned()
        .collect();
    let phrase = track_phrase(&phrase_words)?;
    let amount = if words.has("hard") { 1.0 } else { amount };
    Some(resolve(project, &phrase).map(|track| {
        vec![ToolCall::new(
            "set_track_pan",
            json!({ "track": track, "pan": side * amount }),
        )]
    }))
}

fn common_part_args(words: &Words, args: &mut Map<String, Value>) {
    if let Some(bars) = words.bars() {
        args.insert("bars".to_owned(), json!(bars));
    }
    if let Some(seed) = seed(words) {
        args.insert("seed".to_owned(), json!(seed));
    }
    if words.has_any(&["busy", "busier", "dense", "lots"]) {
        args.insert("density".to_owned(), json!(0.85));
    } else if words.has_any(&["sparse", "minimal", "simple", "sparser"]) {
        args.insert("density".to_owned(), json!(0.2));
    }
}

fn drums(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    if !words.has_any(ADD_VERBS)
        || !(words.has_any(&["beat", "drums", "drum", "groove", "rhythm"])
            || words.contains_text("four on the floor"))
    {
        return None;
    }
    let mut args = Map::new();
    common_part_args(words, &mut args);
    let style = if words.has("deep") {
        "deep"
    } else if words.has_any(&["tech", "techy"]) {
        "tech"
    } else if words.has_any(&["jackin", "jacking", "jackin'"]) {
        "jackin"
    } else {
        "classic"
    };
    args.insert("style".to_owned(), json!(style));
    if words.contains_text("no fill") || words.contains_text("without fill") {
        args.insert("fills".to_owned(), json!(false));
    }
    Some(Ok(vec![ToolCall::new(
        "add_house_drum_pattern",
        Value::Object(args),
    )]))
}

fn bass(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    if !words.has_any(ADD_VERBS) || !words.lower.iter().any(|w| w.starts_with("bass")) {
        return None;
    }
    let mut args = Map::new();
    common_part_args(words, &mut args);
    let style = if words.has("rolling") {
        "rolling"
    } else if words.has_any(&["octave", "octaves"]) {
        "octave"
    } else if words.has_any(&["walking", "moving", "melodic"]) {
        "walking"
    } else {
        "offbeat"
    };
    args.insert("style".to_owned(), json!(style));
    if let Some(key) = key_in(words) {
        args.insert("key".to_owned(), json!(key));
    }
    Some(Ok(vec![ToolCall::new("add_bassline", Value::Object(args))]))
}

fn chords(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let nouns = [
        "chord",
        "chords",
        "organ",
        "organs",
        "stab",
        "stabs",
        "pad",
        "pads",
        "keys",
        "progression",
    ];
    if !words.has_any(ADD_VERBS) || !words.has_any(&nouns) {
        return None;
    }
    let mut args = Map::new();
    common_part_args(words, &mut args);
    let style = if words.has_any(&["pad", "pads", "sustained", "held", "lush"]) {
        "sustained"
    } else if words.has_any(&["pulse", "pulsing", "pulses"]) {
        "pulse"
    } else if words.has_any(&["stab", "stabs"]) {
        "stab"
    } else {
        "organ"
    };
    args.insert("style".to_owned(), json!(style));
    if let Some(key) = key_in(words) {
        args.insert("key".to_owned(), json!(key));
    }
    Some(Ok(vec![ToolCall::new("add_chords", Value::Object(args))]))
}

fn riser(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let filtered = words.has_any(&["riser", "sweep", "build", "buildup", "build-up"])
        && (words.has_any(&["riser", "filter"]) || words.contains_text("filter"));
    if !filtered {
        return None;
    }
    let mut args = Map::new();
    args.insert("bars".to_owned(), json!(words.bars().unwrap_or(8)));
    if words.contains_text("low pass") || words.has_any(&["lowpass", "low-pass", "opening", "open"])
    {
        args.insert("mode".to_owned(), json!("low_pass"));
    }
    if let Some(index) = words.position_any(&["on", "over"]) {
        let rest = &words.lower[index + 1..];
        if !rest
            .iter()
            .any(|w| number(w).is_some() || w.starts_with("bar"))
        {
            if let Some(track) = track_phrase(rest) {
                if !matches!(
                    track.as_str(),
                    "everything" | "mix" | "whole mix" | "master" | "all"
                ) {
                    args.insert("track".to_owned(), json!(track));
                }
            }
        }
    }
    if let Some(index) = words.position_any(&["ending", "into", "before", "until"]) {
        if let Some(bar) = words.lower[index + 1..]
            .iter()
            .filter_map(|w| number(w))
            .find(|v| *v >= 1.0)
        {
            args.insert("end_bar".to_owned(), json!(bar as u32));
        }
    }
    Some(Ok(vec![ToolCall::new(
        "add_filter_riser",
        Value::Object(args),
    )]))
}

fn render(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    if words.has("stems") {
        let mut args = Map::new();
        if let Some(lufs) = words
            .number_in(-40.0..=-1.0)
            .filter(|_| words.contains_text("lufs") || words.has_any(&["normalize", "normalise"]))
        {
            args.insert("normalize_lufs".to_owned(), json!(lufs));
        }
        return Some(Ok(vec![ToolCall::new("export_stems", Value::Object(args))]));
    }
    if !words.has_any(&["render", "bounce", "export", "mixdown"]) {
        return None;
    }
    let mut args = Map::new();
    if let Some(path) = words
        .original
        .iter()
        .find(|w| w.to_lowercase().ends_with(".wav"))
    {
        args.insert("path".to_owned(), json!(path));
    }
    if let Some(bars) = words.bars() {
        args.insert("bars".to_owned(), json!(bars));
    }
    if words.contains_text("lufs") || words.has_any(&["normalize", "normalise"]) {
        if let Some(lufs) = words.number_in(-40.0..=-1.0) {
            args.insert("normalize_lufs".to_owned(), json!(lufs));
        }
    }
    Some(Ok(vec![ToolCall::new("render", Value::Object(args))]))
}

fn delete(words: &Words, project: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    let first = words.lower.first()?.as_str();
    if !matches!(first, "delete" | "remove") {
        return None;
    }
    let phrase = track_phrase(&words.lower[1..])?;
    Some(
        resolve(project, &phrase)
            .map(|track| vec![ToolCall::new("delete_track", json!({ "track": track }))]),
    )
}

fn new_track(words: &Words, _: &Project) -> Option<Result<Vec<ToolCall>, ParseError>> {
    if !words.has_any(ADD_VERBS) || !words.has("track") {
        return None;
    }
    let kind = if words.has("audio") {
        "audio"
    } else if words.has_any(&["bus", "group", "return"]) {
        "bus"
    } else {
        "midi"
    };
    let name = match words.position_any(&["called", "named"]) {
        Some(index) => words.tail_original(index + 1),
        None => match kind {
            "audio" => "Audio".to_owned(),
            "bus" => "Bus".to_owned(),
            _ => "MIDI".to_owned(),
        },
    };
    Some(Ok(vec![ToolCall::new(
        "create_track",
        json!({ "name": name, "kind": kind }),
    )]))
}

/// Tried in order; the first that recognises the clause wins.
const MATCHERS: &[Matcher] = &[
    undo_redo, save, rename, sidechain, swing, riser, render, mute_solo, pan, delete, drums, bass,
    chords, new_track, tempo, gain,
];

fn normalize(text: &str) -> String {
    let replaced = text.replace(['\u{2019}', '\u{2018}'], "'");
    let words: Vec<&str> = replaced.split_whitespace().collect();
    // Drop leading filler ("please can you ...") but keep it mid-sentence,
    // where "to" matters.
    let start = words
        .iter()
        .position(|w| !FILLER.contains(&w.to_lowercase().trim_end_matches(',')))
        .unwrap_or(words.len());
    words[start..].join(" ")
}

/// "at 124 bpm" inside a longer clause becomes its own tempo call.
fn split_tempo(words: &Words) -> Option<(Words, f64)> {
    let index = words
        .lower
        .iter()
        .position(|w| w.ends_with("bpm") || w == "bpm")?;
    let (number_index, bpm) =
        if let Some(value) = number(&words.lower[index]).filter(|v| *v >= 20.0) {
            (index, value)
        } else {
            let value = number(words.lower.get(index.checked_sub(1)?)?)?;
            (index - 1, value)
        };
    let start = if number_index > 0 && words.lower[number_index - 1] == "at" {
        number_index - 1
    } else {
        number_index
    };
    let keep = |list: &[String]| -> Vec<String> {
        list[..start]
            .iter()
            .chain(list[index + 1..].iter())
            .cloned()
            .collect()
    };
    let rest = Words {
        lower: keep(&words.lower),
        original: keep(&words.original),
    };
    (!rest.lower.is_empty()).then_some((rest, bpm))
}

fn parse_clause(text: &str, project: &Project) -> Result<Vec<ToolCall>, ParseError> {
    let words = Words::new(&normalize(text));
    if words.lower.is_empty() {
        return Err(ParseError("nothing to do".to_owned()));
    }
    if let Some((rest, bpm)) = split_tempo(&words) {
        if !rest.has("tempo") {
            if let Some(Ok(mut calls)) = MATCHERS.iter().find_map(|m| m(&rest, project)) {
                calls.insert(0, ToolCall::new("set_tempo", json!({ "bpm": bpm })));
                return Ok(calls);
            }
        }
    }
    MATCHERS
        .iter()
        .find_map(|matcher| matcher(&words, project))
        .unwrap_or_else(|| {
            Err(ParseError(format!(
                "I don't understand \"{}\"; {HINT}",
                text.trim()
            )))
        })
}

/// Turns an instruction into tool calls, without running them.
///
/// `project` is used to turn track names into ids for model commands;
/// high-level tools get names and resolve them when they run.
pub fn parse(text: &str, project: &Project) -> Result<Vec<ToolCall>, ParseError> {
    // Split first: "add a bassline and drums" is two parts, even though the
    // drum pattern alone would also match the whole sentence. A split only
    // counts when every part is understood.
    for separator in [" and then ", ", then ", " then ", "; ", ". ", " and ", ", "] {
        if text.contains(separator) {
            let pieces: Vec<&str> = text
                .split(separator)
                .filter(|part| !part.trim().is_empty())
                .collect();
            // "add a bassline and drums": later parts borrow the first
            // part's verb when they have none of their own.
            let first = normalize(pieces.first().copied().unwrap_or_default());
            let verb = first
                .split_whitespace()
                .next()
                .map(str::to_lowercase)
                .filter(|word| ADD_VERBS.contains(&word.as_str()));
            let parts: Result<Vec<Vec<ToolCall>>, ParseError> = pieces
                .iter()
                .map(|part| {
                    parse(part, project).or_else(|error| match &verb {
                        Some(verb) => parse(&format!("{verb} {part}"), project),
                        None => Err(error),
                    })
                })
                .collect();
            if let Ok(parts) = parts {
                return Ok(parts.into_iter().flatten().collect());
            }
        }
    }
    parse_clause(text, project)
}
