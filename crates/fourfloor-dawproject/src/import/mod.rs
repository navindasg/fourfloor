//! Import: reads a DAWproject archive into a fourfloor project.
//!
//! Tracks are flattened (a group becomes a bus its children feed), audio
//! files and plugin state are stored in the project folder's pools, and
//! everything fourfloor cannot hold exactly is listed in the report.

mod automation;
mod clips;
mod content;
mod devices;
mod structure;

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use fourfloor_core::{DeviceId, ParamId, Tempo};
use fourfloor_model::{AudioPool, AudioRef, PluginStateStore, Project, Scene, Track};
use roxmltree::Document;

use crate::container::{Container, EXTRAS_JSON, METADATA_XML, PROJECT_XML};
use crate::error::{io_error, DawprojectError};
use crate::export::APPLICATION_NAME;
use crate::extras::Extras;
use crate::metadata::Metadata;
use crate::options::{ImportOptions, Imported};
use crate::report::{Report, Subject};
use crate::units::TimeUnit;
use crate::xml::{child, elements, number, tag, Node};

/// Reads a `.dawproject` file held in memory.
///
/// Audio files and plugin state are stored in the pools of `project_dir`
/// (created if needed), where the returned project refers to them. The
/// project itself is not saved; use [`fourfloor_model::save_project`].
pub fn import_project(
    bytes: &[u8],
    project_dir: &Path,
    options: &ImportOptions,
) -> Result<Imported, DawprojectError> {
    let files = Container::read(bytes, options.max_entry_bytes)?;
    import_container(&files, None, project_dir, options)
}

/// Reads a `.dawproject` file from disk. Files the project references as
/// external are looked up relative to the file's folder.
pub fn import_file(
    path: &Path,
    project_dir: &Path,
    options: &ImportOptions,
) -> Result<Imported, DawprojectError> {
    let bytes = std::fs::read(path).map_err(io_error("read", path))?;
    let files = Container::read(&bytes, options.max_entry_bytes)?;
    import_container(&files, path.parent(), project_dir, options)
}

fn text_entry<'a>(files: &'a Container, name: &str) -> Result<Option<&'a str>, DawprojectError> {
    files
        .get(name)
        .map(|bytes| {
            std::str::from_utf8(bytes)
                .map_err(|_| DawprojectError::Xml(format!("{name} is not UTF-8")))
        })
        .transpose()
}

fn import_container(
    files: &Container,
    source_dir: Option<&Path>,
    project_dir: &Path,
    options: &ImportOptions,
) -> Result<Imported, DawprojectError> {
    let xml = text_entry(files, PROJECT_XML)?
        .ok_or_else(|| DawprojectError::MissingEntry(PROJECT_XML.to_owned()))?;
    let parse_options = roxmltree::ParsingOptions {
        allow_dtd: false,
        ..roxmltree::ParsingOptions::default()
    };
    let doc = Document::parse_with_options(xml, parse_options)
        .map_err(|error| DawprojectError::Xml(error.to_string()))?;
    let root = doc.root_element();
    if tag(root) != "Project" {
        return Err(DawprojectError::NotDawproject(format!(
            "the root element is <{}>, not <Project>",
            tag(root)
        )));
    }
    let mut report = Report::default();
    let metadata = match text_entry(files, METADATA_XML)? {
        Some(text) => Metadata::from_xml(text).unwrap_or_else(|error| {
            report.dropped(Subject::Project, METADATA_XML, error.to_string());
            Metadata::default()
        }),
        None => Metadata::default(),
    };
    let ours =
        child(root, "Application").and_then(|a| a.attribute("name")) == Some(APPLICATION_NAME);
    let extras = if options.extras && ours {
        read_extras(files, &mut report)
    } else {
        None
    };
    let mut importer = Importer::new(
        &doc,
        files,
        source_dir,
        project_dir,
        extras.as_ref(),
        report,
    );
    let base = importer.project(metadata.title.as_deref())?;
    let Importer { mut report, .. } = importer;
    let base = break_routing_loops(base, &mut report);
    let project = match extras {
        Some(extras) => apply_extras(base, &extras, &mut report)?,
        None => {
            base.validate()?;
            base
        }
    };
    Ok(Imported {
        project,
        metadata,
        report,
    })
}

