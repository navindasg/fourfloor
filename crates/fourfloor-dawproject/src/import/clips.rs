//! Import of the arrangement and the clip launcher: windows onto notes and
//! audio become fourfloor clips.

use std::sync::Arc;

use fourfloor_core::{ClipId, Tempo, Ticks};
use fourfloor_model::validate::MAX_NOTES_PER_CLIP;
use fourfloor_model::{
    AudioRef, Clip, ClipContent, MidiNote, Scene, SceneId, SessionSlot, TrackKind, Warp,
};

use super::content::{Resolved, View};
use super::structure::Plan;
use super::{sanitize_name, Importer};
use crate::naming;
use crate::report::Subject;
use crate::units::{beats_to_ticks, unit_to_velocity, TimeUnit};
use crate::xml::{child, elements, integer, number, path, tag, Node};

/// Default note velocity when a note gives none: MIDI 100.
const DEFAULT_VELOCITY: f64 = 100.0 / 127.0;
/// Algorithms that change pitch with speed.
const REPITCH: [&str; 5] = ["repitch", "resample", "varispeed", "raw", "none"];

fn inside_clip(node: Node<'_, '_>) -> bool {
    node.ancestors()
        .skip(1)
        .any(|a| a.is_element() && tag(a) == "Clip")
}

/// Splits a window whose play start lies before its loop into the lead-in
/// (played once up to the loop end) and the looping rest, wraps a play
/// start past the loop end, and trims anything before time zero.
fn normalize<'a, 'input>(view: View<'a, 'input>) -> Vec<View<'a, 'input>> {
    let mut views = Vec::new();
    let mut rest = view;
    if let Some((start, end)) = rest.looped {
        if rest.play_start < start {
            let lead = (end - rest.play_start).min(rest.duration);
            views.push(View {
                looped: None,
                duration: lead,
                fade_out: if lead >= rest.duration {
                    rest.fade_out
                } else {
                    0.0
                },
                ..rest
            });
            rest = View {
                time: rest.time + lead,
                duration: rest.duration - lead,
                play_start: start,
                fade_in: 0.0,
                ..rest
            };
        } else if rest.play_start >= end {
            rest.play_start = rest.advance(0.0);
        }
    }
    views.push(rest);
    views
        .into_iter()
        .filter_map(|mut v| {
            if v.time < 0.0 {
                let cut = -v.time;
                v.play_start = v.advance(cut);
                v.duration -= cut;
                v.time = 0.0;
                v.fade_in = 0.0;
            }
            (v.duration * fourfloor_core::TICKS_PER_BEAT as f64 >= 0.5).then_some(v)
        })
        .collect()
}

fn snap_bpm(bpm: f64) -> f64 {
    let snapped = (bpm * 1e6).round() / 1e6;
    if (snapped - bpm).abs() <= bpm * 1e-9 {
        snapped
    } else {
        bpm
    }
}

impl<'a, 'input> Importer<'a, 'input> {
    pub(super) fn arrangement(&mut self, root: Node<'a, 'input>, plan: &Plan<'a, 'input>) {
        let Some(arrangement) = child(root, "Arrangement") else {
            return;
        };
        let mut reported_markers = false;
        for node in arrangement.descendants().filter(Node::is_element) {
            if inside_clip(node) {
                continue;
            }
            match tag(node) {
                "Clips" => {
                    for clip in elements(node).filter(|n| tag(*n) == "Clip") {
                        self.arranged_clip(clip, plan);
                    }
                }
                "Points" => self.automation_lane(node),
                "Notes" | "Audio" | "Warps"
                    if !matches!(node.parent_element().map(tag), Some("Warps")) =>
                {
                    self.bare_content(node, plan);
                }
                "Markers" | "markers" if !reported_markers => {
                    reported_markers = true;
                    self.report.dropped(
                        Subject::Marker,
                        path(node),
                        "arrangement markers are not supported",
                    );
                }
                "ClipSlot" => self.report.dropped(
                    Subject::Scene,
                    path(node),
                    "a launcher slot outside a scene is left out",
                ),
                _ => {}
            }
        }
    }

    fn arranged_clip(&mut self, clip: Node<'a, 'input>, plan: &Plan<'a, 'input>) {
        let at = path(clip);
        let Some(track) = plan.track_of(clip) else {
            self.report
                .dropped(Subject::Clip, at, "the clip is on no known track");
            return;
        };
        let parent = self.time_unit(clip);
        let Some(view) = self.view(clip, parent, &at) else {
            return;
        };
        for clip in self.clips_from(view, track, &at) {
            self.tracks[track].clips.push(Arc::new(clip));
        }
    }

