//! Export: writes a fourfloor project as a DAWproject archive.
//!
//! The XML follows the official `Project.xsd` (element order included):
//! tracks are written flat in `Structure`, each with its mixer `Channel`,
//! and buses are told apart by their channel `role`. The model has no
//! master channel, so a unity master track is added and every track that
//! feeds the master output is routed to it.

pub(crate) mod devices;
mod timeline;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use fourfloor_core::TrackId;
use fourfloor_model::{AudioRef, Project, Track, TrackKind};

use crate::container::{Container, EXTRAS_JSON, METADATA_XML, PROJECT_XML};
use crate::error::{io_error, DawprojectError};
use crate::extras::Extras;
use crate::metadata::Metadata;
use crate::naming;
use crate::options::{ExportOptions, Exported};
use crate::report::{Report, Subject};
use crate::units::{db_to_linear, pan_to_normalized, MAX_GAIN_DB};
use crate::xml::{El, XmlWriter};

/// The `Application` name fourfloor writes, and recognizes on import.
pub const APPLICATION_NAME: &str = "fourfloor";
/// The DAWproject format version written.
pub const FORMAT_VERSION: &str = "1.0";

/// What export needs to know about an audio file.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AudioInfo {
    pub channels: usize,
    pub sample_rate: u32,
    pub seconds: f64,
}

pub(crate) struct Exporter<'a> {
    pub project: &'a Project,
    pub dir: Option<&'a Path>,
    pub options: &'a ExportOptions<'a>,
    pub report: Report,
    pub files: Container,
    audio: HashMap<AudioRef, Option<AudioInfo>>,
}

/// Writes `project` as a DAWproject archive.
///
/// `project_dir` is the project folder holding the audio pool and plugin
/// state; without it, audio clips and plugin state cannot be embedded and
/// are listed as missing in the report.
pub fn export_project(
    project: &Project,
    project_dir: Option<&Path>,
    options: &ExportOptions<'_>,
) -> Result<Exported, DawprojectError> {
    project.validate()?;
    let mut exporter = Exporter {
        project,
        dir: project_dir,
        options,
        report: Report::default(),
        files: Container::new(),
        audio: HashMap::new(),
    };
    let xml = exporter.project_xml();
    let Exporter {
        mut report,
        mut files,
        ..
    } = exporter;
    files.insert(PROJECT_XML, xml.into_bytes())?;
    let metadata = match options.metadata {
        Some(metadata) => metadata.clone(),
        None => Metadata::default(),
    };
    let metadata = Metadata {
        title: metadata.title.or_else(|| Some(project.name.clone())),
        ..metadata
    };
    files.insert(METADATA_XML, metadata.to_xml().into_bytes())?;
    let extras = Extras::collect(project, options.extras, &mut report);
    if options.extras {
        files.insert(EXTRAS_JSON, serde_json::to_vec_pretty(&extras)?)?;
    }
    Ok(Exported {
        bytes: files.to_bytes()?,
        report,
    })
}

/// Writes `project` to a `.dawproject` file and returns the report.
pub fn export_to_file(
    project: &Project,
    project_dir: Option<&Path>,
    path: &Path,
    options: &ExportOptions<'_>,
) -> Result<Report, DawprojectError> {
    let exported = export_project(project, project_dir, options)?;
    std::fs::write(path, &exported.bytes).map_err(io_error("write", path))?;
    Ok(exported.report)
}

pub(crate) fn location(track: &Track, what: &str) -> String {
    format!("{} / {what}", track.name)
}