/// Removes routing that loops (a file may route a bus back into itself):
/// first the offending track's sends, then its output.
fn break_routing_loops(project: Project, report: &mut Report) -> Project {
    let mut project = project;
    for _ in 0..=project.tracks.len() * 2 {
        let Err(fourfloor_model::ModelError::InvalidRouting { track, reason }) = project.validate()
        else {
            break;
        };
        let Some(index) = project.track_index(track) else {
            break;
        };
        let current = &project.tracks[index];
        let fixed = if current.sends.is_empty() {
            Track {
                output: None,
                ..(**current).clone()
            }
        } else {
            Track {
                sends: Vec::new(),
                ..(**current).clone()
            }
        };
        let what = if current.sends.is_empty() {
            "output is routed to the master"
        } else {
            "sends are removed"
        };
        report.dropped(
            Subject::Channel,
            current.name.clone(),
            format!("the routing would loop ({reason}); the track's {what}"),
        );
        let tracks = project
            .tracks
            .iter()
            .enumerate()
            .map(|(i, t)| {
                if i == index {
                    Arc::new(fixed.clone())
                } else {
                    Arc::clone(t)
                }
            })
            .collect();
        project = Project { tracks, ..project };
    }
    project
}

fn read_extras(files: &Container, report: &mut Report) -> Option<Extras> {
    let bytes = files.get(EXTRAS_JSON)?;
    match serde_json::from_slice::<Extras>(bytes) {
        Ok(extras) if extras.format <= crate::extras::EXTRAS_FORMAT => Some(extras),
        Ok(extras) => {
            report.dropped(
                Subject::Project,
                EXTRAS_JSON,
                format!(
                    "the extension file has format {}, newer than this build reads; it is ignored",
                    extras.format
                ),
            );
            None
        }
        Err(error) => {
            report.dropped(
                Subject::Project,
                EXTRAS_JSON,
                format!("the extension file cannot be read ({error}); it is ignored"),
            );
            None
        }
    }
}

fn apply_extras(
    base: Project,
    extras: &Extras,
    report: &mut Report,
) -> Result<Project, DawprojectError> {
    let merged = extras.apply(&base);
    match merged.validate() {
        Ok(()) => Ok(merged),
        Err(error) => {
            report.dropped(
                Subject::Project,
                EXTRAS_JSON,
                format!("the extension file does not fit the project ({error}); it is ignored"),
            );
            base.validate()?;
            Ok(base)
        }
    }
}

/// Hands out model ids. With an extension file present the ids written
/// into the XML by fourfloor are reused, so the extension file's references
/// stay valid; otherwise ids are numbered from 1.
pub(crate) struct IdGen {
    preserve: bool,
    used: HashSet<u64>,
    next: u64,
    floor: u64,
}

impl IdGen {
    fn new(preserve_from: Option<u64>) -> Self {
        Self {
            preserve: preserve_from.is_some(),
            used: HashSet::new(),
            next: preserve_from.unwrap_or(1).max(1),
            floor: preserve_from.unwrap_or(1),
        }
    }

    pub(crate) fn claim(&mut self, hint: Option<u64>) -> u64 {
        if let Some(hint) = hint.filter(|h| self.preserve && *h > 0 && *h < self.floor) {
            if self.used.insert(hint) {
                return hint;
            }
        }
        while !self.used.insert(self.next) {
            self.next += 1;
        }
        self.next
    }

    fn next_id(&self) -> u64 {
        let top = self.used.iter().max().map_or(1, |max| max + 1);
        top.max(self.floor)
    }
}

/// What an automation lane's `Target parameter` refers to.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Target {
    Volume(usize),
    Pan(usize),
    Device {
        track: usize,
        device: DeviceId,
        param: ParamId,
        /// The range and unit the XML parameter declared, for reading point
        /// values written in its units.
        xml_min: f64,
        xml_max: f64,
        normalized: bool,
    },
    Unsupported(&'static str),
}

pub(crate) struct Importer<'a, 'input> {
    pub doc: &'a Document<'input>,
    pub files: &'a Container,
    pub source_dir: Option<&'a Path>,
    pub pool: AudioPool,
    pub states: PluginStateStore,
    pub tempo: Tempo,
    pub report: Report,
    pub ids: IdGen,
    pub by_id: HashMap<&'a str, Node<'a, 'input>>,
    pub targets: HashMap<&'a str, Target>,
    pub audio_files: HashMap<String, Option<AudioRef>>,
    pub tracks: Vec<Track>,
    pub scenes: Vec<Scene>,
}