    /// Notes or audio placed straight on a track lane, with no clip: one
    /// clip from time zero to the end of the content.
    fn bare_content(&mut self, node: Node<'a, 'input>, plan: &Plan<'a, 'input>) {
        let at = path(node);
        let Some(track) = plan.track_of(node) else {
            return;
        };
        let unit = self.time_unit(node);
        let Some(end) = self.extent(node, unit).filter(|end| *end > 0.0) else {
            return;
        };
        let view = View {
            name: node.attribute("name"),
            time: 0.0,
            duration: end,
            play_start: 0.0,
            looped: None,
            fade_in: 0.0,
            fade_out: 0.0,
            unit,
            content: node,
        };
        for clip in self.clips_from(view, track, &at) {
            self.tracks[track].clips.push(Arc::new(clip));
        }
    }

    /// The fourfloor clips a window becomes on track `track`.
    pub(crate) fn clips_from(
        &mut self,
        view: View<'a, 'input>,
        track: usize,
        at: &str,
    ) -> Vec<Clip> {
        let mut resolved = Vec::new();
        self.resolve(view, 0, at, &mut resolved);
        let kind = self.tracks[track].kind;
        let mut clips = Vec::new();
        for item in resolved {
            let (view, wanted) = match item {
                Resolved::Notes(view) => (view, TrackKind::Midi),
                Resolved::Audio(view, ..) => (view, TrackKind::Audio),
            };
            if kind != wanted {
                self.report.dropped(
                    Subject::Clip,
                    at.to_owned(),
                    format!(
                        "{} content on a {kind} track is left out",
                        if wanted == TrackKind::Midi {
                            "note"
                        } else {
                            "audio"
                        }
                    ),
                );
                continue;
            }
            for piece in normalize(view) {
                let clip = match item {
                    Resolved::Notes(_) => self.midi_clip(piece, at),
                    Resolved::Audio(_, audio, warps) => self.audio_clip(piece, audio, warps, at),
                };
                clips.extend(clip);
            }
        }
        clips
    }

    fn midi_clip(&mut self, view: View<'a, 'input>, at: &str) -> Option<Clip> {
        let notes_node = view.content;
        let unit = TimeUnit::parse(notes_node.attribute("timeUnit")).unwrap_or(view.unit);
        let (shift, offset, loop_length) = match view.looped {
            Some((start, end)) => (start, view.play_start - start, Some(end - start)),
            None if view.play_start < 0.0 => (view.play_start, 0.0, None),
            None => (0.0, view.play_start, None),
        };
        let mut notes = Vec::new();
        let (mut outside, mut channels, mut expressions) = (0usize, false, false);
        for note in elements(notes_node).filter(|n| tag(*n) == "Note") {
            let (Some(time), Some(duration), Some(key)) = (
                number(note, "time"),
                number(note, "duration"),
                integer(note, "key"),
            ) else {
                outside += 1;
                continue;
            };
            channels |= integer(note, "channel").is_some_and(|c| c != 0);
            expressions |= elements(note).next().is_some();
            let start = beats_to_ticks(self.beats(time, unit) - shift);
            if start < Ticks::ZERO || !(0..=127).contains(&key) {
                outside += 1;
                continue;
            }
            notes.push(MidiNote {
                start,
                length: beats_to_ticks(self.beats(duration, unit)).max(Ticks::new(1)),
                pitch: key as u8,
                velocity: unit_to_velocity(number(note, "vel").unwrap_or(DEFAULT_VELOCITY)),
            });
        }
        if outside > 0 {
            self.report.dropped(
                Subject::Note,
                at.to_owned(),
                format!(
                    "{outside} notes before the clip content or with no valid key are left out"
                ),
            );
        }
        if channels {
            self.report.approximated(
                Subject::Note,
                at.to_owned(),
                "notes on several MIDI channels are merged onto one",
            );
        }
        if expressions {
            self.report.dropped(
                Subject::Note,
                at.to_owned(),
                "per-note expressions are not supported",
            );
        }
        if notes.len() > MAX_NOTES_PER_CLIP {
            self.report.dropped(
                Subject::Note,
                at.to_owned(),
                format!("only the first {MAX_NOTES_PER_CLIP} notes are kept"),
            );
            notes.truncate(MAX_NOTES_PER_CLIP);
        }
        notes.sort_by_key(|note| (note.start, note.pitch));
        let hint = naming::clip_number(notes_node.attribute("id"));
        Some(self.make_clip(
            &view,
            hint,
            offset,
            loop_length,
            ClipContent::Midi { notes },
        ))
    }

