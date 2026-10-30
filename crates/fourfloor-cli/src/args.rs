//! Argument parsing for the `fourfloor` binary. Hand-written: a handful of
//! subcommands do not need a parser dependency.

use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;

use crate::error::CliError;

/// Text printed by `fourfloor --help`.
pub const USAGE: &str = "\
fourfloor: an open-source DAW for house music (early command-line build)

Usage:
  fourfloor new <dir> [--name <text>] [--bpm <20-999>] [--bars <1-64>] [--swing <50-75>]
      Create a project folder holding a house pattern (kick and hats).
  fourfloor demo <dir> [--name <text>] [--bpm <20-999>]
      Create a full house track (28 bars: intro, build, drop) through edit
      commands: 909 drums with swing, offbeat FM bass, a 303 line, pad
      chords ducked by the kick, reverb and delay sends, a filter riser and
      a true-peak limiter on a Master bus.
  fourfloor render <dir> -o <out.wav> [--rate <hz>] [--tail <seconds>] [--normalize <lufs>]
      Render the project offline to a 32-bit float stereo WAV file.
  fourfloor stems <dir> -o <out-dir> [--rate <hz>] [--tail <seconds>] [--normalize <lufs>]
      Render master.wav plus one WAV per track or bus that feeds the master.
  fourfloor play <dir> [--loops <n>] [--volume <0-1>]
      Play the project through the default audio output.
  fourfloor meter <file.wav>
      Print integrated loudness (LUFS), true peak and sample peak.
  fourfloor analyze <audio-file>
      Print tempo (BPM), key and loudness of a WAV, FLAC, AIFF, MP3 or Ogg file.
  fourfloor export <dir> -o <file.dawproject>
      Write the project as a DAWproject file and list what did not fit.
  fourfloor import <file.dawproject> <new-dir>
      Read a DAWproject file into a new project folder.
  fourfloor library scan <folder>... [--db <file>]
      Index audio files (tempo, key, kind, loudness) into the library.
  fourfloor library search [words...] [--bpm <lo-hi>] [--key <Am|8A>] [--kind <one-shot|loop|track>] [--limit <n>] [--db <file>]
      Search the library.
  fourfloor plugins scan | list
      Scan installed CLAP plugins and Audio Units into the plugin cache, or list it.
  fourfloor agent serve <dir> [--http [ADDR]] [--no-autosave]
      Serve the project to AI agents over MCP (stdio, or HTTP on localhost).
  fourfloor agent do <dir> <instruction...> [--llm <url> --model <name>]
      Run a plain-language instruction against the project.
  fourfloor --version | --help
";

/// Default tempo for `new`, in BPM.
pub const DEFAULT_BPM: f64 = 124.0;
/// Default pattern length for `new`, in bars.
pub const DEFAULT_BARS: u32 = 4;
/// Default render sample rate, in Hz.
pub const DEFAULT_RATE_HZ: u32 = 48_000;
/// Default silence-free tail after the last clip, in seconds.
pub const DEFAULT_TAIL_SECONDS: f64 = 0.5;

const RATE_RANGE: std::ops::RangeInclusive<u32> = 8_000..=192_000;
const MAX_TAIL_SECONDS: f64 = 60.0;
const MAX_LOOPS: u32 = 1_000;
const LUFS_RANGE: std::ops::RangeInclusive<f64> = -40.0..=-5.0;

/// Options for `fourfloor new`.
#[derive(Clone, Debug, PartialEq)]
pub struct NewOptions {
    /// Project folder to create.
    pub dir: PathBuf,
    /// Project name; defaults to the folder name.
    pub name: Option<String>,
    /// Tempo in BPM.
    pub bpm: f64,
    /// Pattern length in bars.
    pub bars: u32,
    /// Swing percentage, when given.
    pub swing: Option<f64>,
}

/// Options for `fourfloor render`.
#[derive(Clone, Debug, PartialEq)]
pub struct RenderOptions {
    /// Project folder to read.
    pub dir: PathBuf,
    /// WAV file to write.
    pub output: PathBuf,
    /// Sample rate in Hz.
    pub rate_hz: u32,
    /// Extra time after the last clip so tails ring out, in seconds.
    pub tail_seconds: f64,
    /// Integrated loudness to normalise to, in LUFS, when given.
    pub normalize_lufs: Option<f64>,
}

/// Options for `fourfloor stems`. The fields mean what they do in
/// [`RenderOptions`], except that `output` is a directory.
pub type StemsOptions = RenderOptions;

/// Options for `fourfloor demo`.
#[derive(Clone, Debug, PartialEq)]
pub struct DemoOptions {
    /// Project folder to create.
    pub dir: PathBuf,
    /// Project name; defaults to the folder name.
    pub name: Option<String>,
    /// Tempo in BPM.
    pub bpm: f64,
}