impl Exporter<'_> {
    fn project_xml(&mut self) -> String {
        let mut w = XmlWriter::new();
        w.open(El::new("Project").attr("version", FORMAT_VERSION));
        w.leaf(
            El::new("Application")
                .attr("name", APPLICATION_NAME)
                .attr("version", env!("CARGO_PKG_VERSION")),
        );
        self.transport(&mut w);
        self.structure(&mut w);
        self.arrangement(&mut w);
        self.scenes(&mut w);
        w.finish()
    }

    fn transport(&self, w: &mut XmlWriter) {
        w.open(El::new("Transport"));
        w.leaf(
            El::new("Tempo")
                .attr("id", naming::TEMPO)
                .attr("name", "Tempo")
                .attr("unit", "bpm")
                .attr("min", fourfloor_core::Tempo::MIN_BPM)
                .attr("max", fourfloor_core::Tempo::MAX_BPM)
                .attr("value", self.project.tempo.bpm()),
        );
        // The model has no time signature; everything is in 4/4.
        w.leaf(
            El::new("TimeSignature")
                .attr("id", naming::TIME_SIGNATURE)
                .attr("numerator", 4)
                .attr("denominator", 4),
        );
        w.close();
    }

    fn structure(&mut self, w: &mut XmlWriter) {
        w.open(El::new("Structure"));
        let tracks = self.project.tracks.clone();
        for track in &tracks {
            self.track(w, track);
        }
        w.open(
            El::new("Track")
                .attr("id", naming::MASTER_TRACK)
                .attr("name", "Master")
                .attr("contentType", "audio notes")
                .attr("loaded", true),
        );
        w.open(
            El::new("Channel")
                .attr("id", naming::MASTER_CHANNEL)
                .attr("role", "master")
                .attr("audioChannels", 2)
                .attr("solo", false),
        );
        mixer_params(w, naming::MASTER_MUTE, false, naming::MASTER_PAN, 0.0);
        volume_param(w, naming::MASTER_VOLUME, 0.0);
        w.close();
        w.close();
        w.close();
    }

    fn bus_role(&self, id: TrackId) -> &'static str {
        let fed = self.project.tracks.iter().any(|t| t.output == Some(id));
        let sent = self
            .project
            .tracks
            .iter()
            .any(|t| t.sends.iter().any(|send| send.target == id));
        if sent && !fed {
            "effect"
        } else {
            "submix"
        }
    }

    fn track(&mut self, w: &mut XmlWriter, track: &Track) {
        let (content, role) = match track.kind {
            TrackKind::Midi => ("notes", "regular"),
            TrackKind::Audio => ("audio", "regular"),
            TrackKind::Bus => ("audio", self.bus_role(track.id)),
        };
        w.open(
            El::new("Track")
                .attr("id", naming::track(track.id))
                .attr("name", &track.name)
                .attr("contentType", content)
                .attr("loaded", true),
        );
        let destination = track
            .output
            .map_or_else(|| naming::MASTER_CHANNEL.to_owned(), naming::channel);
        w.open(
            El::new("Channel")
                .attr("id", naming::channel(track.id))
                .attr("role", role)
                .attr("audioChannels", 2)
                .attr("destination", destination)
                .attr("solo", track.solo),
        );
        self.devices(w, track);
        mixer_params(
            w,
            &naming::mute(track.id),
            track.mute,
            &naming::pan(track.id),
            track.pan,
        );
        if !track.sends.is_empty() {
            w.open(El::new("Sends"));
            for send in &track.sends {
                w.open(
                    El::new("Send")
                        .attr("id", naming::send(track.id, send.target))
                        .attr("destination", naming::channel(send.target))
                        .attr("type", if send.pre_fader { "pre" } else { "post" }),
                );
                w.leaf(
                    El::new("Volume")
                        .attr("id", naming::send_volume(track.id, send.target))
                        .attr("name", "Send")
                        .attr("unit", "linear")
                        .attr("min", 0)
                        .attr("max", db_to_linear(MAX_GAIN_DB))
                        .attr("value", db_to_linear(send.gain_db)),
                );
                w.close();
            }
            w.close();
        }
        volume_param(w, &naming::volume(track.id), track.gain_db);
        w.close();
        w.close();
    }

    /// Facts about a pool file, decoded once. `None` (reported) when the
    /// file cannot be read.
    pub(crate) fn audio_info(&mut self, track: &Track, source: &AudioRef) -> Option<AudioInfo> {
        if let Some(known) = self.audio.get(source) {
            return *known;
        }
        let info = self.load_audio(track, source);
        self.audio.insert(source.clone(), info);
        info
    }

    fn load_audio(&mut self, track: &Track, source: &AudioRef) -> Option<AudioInfo> {
        let at = location(track, &source.file_name());
        let Some(dir) = self.dir else {
            self.report.missing(
                Subject::Audio,
                at,
                "no project folder was given, so the audio file cannot be embedded; its clips are left out",
            );
            return None;
        };
        let path: PathBuf = match fourfloor_model::AudioPool::new(dir).path_for(source) {
            Ok(path) => path,
            Err(error) => {
                self.report.missing(Subject::Audio, at, error.to_string());
                return None;
            }
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                let message =
                    format!("cannot read the pool file ({error}); its clips are left out");
                self.report.missing(Subject::Audio, at, message);
                return None;
            }
        };
        let decoded = match fourfloor_audio::decode_bytes(bytes.clone(), Some(&source.extension)) {
            Ok(decoded) => decoded,
            Err(error) => {
                let message =
                    format!("cannot decode the pool file ({error}); its clips are left out");
                self.report.missing(Subject::Audio, at, message);
                return None;
            }
        };
        let info = AudioInfo {
            channels: decoded.channel_count(),
            sample_rate: decoded.sample_rate.hz(),
            seconds: decoded.duration_seconds(),
        };
        // Inserting a valid name into the container cannot fail.
        let _ = self.files.insert(&audio_path(source), bytes);
        Some(info)
    }
}

/// Where a pool file is stored inside the archive.
pub(crate) fn audio_path(source: &AudioRef) -> String {
    format!("audio/{}", source.file_name())
}

fn mixer_params(w: &mut XmlWriter, mute_id: &str, mute: bool, pan_id: &str, pan: f32) {
    w.leaf(
        El::new("Mute")
            .attr("id", mute_id)
            .attr("name", "Mute")
            .attr("value", mute),
    );
    w.leaf(
        El::new("Pan")
            .attr("id", pan_id)
            .attr("name", "Pan")
            .attr("unit", "normalized")
            .attr("min", 0)
            .attr("max", 1)
            .attr("value", pan_to_normalized(pan)),
    );
}

fn volume_param(w: &mut XmlWriter, id: &str, gain_db: f32) {
    w.leaf(
        El::new("Volume")
            .attr("id", id)
            .attr("name", "Volume")
            .attr("unit", "linear")
            .attr("min", 0)
            .attr("max", db_to_linear(MAX_GAIN_DB))
            .attr("value", db_to_linear(gain_db)),
    );
}
