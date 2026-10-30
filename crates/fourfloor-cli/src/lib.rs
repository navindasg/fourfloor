//! The `fourfloor` command line: the first end-to-end path through the
//! workspace.
//!
//! - `new` builds a house pattern through the edit model's command bus and
//!   saves a project folder.
//! - `demo` builds a richer project the same way: groups, a return,
//!   note effects, automation, modulation and launcher scenes.
//! - `stems` renders the master and one WAV per track or bus feeding it.
//! - `render` loads a project, compiles it into an engine graph
//!   (`fourfloor-compile`, through the device registry) and renders it
//!   offline to a stereo WAV.
//! - `play` runs the same graph through the default audio device.
//! - `meter` measures a WAV file's loudness.
//! - `analyze` reports an audio file's tempo, key and loudness.
//! - `export` and `import` convert to and from DAWproject.
//! - `library scan` and `library search` drive the sample library.
//! - `plugins scan` and `plugins list` manage the hosted-plugin cache.
//! - `agent serve` and `agent do` run the MCP server and phrase commands.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

pub mod args;
pub mod args_tools;
pub mod commands;
pub mod demo;
pub mod error;
pub mod tools;

use std::ffi::OsString;
use std::io::Write;

use args::Invocation;
pub use error::CliError;

/// The line printed by `fourfloor --version`, e.g. `fourfloor 0.1.0`.
pub fn version_line() -> String {
    format!("fourfloor {}", env!("CARGO_PKG_VERSION"))
}

/// Linear amplitude as decibels relative to full scale.
fn dbfs(linear: f32) -> f64 {
    20.0 * f64::from(linear).log10()
}

fn print_warnings(out: &mut dyn Write, warnings: &[String]) -> std::io::Result<()> {
    warnings
        .iter()
        .try_for_each(|warning| writeln!(out, "note: {warning}"))
}

fn print_normalized(
    out: &mut dyn Write,
    normalized: Option<fourfloor_compile::Normalized>,
) -> std::io::Result<()> {
    match normalized {
        Some(result) => writeln!(
            out,
            "normalised from {:.2} LUFS: gain {:+.2} dB{}",
            result.measured_lufs,
            result.gain_db,
            if result.peak_limited {
                " (held back by the -1 dBTP ceiling)"
            } else {
                ""
            }
        ),
        None => Ok(()),
    }
}

fn dispatch(invocation: Invocation, out: &mut dyn Write) -> Result<std::io::Result<()>, CliError> {
    Ok(match invocation {
        Invocation::Help => write!(out, "{}", args::USAGE),
        Invocation::Version => writeln!(out, "{}", version_line()),
        Invocation::New(options) => {
            let summary = commands::new_project(&options)?;
            writeln!(
                out,
                "created \"{}\" in {}: {} bars at {} BPM, {} tracks, {} notes",
                summary.name,
                options.dir.display(),
                summary.bars,
                summary.bpm,
                summary.tracks,
                summary.notes
            )
        }
        Invocation::Demo(options) => {
            let summary = demo::demo_project(&options)?;
            writeln!(
                out,
                "created demo \"{}\" in {}: {} bars at {} BPM, {} tracks, {} notes",
                summary.name,
                options.dir.display(),
                summary.bars,
                summary.bpm,
                summary.tracks,
                summary.notes
            )
        }
        Invocation::Stems(options) => {
            let summary = commands::stems(&options)?;
            print_warnings(out, &summary.warnings)
                .and_then(|()| print_normalized(out, summary.normalized))
                .and_then(|()| {
                    summary
                        .files
                        .iter()
                        .try_for_each(|file| writeln!(out, "wrote {}", file.display()))
                })
                .and_then(|()| {
                    writeln!(
                        out,
                        "{} files, {:.2} s each at {} Hz",
                        summary.files.len(),
                        summary.frames as f64 / summary.sample_rate.as_f64(),
                        summary.sample_rate.hz()
                    )
                })
        }
        Invocation::Render(options) => {
            let summary = commands::render(&options)?;
            print_warnings(out, &summary.warnings)
                .and_then(|()| print_normalized(out, summary.normalized))
                .and_then(|()| {
                    writeln!(
                        out,
                        "wrote {}: {:.2} s, {} Hz, {} channels, {} tracks, peak {:.1} dBFS",
                        options.output.display(),
                        summary.frames as f64 / summary.sample_rate.as_f64(),
                        summary.sample_rate.hz(),
                        summary.channels,
                        summary.sounding_tracks,
                        dbfs(summary.peak)
                    )
                })
        }
        Invocation::Play(options) => {
            let summary = commands::play(&options)?;
            print_warnings(out, &summary.warnings).and_then(|()| {
                writeln!(
                    out,
                    "played {:.2} s at {} Hz on {} channels, peak {:.1} dBFS, {} stream errors",
                    summary.seconds,
                    summary.sample_rate.hz(),
                    summary.device_channels,
                    dbfs(summary.peak),
                    summary.stream_errors
                )
            })
        }
        Invocation::Tool(tool) => {
            let summary = tools::run_tool(tool)?;
            tools::print(out, &summary)
        }
        Invocation::Meter { path } => {
            let summary = commands::meter(&path)?;
            writeln!(
                out,
                "{}: {} channels, {} Hz, {:.2} s\n\
                 integrated {:.2} LUFS, true peak {:.2} dBTP, sample peak {:.2} dBFS",
                path.display(),
                summary.channels,
                summary.sample_rate.hz(),
                summary.frames as f64 / summary.sample_rate.as_f64(),
                summary.loudness.integrated_lufs,
                summary.loudness.true_peak_dbtp,
                summary.loudness.sample_peak_dbfs
            )
        }
    })
}

/// Runs one invocation. `args` are the arguments after the program name;
/// results are written to `out`.
pub fn run(args: impl IntoIterator<Item = OsString>, out: &mut dyn Write) -> Result<(), CliError> {
    let invocation = args::parse(args)?;
    dispatch(invocation, out)?.map_err(|source| CliError::Io {
        action: "write to",
        path: "standard output".into(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_str(args: &[&str]) -> Result<String, CliError> {
        let mut out = Vec::new();
        run(args.iter().map(OsString::from), &mut out)?;
        Ok(String::from_utf8(out).expect("output is UTF-8"))
    }

    #[test]
    fn version_line_names_the_binary_and_version() {
        let line = version_line();
        assert!(line.starts_with("fourfloor "));
        assert!(line.ends_with(env!("CARGO_PKG_VERSION")));
        assert_eq!(run_str(&["--version"]).unwrap().trim_end(), line);
    }

    #[test]
    fn help_lists_every_subcommand() {
        let help = run_str(&[]).unwrap();
        for command in [
            "new", "demo", "render", "stems", "play", "meter", "analyze", "export", "import",
            "library", "plugins", "agent",
        ] {
            assert!(help.contains(&format!("fourfloor {command} ")), "{command}");
        }
    }

    #[test]
    fn usage_errors_are_flagged_as_such() {
        let error = run_str(&["bogus"]).unwrap_err();
        assert!(error.is_usage());
        let error = run_str(&["meter", "/definitely/not/here.wav"]).unwrap_err();
        assert!(!error.is_usage());
    }

    #[test]
    fn dbfs_of_full_scale_is_zero() {
        assert_eq!(dbfs(1.0), 0.0);
        assert!((dbfs(0.5) + 6.0206).abs() < 1e-3);
    }
}
