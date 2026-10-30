//! `fourfloor-app`: the desktop application.
//!
//! ```text
//! fourfloor-app [PROJECT_DIR] [--no-audio] [--plugins] [--smoke [FRAMES]]
//! ```

use std::path::PathBuf;
use std::process::ExitCode;

use fourfloor_app::services::OutputMode;
use fourfloor_app::{AppConfig, FourfloorApp};

/// Parsed command line.
#[derive(Debug, Default, PartialEq)]
struct Args {
    project: Option<PathBuf>,
    no_audio: bool,
    plugins: bool,
    smoke: Option<u64>,
}

fn parse(args: &[String]) -> Result<Args, String> {
    let mut parsed = Args::default();
    let mut iter = args.iter().peekable();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--no-audio" => parsed.no_audio = true,
            "--plugins" => parsed.plugins = true,
            "--smoke" => {
                let frames = match iter.peek().and_then(|next| next.parse::<u64>().ok()) {
                    Some(frames) => {
                        iter.next();
                        frames
                    }
                    None => 240,
                };
                parsed.smoke = Some(frames.clamp(1, 1_000_000));
            }
            "-h" | "--help" => return Err(usage()),
            flag if flag.starts_with('-') => {
                return Err(format!("unknown option {flag}\n{}", usage()))
            }
            path => parsed.project = Some(PathBuf::from(path)),
        }
    }
    Ok(parsed)
}

fn usage() -> String {
    "usage: fourfloor-app [PROJECT_DIR] [--no-audio] [--plugins] [--smoke [FRAMES]]".to_owned()
}

#[cfg(feature = "audio")]
fn output(no_audio: bool) -> OutputMode {
    if no_audio {
        AppConfig::headless().output
    } else {
        OutputMode::Device(Default::default())
    }
}

#[cfg(not(feature = "audio"))]
fn output(_no_audio: bool) -> OutputMode {
    AppConfig::headless().output
}

fn main() -> ExitCode {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let args = match parse(&raw) {
        Ok(args) => args,
        Err(message) => {
            eprintln!("{message}");
            return ExitCode::from(2);
        }
    };
    if let Some(frames) = args.smoke {
        return match fourfloor_app::smoke::run(frames) {
            Ok(report) => {
                println!(
                    "smoke ok: {} frames, {} tracks, playhead {:.2} beats, peak {:.3}, {:.1} LUFS, {} warnings, {} DJ decks, {} assistant edits",
                    report.frames, report.tracks, report.beats, report.max_peak, report.integrated_lufs, report.warnings, report.dj_decks, report.assistant_edits
                );
                ExitCode::SUCCESS
            }
            Err(error) => {
                eprintln!("smoke failed: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let config = AppConfig {
        output: output(args.no_audio),
        project_dir: args.project,
        with_plugins: args.plugins,
        native_dialogs: cfg!(feature = "dialogs"),
        paths: fourfloor_app::state::FeaturePaths::user(),
    };
    let app = match FourfloorApp::new(&config) {
        Ok(app) => app,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("fourfloor")
            .with_inner_size([1500.0, 920.0])
            .with_min_inner_size([900.0, 600.0]),
        ..Default::default()
    };
    match eframe::run_native("fourfloor", options, Box::new(|_cc| Ok(Box::new(app)))) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_parse() {
        assert_eq!(parse(&strings(&[])), Ok(Args::default()));
        let args = parse(&strings(&["song", "--no-audio", "--smoke", "30"])).expect("parses");
        assert_eq!(args.project, Some(PathBuf::from("song")));
        assert!(args.no_audio);
        assert_eq!(args.smoke, Some(30));
        assert_eq!(
            parse(&strings(&["--smoke"])).expect("parses").smoke,
            Some(240)
        );
        assert!(parse(&strings(&["--bogus"])).is_err());
    }
}