/// Options for `fourfloor play`.
#[derive(Clone, Debug, PartialEq)]
pub struct PlayOptions {
    /// Project folder to read.
    pub dir: PathBuf,
    /// How many times to play the project through.
    pub loops: u32,
    /// Linear output volume, 0 to 1.
    pub volume: f32,
}

/// One parsed command line.
#[derive(Clone, Debug, PartialEq)]
pub enum Invocation {
    /// Print the usage text.
    Help,
    /// Print the version.
    Version,
    /// Create a project.
    New(NewOptions),
    /// Create the demo project.
    Demo(DemoOptions),
    /// Render a project to WAV.
    Render(RenderOptions),
    /// Render a project's master and stems into a directory.
    Stems(StemsOptions),
    /// Play a project.
    Play(PlayOptions),
    /// Measure a WAV file.
    Meter {
        /// The file to measure.
        path: PathBuf,
    },
    /// One of the analysis, interchange, library, plugin or agent commands.
    Tool(crate::args_tools::ToolInvocation),
}

fn usage(message: impl Into<String>) -> CliError {
    CliError::Usage(message.into())
}

/// The arguments after the subcommand, split into the one positional
/// argument and `--flag value` pairs.
struct Rest {
    positional: Option<OsString>,
    flags: Vec<(String, OsString)>,
}

impl Rest {
    fn split(command: &str, args: Vec<OsString>, known: &[&str]) -> Result<Self, CliError> {
        let mut positional = None;
        let mut flags = Vec::new();
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            let text = arg.to_string_lossy();
            if text.starts_with('-') && text.len() > 1 {
                let flag = text.into_owned();
                if !known.contains(&flag.as_str()) {
                    return Err(usage(format!("unknown option {flag} for `{command}`")));
                }
                let value = args
                    .next()
                    .ok_or_else(|| usage(format!("{flag} needs a value")))?;
                if flags.iter().any(|(seen, _)| *seen == flag) {
                    return Err(usage(format!("{flag} was given twice")));
                }
                flags.push((flag, value));
            } else if positional.is_none() {
                positional = Some(arg);
            } else {
                return Err(usage(format!(
                    "unexpected argument {} for `{command}`",
                    arg.to_string_lossy()
                )));
            }
        }
        Ok(Self { positional, flags })
    }

    fn path(&mut self, command: &str, what: &str) -> Result<PathBuf, CliError> {
        self.positional
            .take()
            .map(PathBuf::from)
            .ok_or_else(|| usage(format!("`{command}` needs {what}")))
    }

    fn raw(&self, flag: &str) -> Option<&OsString> {
        self.flags
            .iter()
            .find(|(name, _)| name == flag)
            .map(|(_, value)| value)
    }

    fn text(&self, flag: &str) -> Result<Option<String>, CliError> {
        self.raw(flag)
            .map(|value| {
                value
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| usage(format!("{flag} is not valid UTF-8")))
            })
            .transpose()
    }

    fn number<T: FromStr>(&self, flag: &str, expected: &str) -> Result<Option<T>, CliError> {
        self.text(flag)?
            .map(|text| {
                text.parse::<T>()
                    .map_err(|_| usage(format!("{flag} expects {expected}, got `{text}`")))
            })
            .transpose()
    }
}

fn parse_new(args: Vec<OsString>) -> Result<Invocation, CliError> {
    let mut rest = Rest::split("new", args, &["--name", "--bpm", "--bars", "--swing"])?;
    // Ranges for tempo, bars and swing are the edit model's to enforce; only
    // reject what is not a number at all.
    let finite = |flag: &str, value: Option<f64>| match value {
        Some(v) if !v.is_finite() => Err(usage(format!("{flag} expects a number"))),
        other => Ok(other),
    };
    Ok(Invocation::New(NewOptions {
        dir: rest.path("new", "a project folder")?,
        name: rest.text("--name")?,
        bpm: finite("--bpm", rest.number("--bpm", "a tempo in BPM")?)?.unwrap_or(DEFAULT_BPM),
        bars: rest
            .number("--bars", "a whole number of bars")?
            .unwrap_or(DEFAULT_BARS),
        swing: finite("--swing", rest.number("--swing", "a percentage")?)?,
    }))
}

fn parse_demo(args: Vec<OsString>) -> Result<Invocation, CliError> {
    let mut rest = Rest::split("demo", args, &["--name", "--bpm"])?;
    let bpm = match rest.number::<f64>("--bpm", "a tempo in BPM")? {
        Some(bpm) if !bpm.is_finite() => return Err(usage("--bpm expects a number")),
        other => other.unwrap_or(DEFAULT_BPM),
    };
    Ok(Invocation::Demo(DemoOptions {
        dir: rest.path("demo", "a project folder")?,
        name: rest.text("--name")?,
        bpm,
    }))
}