impl<'a, 'input> Importer<'a, 'input> {
    fn new(
        doc: &'a Document<'input>,
        files: &'a Container,
        source_dir: Option<&'a Path>,
        project_dir: &Path,
        extras: Option<&Extras>,
        report: Report,
    ) -> Self {
        let by_id = doc
            .descendants()
            .filter(Node::is_element)
            .filter_map(|node| node.attribute("id").map(|id| (id, node)))
            .collect();
        Self {
            doc,
            files,
            source_dir,
            pool: AudioPool::new(project_dir),
            states: PluginStateStore::new(project_dir),
            tempo: Tempo::DEFAULT,
            report,
            ids: IdGen::new(extras.map(|extras| extras.next_id)),
            by_id,
            targets: HashMap::new(),
            audio_files: HashMap::new(),
            tracks: Vec::new(),
            scenes: Vec::new(),
        }
    }

    fn project(&mut self, title: Option<&str>) -> Result<Project, DawprojectError> {
        let root = self.doc.root_element();
        let name = title
            .map(str::to_owned)
            .unwrap_or_else(|| "Imported".to_owned());
        self.transport(root);
        let plan = self.structure(root);
        self.arrangement(root, &plan);
        self.scenes_from(root, &plan);
        let tracks = std::mem::take(&mut self.tracks)
            .into_iter()
            .map(Arc::new)
            .collect();
        Ok(Project {
            tracks,
            scenes: std::mem::take(&mut self.scenes),
            next_id: self.ids.next_id(),
            tempo: self.tempo,
            ..Project::new(sanitize_name(&name, "Imported"))
        })
    }

    fn transport(&mut self, root: Node<'a, 'input>) {
        let transport = child(root, "Transport");
        let tempo = transport.and_then(|t| child(t, "Tempo"));
        match tempo.and_then(|t| number(t, "value")) {
            Some(bpm) => match Tempo::new(bpm) {
                Ok(tempo) => self.tempo = tempo,
                Err(_) => self.report.approximated(
                    Subject::Transport,
                    "Transport/Tempo",
                    format!("tempo {bpm} BPM is out of range; 120 BPM is used"),
                ),
            },
            None => self.report.approximated(
                Subject::Transport,
                "Transport/Tempo",
                "the file gives no tempo; 120 BPM is used",
            ),
        }
        let signature = transport.and_then(|t| child(t, "TimeSignature"));
        if let Some(signature) = signature {
            let numerator = crate::xml::integer(signature, "numerator");
            let denominator = crate::xml::integer(signature, "denominator");
            if (numerator, denominator) != (Some(4), Some(4)) {
                self.report.approximated(
                    Subject::Transport,
                    "Transport/TimeSignature",
                    format!(
                        "fourfloor has no time signature; {}/{} is played as 4/4 (notes keep their beat positions)",
                        numerator.unwrap_or(4),
                        denominator.unwrap_or(4)
                    ),
                );
            }
        }
        if let Some(arrangement) = child(root, "Arrangement") {
            self.tempo_automation(arrangement);
        }
    }

    fn tempo_automation(&mut self, arrangement: Node<'a, 'input>) {
        if let Some(points) = child(arrangement, "TempoAutomation") {
            let values: Vec<f64> = elements(points)
                .filter_map(|point| number(point, "value"))
                .collect();
            if values.windows(2).any(|pair| pair[0] != pair[1]) {
                self.report.approximated(
                    Subject::Transport,
                    "Arrangement/TempoAutomation",
                    format!(
                        "fourfloor has one tempo; the tempo changes are dropped and {} BPM is used throughout",
                        self.tempo.bpm()
                    ),
                );
            }
        }
        if let Some(points) = child(arrangement, "TimeSignatureAutomation") {
            if elements(points).any(|point| tag(point) != "Target") {
                self.report.dropped(
                    Subject::Transport,
                    "Arrangement/TimeSignatureAutomation",
                    "time signature changes are not supported",
                );
            }
        }
    }

    /// The time unit in effect at `node`: its own `timeUnit`, or the
    /// nearest ancestor's, or beats.
    pub(crate) fn time_unit(&self, node: Node<'a, 'input>) -> TimeUnit {
        node.ancestors()
            .filter(Node::is_element)
            .find_map(|n| TimeUnit::parse(n.attribute("timeUnit")))
            .unwrap_or(TimeUnit::Beats)
    }

    /// The element an IDREF names.
    pub(crate) fn lookup(&self, id: Option<&str>) -> Option<Node<'a, 'input>> {
        id.and_then(|id| self.by_id.get(id).copied())
    }
}

/// A name the model accepts: control characters removed, trimmed, at most
/// [`fourfloor_model::validate::MAX_NAME_LEN`] characters, `fallback` when
/// nothing is left.
pub(crate) fn sanitize_name(raw: &str, fallback: &str) -> String {
    let cleaned: String = raw
        .chars()
        .filter(|c| !c.is_control())
        .take(fourfloor_model::validate::MAX_NAME_LEN)
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}