    fn make_clip(
        &mut self,
        view: &View<'a, 'input>,
        hint: Option<u64>,
        offset: f64,
        loop_length: Option<f64>,
        content: ClipContent,
    ) -> Clip {
        let name = sanitize_name(view.name.unwrap_or_default(), "Clip");
        let start = beats_to_ticks(view.time).max(Ticks::ZERO);
        let length = beats_to_ticks(view.duration).max(Ticks::new(1));
        let loop_length = loop_length.map(|l| beats_to_ticks(l).max(Ticks::new(1)));
        let offset = beats_to_ticks(offset).max(Ticks::ZERO);
        let offset = match loop_length {
            Some(period) => Ticks::new(offset.get().rem_euclid(period.get())),
            None => offset,
        };
        Clip {
            id: ClipId::new(self.ids.claim(hint)),
            name,
            start,
            length,
            offset,
            loop_length,
            content,
        }
    }

    fn audio_clip(
        &mut self,
        mut view: View<'a, 'input>,
        audio: Node<'a, 'input>,
        warps: Option<Node<'a, 'input>>,
        at: &str,
    ) -> Option<Clip> {
        let source = self.audio_file(audio, at)?;
        let rate = integer(audio, "sampleRate").filter(|r| *r > 0).map_or_else(
            || {
                self.report.approximated(
                    Subject::Audio,
                    at.to_owned(),
                    "the audio has no sample rate; 48 kHz is assumed",
                );
                48_000.0
            },
            |r| r as f64,
        );
        let (seconds_per_beat, origin, warp) = self.warp_map(warps, audio, at);
        let position = |beats: f64| origin + seconds_per_beat * beats;
        let (mut source_seconds, offset, loop_length) = match view.looped {
            Some((start, end)) => (position(start), view.play_start - start, Some(end - start)),
            None => (position(view.play_start), 0.0, None),
        };
        if source_seconds < 0.0 {
            match loop_length {
                None => {
                    let lead = -source_seconds / seconds_per_beat;
                    view.time += lead;
                    view.duration -= lead;
                    view.fade_in = (view.fade_in - lead).max(0.0);
                    if view.duration <= 0.0 {
                        return None;
                    }
                }
                Some(_) => self.report.approximated(
                    Subject::Clip,
                    at.to_owned(),
                    "the loop starts before the audio file; it is moved to the file start",
                ),
            }
            source_seconds = 0.0;
        }
        let length = beats_to_ticks(view.duration).max(Ticks::new(1));
        let (mut fade_in, mut fade_out) =
            (beats_to_ticks(view.fade_in), beats_to_ticks(view.fade_out));
        if fade_in.get() + fade_out.get() > length.get() {
            self.report.approximated(
                Subject::Clip,
                at.to_owned(),
                "fades longer than the clip are shortened",
            );
            let total = (fade_in.get() + fade_out.get()) as f64;
            fade_in =
                Ticks::new((fade_in.get() as f64 * length.get() as f64 / total).floor() as i64);
            fade_out = Ticks::new((length.get() - fade_in.get()).min(fade_out.get()));
        }
        let content = ClipContent::Audio {
            source,
            source_offset_frames: (source_seconds * rate).round() as u64,
            gain_db: 0.0,
            fade_in,
            fade_out,
            warp,
        };
        let hint = warps
            .and_then(|w| naming::clip_number(w.attribute("id")))
            .or_else(|| naming::clip_number(audio.attribute("id")));
        Some(self.make_clip(&view, hint, offset, loop_length, content))
    }

    /// Seconds of the file per beat of content, the file position at
    /// content zero, and the model warp.
    fn warp_map(
        &mut self,
        warps: Option<Node<'a, 'input>>,
        audio: Node<'a, 'input>,
        at: &str,
    ) -> (f64, f64, Option<Warp>) {
        let plain = (60.0 / self.tempo.bpm(), 0.0, None);
        let Some(warps) = warps else {
            return plain;
        };
        let time_unit =
            TimeUnit::parse(warps.attribute("timeUnit")).unwrap_or(self.time_unit(warps));
        let content_unit =
            TimeUnit::parse(warps.attribute("contentTimeUnit")).unwrap_or(TimeUnit::Seconds);
        let points: Vec<(f64, f64)> = elements(warps)
            .filter(|n| tag(*n) == "Warp")
            .filter_map(|n| Some((number(n, "time")?, number(n, "contentTime")?)))
            .map(|(t, c)| {
                let seconds =
                    crate::units::convert_time(c, content_unit, TimeUnit::Seconds, self.tempo);
                (self.beats(t, time_unit), seconds)
            })
            .collect();
        let (Some(first), Some(last)) = (points.first(), points.last()) else {
            return plain;
        };
        let slope = (last.1 - first.1) / (last.0 - first.0);
        if points.len() < 2 || !slope.is_finite() || slope <= 0.0 {
            self.report.approximated(
                Subject::Warp,
                at.to_owned(),
                "the warp markers do not give a tempo; the audio plays unwarped",
            );
            return plain;
        }
        let origin = first.1 - slope * first.0;
        let straight = points
            .iter()
            .all(|(t, c)| (origin + slope * t - c).abs() < 1e-6);
        if !straight {
            self.report.approximated(
                Subject::Warp,
                at.to_owned(),
                "fourfloor warps at one constant tempo; the warp markers are replaced by the average tempo between the first and last",
            );
        }
        let bpm = snap_bpm(60.0 / slope);
        if Tempo::new(bpm).is_err() {
            self.report.approximated(
                Subject::Warp,
                at.to_owned(),
                format!("a warp tempo of {bpm:.2} BPM is out of range; the audio plays unwarped"),
            );
            return plain;
        }
        let algorithm = audio.attribute("algorithm").map(str::to_ascii_lowercase);
        let key_lock = !algorithm.is_some_and(|a| REPITCH.contains(&a.as_str()));
        (
            60.0 / bpm,
            origin,
            Some(Warp {
                source_bpm: bpm,
                key_lock,
            }),
        )
    }

