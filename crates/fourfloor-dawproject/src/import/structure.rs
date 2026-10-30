//! Import of the track structure and the mixer: tracks, channels, routing,
//! volume, pan, mute, solo and sends.

use std::collections::{HashMap, HashSet};

use fourfloor_core::TrackId;
use fourfloor_model::{Send, Track, TrackKind};

use super::{sanitize_name, Importer, Target};
use crate::naming;
use crate::report::Subject;
use crate::units::{clamp_db, linear_to_db, normalized_to_pan, MAX_GAIN_DB, MIN_GAIN_DB};
use crate::xml::{boolean, child, elements, number, path, tag, Node};

/// One fourfloor track to be made from the document.
pub(crate) struct TrackPlan<'a, 'input> {
    pub node: Option<Node<'a, 'input>>,
    pub channel: Option<Node<'a, 'input>>,
    pub name: String,
    pub parent: Option<usize>,
    pub kind: TrackKind,
    pub id: TrackId,
}

/// The tracks to be made, and how XML ids map onto them.
pub(crate) struct Plan<'a, 'input> {
    pub tracks: Vec<TrackPlan<'a, 'input>>,
    /// `Track` element id to plan index.
    pub by_track: HashMap<&'a str, usize>,
    /// `Channel` element id to plan index.
    pub by_channel: HashMap<&'a str, usize>,
    pub master_channel: Option<&'a str>,
    /// The bus standing in for a master channel with devices or a level.
    pub master_bus: Option<usize>,
}

impl<'a> Plan<'a, '_> {
    /// The plan index of the track a timeline belongs to: its own or its
    /// nearest ancestor's `track` attribute.
    pub(crate) fn track_of(&self, node: Node<'a, '_>) -> Option<usize> {
        node.ancestors()
            .filter(Node::is_element)
            .find_map(|n| n.attribute("track"))
            .and_then(|id| self.by_track.get(id).copied())
    }
}

/// Content found in the timelines of one track.
#[derive(Clone, Copy, Default)]
struct Found {
    notes: bool,
    audio: bool,
}

fn content_types(node: Option<Node<'_, '_>>) -> Vec<String> {
    node.and_then(|n| n.attribute("contentType"))
        .map(|text| text.split_whitespace().map(str::to_owned).collect())
        .unwrap_or_default()
}

/// The value of a `RealParameter`, mapped from `normalized` through its
/// range when it declares one.
fn real_value(node: Node<'_, '_>) -> Option<f64> {
    let value = number(node, "value")?;
    if node.attribute("unit") == Some("normalized") {
        if let (Some(min), Some(max)) = (number(node, "min"), number(node, "max")) {
            if (min, max) != (0.0, 1.0) && max > min {
                return Some(min + value * (max - min));
            }
        }
    }
    Some(value)
}

/// A channel or send `Volume` in decibels.
pub(crate) fn volume_db(node: Node<'_, '_>) -> Option<f64> {
    let value = real_value(node)?;
    Some(match node.attribute("unit") {
        Some("decibel") => value,
        _ if value <= 0.0 => f64::NEG_INFINITY,
        _ => 20.0 * value.log10(),
    })
}

/// A channel `Pan` as -1..1.
pub(crate) fn pan_value(node: Node<'_, '_>) -> Option<f32> {
    let value = number(node, "value")?;
    let (min, max) = match node.attribute("unit") {
        Some("normalized") => (0.0, 1.0),
        Some("percent") => (-100.0, 100.0),
        _ => (-1.0, 1.0),
    };
    let min = number(node, "min").unwrap_or(min);
    let max = number(node, "max").unwrap_or(max);
    if max <= min {
        return None;
    }
    Some(normalized_to_pan((value - min) / (max - min)))
}

