//! The AI panel's mix report: render the project offline, measure it, and
//! compare it with the loaded reference track when there is one.

use std::path::PathBuf;
use std::sync::Arc;

use fourfloor_analysis::{compare_to_reference, measure_levels, LevelReport, ReferenceReport};
use fourfloor_audio::AudioBuffer;
use fourfloor_core::{DeviceRegistry, SampleRate};
use fourfloor_model::Project;

use super::callback::ReferenceAudio;
use super::Progress;
use crate::project::files::render_mix;
use crate::views::mixer::db_text;

/// What the report found.
#[derive(Clone, Debug, PartialEq)]
pub struct MixReport {
    /// Levels of the mix.
    pub mix: LevelReport,
    /// The comparison, when a reference was loaded and could be measured.
    pub reference: Option<ReferenceReport>,
    /// Name of the reference.
    pub reference_name: Option<String>,
    /// The report as lines of text, for the chat log.
    pub lines: Vec<String>,
}

/// Lines describing the mix's own levels.
pub fn level_lines(levels: &LevelReport) -> Vec<String> {
    vec![
        format!(
            "Loudness {} (range {:.1} LU), true peak {}",
            db_text(levels.integrated_lufs, "LUFS"),
            levels.loudness_range_lu,
            db_text(levels.true_peak_dbtp, "dBTP"),
        ),
        format!(
            "Crest {:.1} dB, correlation {:+.2}, width {:.2}",
            levels.crest_factor_db, levels.correlation, levels.width
        ),
    ]
}

/// Lines comparing the mix with a reference: band deltas, then the
/// analysis crate's suggestions.
pub fn reference_lines(report: &ReferenceReport) -> Vec<String> {
    let mut lines = vec![format!(
        "Mix is {:+.1} LU against the reference ({:+.1} dB would match it)",
        report.loudness_delta_lu, report.gain_to_match_db
    )];
    for (name, band) in [
        ("Low", &report.low),
        ("Mid", &report.mid),
        ("High", &report.high),
    ] {
        lines.push(format!(
            "{name} {:.0}-{:.0} Hz: mix {:+.1} dB vs reference",
            band.low_hz, band.high_hz, band.delta_db
        ));
    }
    lines.push(format!(
        "Width {:+.2}, crest {:+.1} dB vs reference",
        report.width_delta, report.crest_delta_db
    ));
    if report.suggestions.is_empty() {
        lines.push("No changes suggested: the mix is close to the reference.".to_owned());
    }
    lines.extend(
        report
            .suggestions
            .iter()
            .map(|s| format!("- {}", s.message)),
    );
    lines
}

/// A loaded reference handed to the report job.
#[derive(Clone, Debug)]
pub struct ReferenceInput {
    /// File name.
    pub name: String,
    /// Stereo audio at `rate`.
    pub audio: Arc<ReferenceAudio>,
}

/// Job: render, measure and (with a reference) compare.
pub fn mix_report_job(
    project: Project,
    dir: Option<PathBuf>,
    registry: Arc<DeviceRegistry>,
    rate: SampleRate,
    reference: Option<ReferenceInput>,
) -> impl FnOnce(&Progress) -> Result<MixReport, String> + Send + 'static {
    move |progress| {
        progress.set(0.1);
        let mix = render_mix(&project, &registry, dir.as_deref(), rate)?;
        progress.set(0.7);
        let levels = measure_levels(&mix).map_err(|e| format!("could not measure the mix: {e}"))?;
        let mut lines = level_lines(&levels);
        let mut compared = None;
        let reference_name = reference.as_ref().map(|r| r.name.clone());
        match reference {
            Some(reference) => {
                let audio = AudioBuffer::from_interleaved(rate, 2, &reference.audio.samples);
                match compare_to_reference(&mix, &audio) {
                    Ok(report) => {
                        lines.push(format!("Against {}:", reference.name));
                        lines.extend(reference_lines(&report));
                        compared = Some(report);
                    }
                    Err(error) => {
                        lines.push(format!("Could not compare with the reference: {error}"))
                    }
                }
            }
            None => lines.push(
                "Load a reference track in the master panel to compare against it.".to_owned(),
            ),
        }
        Ok(MixReport {
            mix: levels,
            reference: compared,
            reference_name,
            lines,
        })
    }
}