    /// Stores the file an `Audio` element points at in the audio pool.
    fn audio_file(&mut self, audio: Node<'a, 'input>, at: &str) -> Option<AudioRef> {
        let Some(file) = child(audio, "File") else {
            self.report.missing(
                Subject::Audio,
                at.to_owned(),
                "the audio has no file; the clip is left out",
            );
            return None;
        };
        let key = format!(
            "{}|{}",
            file.attribute("external").unwrap_or("false"),
            file.attribute("path").unwrap_or("")
        );
        if let Some(known) = self.audio_files.get(&key) {
            return known.clone();
        }
        let extension = std::path::Path::new(file.attribute("path").unwrap_or(""))
            .extension()
            .and_then(|e| e.to_str())
            .map(str::to_ascii_lowercase)
            .filter(|e| {
                fourfloor_model::validate::MAX_EXTENSION_LEN >= e.len()
                    && !e.is_empty()
                    && e.bytes().all(|b| b.is_ascii_alphanumeric())
            })
            .unwrap_or_else(|| "wav".to_owned());
        let stored = self.referenced_file(file).and_then(|bytes| {
            self.pool
                .import_bytes(&bytes, &extension)
                .map_err(|e| e.to_string())
        });
        let source = match stored {
            Ok(source) => Some(source),
            Err(reason) => {
                self.report.missing(
                    Subject::Audio,
                    at.to_owned(),
                    format!("{reason}; its clips are left out"),
                );
                None
            }
        };
        self.audio_files.insert(key, source.clone());
        source
    }

    pub(super) fn scenes_from(&mut self, root: Node<'a, 'input>, plan: &Plan<'a, 'input>) {
        let Some(list) = child(root, "Scenes") else {
            return;
        };
        for (index, node) in elements(list).filter(|n| tag(*n) == "Scene").enumerate() {
            let id = SceneId::new(
                self.ids
                    .claim(naming::number(node.attribute("id"), "scene")),
            );
            let name = sanitize_name(
                node.attribute("name").unwrap_or_default(),
                &format!("Scene {}", index + 1),
            );
            self.scenes.push(Scene { id, name });
            for slot in node
                .descendants()
                .filter(|n| n.is_element() && tag(*n) == "ClipSlot")
            {
                self.slot(slot, id, plan);
            }
        }
    }

    fn slot(&mut self, slot: Node<'a, 'input>, scene: SceneId, plan: &Plan<'a, 'input>) {
        let at = path(slot);
        let Some(clip) = elements(slot).find(|n| tag(*n) == "Clip") else {
            return;
        };
        let Some(track) = plan.track_of(slot) else {
            self.report
                .dropped(Subject::Scene, at, "the slot is on no known track");
            return;
        };
        let parent = self.time_unit(slot);
        let Some(view) = self.view(clip, parent, &at) else {
            return;
        };
        let view = View { time: 0.0, ..view };
        let mut clips = self.clips_from(view, track, &at).into_iter();
        let Some(first) = clips.next() else {
            return;
        };
        if clips.next().is_some() {
            self.report.approximated(
                Subject::Scene,
                at.clone(),
                "a launcher clip made of several pieces keeps only the first",
            );
        }
        if self.tracks[track].slot(scene).is_some() {
            self.report.dropped(
                Subject::Scene,
                at,
                "a second clip for the same track and scene",
            );
            return;
        }
        self.tracks[track].slots.push(SessionSlot {
            scene,
            clip: Arc::new(Clip {
                start: Ticks::ZERO,
                ..first
            }),
        });
    }
}