impl<'a, 'input> Importer<'a, 'input> {
    pub(super) fn structure(&mut self, root: Node<'a, 'input>) -> Plan<'a, 'input> {
        let mut plan = Plan {
            tracks: Vec::new(),
            by_track: HashMap::new(),
            by_channel: HashMap::new(),
            master_channel: None,
            master_bus: None,
        };
        let mut master = None;
        if let Some(structure) = child(root, "Structure") {
            for node in elements(structure) {
                self.collect(node, None, &mut plan, &mut master);
            }
        }
        let found = self.scan_content(root);
        let destinations = destinations(root);
        for index in 0..plan.tracks.len() {
            let kind = self.decide_kind(&plan, index, &found, &destinations);
            plan.tracks[index].kind = kind;
        }
        if let Some(channel) = master {
            self.master(channel, &mut plan);
        }
        for track in &mut plan.tracks {
            let hint = naming::number(track.node.and_then(|n| n.attribute("id")), "t");
            track.id = TrackId::new(self.ids.claim(hint));
        }
        self.tracks = (0..plan.tracks.len())
            .map(|index| self.build_track(&plan, index))
            .collect();
        plan
    }

    fn collect(
        &mut self,
        node: Node<'a, 'input>,
        parent: Option<usize>,
        plan: &mut Plan<'a, 'input>,
        master: &mut Option<Node<'a, 'input>>,
    ) {
        let (track_node, channel) = match tag(node) {
            "Track" => (Some(node), child(node, "Channel")),
            "Channel" => (None, Some(node)),
            other => {
                self.report.dropped(
                    Subject::Other,
                    path(node),
                    format!("unknown structure element <{other}>"),
                );
                return;
            }
        };
        let is_master = channel.and_then(|c| c.attribute("role")) == Some("master");
        if is_master && master.is_none() {
            *master = channel;
            plan.master_channel = channel.and_then(|c| c.attribute("id"));
            if let Some(track_node) = track_node {
                for sub in elements(track_node).filter(|n| tag(*n) == "Track") {
                    self.collect(sub, parent, plan, master);
                }
            }
            return;
        }
        let raw_name = track_node
            .and_then(|n| n.attribute("name"))
            .or_else(|| channel.and_then(|c| c.attribute("name")))
            .unwrap_or_default();
        let index = plan.tracks.len();
        plan.tracks.push(TrackPlan {
            node: track_node,
            channel,
            name: sanitize_name(raw_name, &format!("Track {}", index + 1)),
            parent,
            kind: TrackKind::Audio,
            id: TrackId::new(0),
        });
        if let Some(id) = track_node.and_then(|n| n.attribute("id")) {
            plan.by_track.insert(id, index);
        }
        if let Some(id) = channel.and_then(|c| c.attribute("id")) {
            plan.by_channel.insert(id, index);
        }
        if let Some(track_node) = track_node {
            for sub in elements(track_node).filter(|n| tag(*n) == "Track") {
                self.collect(sub, Some(index), plan, master);
            }
        }
    }