/// Parses the options `render` and `stems` share. `target` names what
/// `-o` points at.
fn parse_export(
    command: &str,
    target: &str,
    args: Vec<OsString>,
) -> Result<RenderOptions, CliError> {
    let known = ["-o", "--output", "--rate", "--tail", "--normalize"];
    let mut rest = Rest::split(command, args, &known)?;
    let output = match (rest.raw("-o"), rest.raw("--output")) {
        (Some(_), Some(_)) => return Err(usage("give either -o or --output, not both")),
        (Some(path), None) | (None, Some(path)) => PathBuf::from(path),
        (None, None) => return Err(usage(format!("`{command}` needs -o <{target}>"))),
    };
    let rate_hz = rest
        .number("--rate", "a sample rate in Hz")?
        .unwrap_or(DEFAULT_RATE_HZ);
    if !RATE_RANGE.contains(&rate_hz) {
        return Err(usage(format!(
            "--rate must be between {} and {} Hz",
            RATE_RANGE.start(),
            RATE_RANGE.end()
        )));
    }
    let tail_seconds = rest
        .number("--tail", "a number of seconds")?
        .unwrap_or(DEFAULT_TAIL_SECONDS);
    if !(0.0..=MAX_TAIL_SECONDS).contains(&tail_seconds) {
        return Err(usage(format!(
            "--tail must be between 0 and {MAX_TAIL_SECONDS} seconds"
        )));
    }
    let normalize_lufs: Option<f64> = rest.number("--normalize", "a loudness in LUFS")?;
    if normalize_lufs.is_some_and(|lufs| !LUFS_RANGE.contains(&lufs)) {
        return Err(usage(format!(
            "--normalize must be between {} and {} LUFS",
            LUFS_RANGE.start(),
            LUFS_RANGE.end()
        )));
    }
    Ok(RenderOptions {
        dir: rest.path(command, "a project folder")?,
        output,
        rate_hz,
        tail_seconds,
        normalize_lufs,
    })
}

fn parse_play(args: Vec<OsString>) -> Result<Invocation, CliError> {
    let mut rest = Rest::split("play", args, &["--loops", "--volume"])?;
    let loops = rest.number("--loops", "a whole number")?.unwrap_or(1);
    if !(1..=MAX_LOOPS).contains(&loops) {
        return Err(usage(format!("--loops must be between 1 and {MAX_LOOPS}")));
    }
    let volume: f32 = rest.number("--volume", "a number")?.unwrap_or(1.0);
    if !(0.0..=1.0).contains(&volume) {
        return Err(usage("--volume must be between 0 and 1"));
    }
    Ok(Invocation::Play(PlayOptions {
        dir: rest.path("play", "a project folder")?,
        loops,
        volume,
    }))
}

fn parse_meter(args: Vec<OsString>) -> Result<Invocation, CliError> {
    let mut rest = Rest::split("meter", args, &[])?;
    Ok(Invocation::Meter {
        path: rest.path("meter", "a WAV file")?,
    })
}