    /// Which tracks have notes or audio anywhere in their timelines, by
    /// `Track` id.
    fn scan_content(&self, root: Node<'a, 'input>) -> HashMap<&'a str, Found> {
        let mut found: HashMap<&'a str, Found> = HashMap::new();
        for node in root.descendants().filter(Node::is_element) {
            let kind = match tag(node) {
                "Notes" => Found {
                    notes: true,
                    audio: false,
                },
                "Audio" => Found {
                    notes: false,
                    audio: true,
                },
                _ => continue,
            };
            let owner = node
                .ancestors()
                .filter(Node::is_element)
                .find_map(|n| n.attribute("track"));
            if let Some(owner) = owner {
                let entry = found.entry(owner).or_default();
                entry.notes |= kind.notes;
                entry.audio |= kind.audio;
            }
        }
        found
    }

    fn decide_kind(
        &self,
        plan: &Plan<'a, 'input>,
        index: usize,
        found: &HashMap<&'a str, Found>,
        destinations: &HashSet<&str>,
    ) -> TrackKind {
        let track = &plan.tracks[index];
        let role = track.channel.and_then(|c| c.attribute("role"));
        let types = content_types(track.node);
        let is_group = track
            .node
            .is_some_and(|n| elements(n).any(|c| tag(c) == "Track"))
            || types.iter().any(|t| t == "tracks");
        let content = track
            .node
            .and_then(|n| n.attribute("id"))
            .and_then(|id| found.get(id).copied())
            .unwrap_or_default();
        let has_instrument = track
            .channel
            .and_then(|c| child(c, "Devices"))
            .is_some_and(|d| {
                elements(d).any(|device| device.attribute("deviceRole") == Some("instrument"))
            });
        let is_target = track
            .channel
            .and_then(|c| c.attribute("id"))
            .is_some_and(|id| destinations.contains(id));
        if matches!(role, Some("effect" | "submix" | "vca")) || is_group || track.node.is_none() {
            TrackKind::Bus
        } else if content.notes {
            TrackKind::Midi
        } else if content.audio {
            TrackKind::Audio
        } else if types.iter().any(|t| t == "notes") || has_instrument {
            TrackKind::Midi
        } else if is_target {
            TrackKind::Bus
        } else {
            TrackKind::Audio
        }
    }

    fn master(&mut self, channel: Node<'a, 'input>, plan: &mut Plan<'a, 'input>) {
        let has_devices = child(channel, "Devices").is_some_and(|d| elements(d).next().is_some());
        let gain = child(channel, "Volume").and_then(volume_db).unwrap_or(0.0);
        let pan = child(channel, "Pan").and_then(pan_value).unwrap_or(0.0);
        let muted = child(channel, "Mute").and_then(|m| boolean(m, "value")) == Some(true);
        if !has_devices && gain.abs() < 1e-4 && pan.abs() < 1e-6 && !muted {
            return;
        }
        let raw = channel
            .parent_element()
            .and_then(|t| t.attribute("name"))
            .unwrap_or("Master");
        let name = sanitize_name(raw, "Master");
        self.report.approximated(
            Subject::Track,
            name.clone(),
            "fourfloor has no master channel; its devices and level are kept on a bus that every track feeds",
        );
        plan.master_bus = Some(plan.tracks.len());
        plan.tracks.push(TrackPlan {
            node: None,
            channel: Some(channel),
            name,
            parent: None,
            kind: TrackKind::Bus,
            id: TrackId::new(0),
        });
    }

    fn build_track(&mut self, plan: &Plan<'a, 'input>, index: usize) -> Track {
        let entry = &plan.tracks[index];
        let base = Track::new(entry.id, entry.name.clone(), entry.kind);
        let Some(channel) = entry.channel else {
            return base;
        };
        let name = entry.name.clone();
        let gain_db = match child(channel, "Volume") {
            Some(volume) => {
                if let Some(id) = volume.attribute("id") {
                    self.targets.insert(id, Target::Volume(index));
                }
                self.gain(volume, &name, "volume")
            }
            None => 0.0,
        };
        let pan = child(channel, "Pan").map_or(0.0, |node| {
            if let Some(id) = node.attribute("id") {
                self.targets.insert(id, Target::Pan(index));
            }
            pan_value(node).unwrap_or(0.0)
        });
        let mute = child(channel, "Mute").is_some_and(|node| {
            if let Some(id) = node.attribute("id") {
                self.targets.insert(id, Target::Unsupported("mute"));
            }
            boolean(node, "value") == Some(true)
        });
        let solo = boolean(channel, "solo") == Some(true);
        let output = if plan.master_bus == Some(index) {
            None
        } else {
            self.output(plan, index, channel)
        };
        let sends = self.sends(plan, index, channel);
        let devices = self.devices(index, &name, channel);
        Track {
            gain_db,
            pan,
            mute,
            solo,
            output,
            sends,
            devices,
            ..base
        }
    }

    /// A volume parameter in decibels within the fader range.
    pub(crate) fn gain(&mut self, node: Node<'a, 'input>, at: &str, what: &str) -> f32 {
        let (Some(value), Some(db)) = (real_value(node), volume_db(node)) else {
            return 0.0;
        };
        if db > f64::from(MAX_GAIN_DB) + 1e-6 {
            self.report.approximated(
                Subject::Channel,
                at.to_owned(),
                format!("the {what} of {db:.2} dB is above fourfloor's +{MAX_GAIN_DB} dB and is limited to it"),
            );
        }
        if node.attribute("unit") == Some("decibel") {
            clamp_db(db.max(f64::from(MIN_GAIN_DB)))
        } else {
            // The inverse of what export writes, so values return bit for bit.
            linear_to_db(value)
        }
    }

    fn output(
        &mut self,
        plan: &Plan<'a, 'input>,
        index: usize,
        channel: Node<'a, 'input>,
    ) -> Option<TrackId> {
        let master = plan.master_bus.map(|bus| plan.tracks[bus].id);
        let Some(destination) = channel.attribute("destination") else {
            let parent = plan.tracks[index].parent;
            return match parent {
                Some(parent) if plan.tracks[parent].kind == TrackKind::Bus => {
                    Some(plan.tracks[parent].id)
                }
                _ => master,
            };
        };
        if Some(destination) == plan.master_channel {
            return master;
        }
        let name = plan.tracks[index].name.clone();
        match plan.by_channel.get(destination).copied() {
            Some(target) if target == index => None,
            Some(target) if plan.tracks[target].kind == TrackKind::Bus => {
                Some(plan.tracks[target].id)
            }
            Some(target) => {
                self.report.approximated(
                    Subject::Channel,
                    name,
                    format!(
                        "routed to `{}`, which holds clips and so cannot be a bus in fourfloor; routed to the master instead",
                        plan.tracks[target].name
                    ),
                );
                master
            }
            None => {
                self.report.approximated(
                    Subject::Channel,
                    name,
                    format!(
                        "the output `{destination}` is not a channel; routed to the master instead"
                    ),
                );
                master
            }
        }
    }

    fn sends(
        &mut self,
        plan: &Plan<'a, 'input>,
        index: usize,
        channel: Node<'a, 'input>,
    ) -> Vec<Send> {
        let Some(list) = child(channel, "Sends") else {
            return Vec::new();
        };
        let name = plan.tracks[index].name.clone();
        let mut sends: Vec<Send> = Vec::new();
        for send in elements(list).filter(|n| tag(*n) == "Send") {
            if let Some(id) = child(send, "Volume").and_then(|v| v.attribute("id")) {
                self.targets.insert(id, Target::Unsupported("send level"));
            }
            let at = format!("{name} / send");
            let destination = send.attribute("destination");
            let target = destination.and_then(|d| plan.by_channel.get(d).copied());
            let Some(target) = target.filter(|t| *t != index) else {
                self.report.dropped(
                    Subject::Send,
                    at,
                    format!(
                        "the send target `{}` is not another channel",
                        destination.unwrap_or("")
                    ),
                );
                continue;
            };
            let target_plan = &plan.tracks[target];
            if target_plan.kind != TrackKind::Bus {
                let message = format!(
                    "`{}` holds clips, so it cannot receive a send in fourfloor",
                    target_plan.name
                );
                self.report.dropped(Subject::Send, at, message);
                continue;
            }
            let target_id = target_plan.id;
            if sends.iter().any(|s| s.target == target_id) {
                self.report
                    .dropped(Subject::Send, at, "a second send to the same bus");
                continue;
            }
            let mut gain_db =
                child(send, "Volume").map_or(MIN_GAIN_DB, |v| self.gain(v, &at, "send level"));
            if child(send, "Enable").and_then(|e| boolean(e, "value")) == Some(false) {
                self.report.approximated(
                    Subject::Send,
                    at.clone(),
                    "a disabled send is kept at silence",
                );
                gain_db = MIN_GAIN_DB;
            }
            if child(send, "Pan")
                .and_then(pan_value)
                .is_some_and(|p| p.abs() > 1e-6)
            {
                self.report.dropped(
                    Subject::Send,
                    at.clone(),
                    "send pan is not supported; the send is centred",
                );
            }
            sends.push(Send {
                target: target_id,
                gain_db,
                pre_fader: send.attribute("type") == Some("pre"),
            });
        }
        sends
    }
}

/// Every channel id something routes to.
fn destinations<'a>(root: Node<'a, '_>) -> HashSet<&'a str> {
    root.descendants()
        .filter(|n| n.is_element() && matches!(tag(*n), "Channel" | "Send"))
        .filter_map(|n| n.attribute("destination"))
        .collect()
}