/// Parses the arguments that follow the program name.
pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Invocation, CliError> {
    let mut args = args.into_iter();
    let Some(command) = args.next() else {
        return Ok(Invocation::Help);
    };
    let rest: Vec<OsString> = args.collect();
    let command = command.to_string_lossy();
    if let Some(parsed) = crate::args_tools::parse_tool(&command, rest.clone()) {
        return parsed;
    }
    match command.as_ref() {
        "-h" | "--help" | "help" => Ok(Invocation::Help),
        "-V" | "--version" | "version" => Ok(Invocation::Version),
        "new" => parse_new(rest),
        "demo" => parse_demo(rest),
        "render" => parse_export("render", "out.wav", rest).map(Invocation::Render),
        "stems" => parse_export("stems", "out-dir", rest).map(Invocation::Stems),
        "play" => parse_play(rest),
        "meter" => parse_meter(rest),
        other => Err(usage(format!(
            "unknown command `{other}`; try `fourfloor --help`"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(args: &[&str]) -> Result<Invocation, CliError> {
        parse(args.iter().map(OsString::from))
    }

    fn usage_message(args: &[&str]) -> String {
        match parse_str(args) {
            Err(CliError::Usage(message)) => message,
            other => panic!("expected a usage error, got {other:?}"),
        }
    }

    #[test]
    fn no_arguments_and_help_flags_ask_for_help() {
        assert_eq!(parse_str(&[]).unwrap(), Invocation::Help);
        assert_eq!(parse_str(&["--help"]).unwrap(), Invocation::Help);
        assert_eq!(parse_str(&["--version"]).unwrap(), Invocation::Version);
    }

    #[test]
    fn new_has_house_defaults() {
        assert_eq!(
            parse_str(&["new", "track"]).unwrap(),
            Invocation::New(NewOptions {
                dir: PathBuf::from("track"),
                name: None,
                bpm: 124.0,
                bars: 4,
                swing: None,
            })
        );
    }

    #[test]
    fn new_reads_every_option_in_any_order() {
        let parsed = parse_str(&[
            "new", "--bpm", "126.5", "--swing", "58", "dir", "--bars", "8", "--name", "Friday",
        ])
        .unwrap();
        assert_eq!(
            parsed,
            Invocation::New(NewOptions {
                dir: PathBuf::from("dir"),
                name: Some("Friday".to_owned()),
                bpm: 126.5,
                bars: 8,
                swing: Some(58.0),
            })
        );
    }

    #[test]
    fn render_needs_an_output_and_accepts_both_spellings() {
        assert!(usage_message(&["render", "dir"]).contains("-o"));
        for flag in ["-o", "--output"] {
            assert_eq!(
                parse_str(&["render", "dir", flag, "out.wav"]).unwrap(),
                Invocation::Render(RenderOptions {
                    dir: PathBuf::from("dir"),
                    output: PathBuf::from("out.wav"),
                    rate_hz: 48_000,
                    tail_seconds: 0.5,
                    normalize_lufs: None,
                })
            );
        }
        assert!(usage_message(&["render", "d", "-o", "a", "--output", "b"]).contains("not both"));
    }

    #[test]
    fn render_checks_rate_and_tail() {
        assert!(usage_message(&["render", "d", "-o", "a", "--rate", "100"]).contains("--rate"));
        assert!(usage_message(&["render", "d", "-o", "a", "--rate", "fast"]).contains("fast"));
        assert!(usage_message(&["render", "d", "-o", "a", "--tail", "-1"]).contains("--tail"));
        assert!(usage_message(&["render", "d", "-o", "a", "--tail", "NaN"]).contains("--tail"));
    }

    #[test]
    fn stems_and_demo_parse_and_normalize_is_range_checked() {
        assert_eq!(
            parse_str(&["stems", "dir", "-o", "out", "--normalize", "-14"]).unwrap(),
            Invocation::Stems(RenderOptions {
                dir: PathBuf::from("dir"),
                output: PathBuf::from("out"),
                rate_hz: 48_000,
                tail_seconds: 0.5,
                normalize_lufs: Some(-14.0),
            })
        );
        assert!(usage_message(&["stems", "dir"]).contains("out-dir"));
        assert!(usage_message(&["render", "d", "-o", "a", "--normalize", "3"]).contains("LUFS"));
        assert!(usage_message(&["render", "d", "-o", "a", "--normalize", "NaN"]).contains("LUFS"));
        assert_eq!(
            parse_str(&["demo", "dir", "--bpm", "122"]).unwrap(),
            Invocation::Demo(DemoOptions {
                dir: PathBuf::from("dir"),
                name: None,
                bpm: 122.0,
            })
        );
        assert!(usage_message(&["demo"]).contains("project folder"));
    }

    #[test]
    fn play_checks_loops_and_volume() {
        assert_eq!(
            parse_str(&["play", "dir", "--loops", "4", "--volume", "0.25"]).unwrap(),
            Invocation::Play(PlayOptions {
                dir: PathBuf::from("dir"),
                loops: 4,
                volume: 0.25,
            })
        );
        assert!(usage_message(&["play", "dir", "--loops", "0"]).contains("--loops"));
        assert!(usage_message(&["play", "dir", "--volume", "2"]).contains("--volume"));
        assert!(usage_message(&["play", "dir", "--volume", "NaN"]).contains("--volume"));
    }

    #[test]
    fn bad_shapes_are_usage_errors() {
        assert!(usage_message(&["frobnicate"]).contains("unknown command"));
        assert!(usage_message(&["new"]).contains("project folder"));
        assert!(usage_message(&["new", "a", "b"]).contains("unexpected argument"));
        assert!(usage_message(&["new", "a", "--tempo", "120"]).contains("unknown option"));
        assert!(usage_message(&["new", "a", "--bpm"]).contains("needs a value"));
        assert!(usage_message(&["new", "a", "--bpm", "1", "--bpm", "2"]).contains("twice"));
        assert!(usage_message(&["new", "a", "--bpm", "inf"]).contains("--bpm"));
        assert!(usage_message(&["meter"]).contains("WAV file"));
    }
}
