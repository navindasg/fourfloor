//! Clip windows and what they show.
//!
//! A DAWproject `Clip` is a window onto a content timeline, which may itself
//! hold clips (Bitwig wraps every audio event this way). Everything is
//! brought to beats at the project tempo, which is exact because fourfloor
//! has one tempo, and nested clips are flattened into plain windows onto
//! notes or audio. Loops of an outer clip are unrolled pass by pass, so
//! flattening keeps the sound.

use super::Importer;
use crate::report::Subject;
use crate::units::{convert_time, TimeUnit};
use crate::xml::{boolean, elements, number, tag, Node};

/// Nesting deeper than this is not followed.
const MAX_DEPTH: u32 = 8;
/// Loop passes unrolled when an outer looping clip holds several clips.
const MAX_PASSES: usize = 4_096;
/// Positions closer than this many beats are the same.
const EPSILON: f64 = 1e-9;

/// A clip window, in beats.
#[derive(Clone, Copy, Debug)]
pub(crate) struct View<'a, 'input> {
    pub name: Option<&'a str>,
    /// Start on the parent timeline.
    pub time: f64,
    /// Length on the parent timeline.
    pub duration: f64,
    /// Content position at the clip start.
    pub play_start: f64,
    /// `(start, end)` of the repeating region of the content.
    pub looped: Option<(f64, f64)>,
    pub fade_in: f64,
    pub fade_out: f64,
    /// The unit the content's own positions are written in.
    pub unit: TimeUnit,
    pub content: Node<'a, 'input>,
}

impl View<'_, '_> {
    /// The content position `delta` beats after the clip start.
    pub(crate) fn advance(&self, delta: f64) -> f64 {
        let position = self.play_start + delta;
        match self.looped {
            Some((start, end)) if position >= end && end - start > EPSILON => {
                start + (position - start).rem_euclid(end - start)
            }
            _ => position,
        }
    }
}

/// Notes or audio seen through a window.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Resolved<'a, 'input> {
    Notes(View<'a, 'input>),
    /// The `Audio` element and the `Warps` around it, if any.
    Audio(View<'a, 'input>, Node<'a, 'input>, Option<Node<'a, 'input>>),
}

impl<'a, 'input> Importer<'a, 'input> {
    pub(crate) fn beats(&self, value: f64, unit: TimeUnit) -> f64 {
        convert_time(value, unit, TimeUnit::Beats, self.tempo)
    }

    /// Reads a `Clip` element whose `time` and `duration` are in `parent`.
    pub(crate) fn view(
        &mut self,
        clip: Node<'a, 'input>,
        parent: TimeUnit,
        at: &str,
    ) -> Option<View<'a, 'input>> {
        if boolean(clip, "enable") == Some(false) {
            self.report
                .dropped(Subject::Clip, at.to_owned(), "a disabled clip is left out");
            return None;
        }
        let content = elements(clip)
            .next()
            .or_else(|| self.lookup(clip.attribute("reference")));
        let Some(content) = content else {
            self.report
                .dropped(Subject::Clip, at.to_owned(), "the clip has no content");
            return None;
        };
        let Some(time) = number(clip, "time") else {
            self.report
                .dropped(Subject::Clip, at.to_owned(), "the clip has no time");
            return None;
        };
        let unit = TimeUnit::parse(clip.attribute("contentTimeUnit"))
            .or_else(|| TimeUnit::parse(content.attribute("timeUnit")))
            .unwrap_or(parent);
        let play_start = self.beats(number(clip, "playStart").unwrap_or(0.0), unit);
        let duration = match number(clip, "duration") {
            Some(duration) => self.beats(duration, parent),
            None => match number(clip, "playStop") {
                Some(stop) => self.beats(stop, unit) - play_start,
                None => self.extent(content, unit).unwrap_or(4.0) - play_start,
            },
        };
        let looped = match (number(clip, "loopStart"), number(clip, "loopEnd")) {
            (start, Some(end)) => {
                let start = self.beats(start.unwrap_or(0.0), unit);
                let end = self.beats(end, unit);
                (end - start > EPSILON).then_some((start, end))
            }
            _ => None,
        };
        let fade_unit = TimeUnit::parse(clip.attribute("fadeTimeUnit")).unwrap_or(parent);
        let mut fade = |name: &str| {
            let raw = number(clip, name).unwrap_or(0.0);
            if raw < 0.0 {
                self.report.approximated(
                    Subject::Clip,
                    at.to_owned(),
                    "a crossfade (negative fade) is played as a plain fade",
                );
            }
            raw.abs()
        };
        let (fade_in, fade_out) = (fade("fadeInTime"), fade("fadeOutTime"));
        Some(View {
            name: clip.attribute("name"),
            time: self.beats(time, parent),
            duration,
            play_start,
            looped,
            fade_in: self.beats(fade_in, fade_unit),
            fade_out: self.beats(fade_out, fade_unit),
            unit,
            content,
        })
    }

    /// The end of a content timeline, in beats, when it can be told.
    pub(crate) fn extent(&self, content: Node<'a, 'input>, unit: TimeUnit) -> Option<f64> {
        let unit = TimeUnit::parse(content.attribute("timeUnit")).unwrap_or(unit);
        match tag(content) {
            "Notes" => elements(content)
                .filter_map(|n| Some(number(n, "time")? + number(n, "duration")?))
                .map(|end| self.beats(end, unit))
                .reduce(f64::max),
            "Audio" => number(content, "duration").map(|s| self.beats(s, TimeUnit::Seconds)),
            "Warps" => elements(content)
                .filter(|n| tag(*n) == "Warp")
                .filter_map(|n| number(n, "time"))
                .map(|t| self.beats(t, unit))
                .reduce(f64::max),
            "Clips" => elements(content)
                .filter_map(|c| Some(number(c, "time")? + number(c, "duration")?))
                .map(|end| self.beats(end, unit))
                .reduce(f64::max),
            _ => None,
        }
    }

    /// Flattens a window into windows onto notes or audio.
    pub(crate) fn resolve(
        &mut self,
        view: View<'a, 'input>,
        depth: u32,
        at: &str,
        out: &mut Vec<Resolved<'a, 'input>>,
    ) {
        if depth > MAX_DEPTH {
            self.report.dropped(
                Subject::Clip,
                at.to_owned(),
                "clips nested too deeply are left out",
            );
            return;
        }
        match tag(view.content) {
            "Notes" => out.push(Resolved::Notes(view)),
            "Audio" => out.push(Resolved::Audio(view, view.content, None)),
            "Warps" => match elements(view.content).find(|n| tag(*n) == "Audio") {
                Some(audio) => out.push(Resolved::Audio(view, audio, Some(view.content))),
                None => self.report.dropped(
                    Subject::Warp,
                    at.to_owned(),
                    "only warped audio is supported; other warped content is left out",
                ),
            },
            "Clips" => self.nested(view, depth, at, out),
            "Lanes" => {
                for inner in elements(view.content) {
                    let unit = TimeUnit::parse(inner.attribute("timeUnit")).unwrap_or(view.unit);
                    let nested = View {
                        content: inner,
                        unit,
                        ..view
                    };
                    self.resolve(nested, depth + 1, at, out);
                }
            }
            "Points" => self.report.dropped(
                Subject::Automation,
                at.to_owned(),
                "automation inside clips is not supported",
            ),
            other => self.report.dropped(
                Subject::Clip,
                at.to_owned(),
                format!("<{other}> clip content is not supported"),
            ),
        }
    }

    fn nested(
        &mut self,
        outer: View<'a, 'input>,
        depth: u32,
        at: &str,
        out: &mut Vec<Resolved<'a, 'input>>,
    ) {
        let unit = TimeUnit::parse(outer.content.attribute("timeUnit")).unwrap_or(outer.unit);
        let inner: Vec<View<'a, 'input>> = elements(outer.content)
            .filter(|n| tag(*n) == "Clip")
            .filter_map(|clip| self.view(clip, unit, at))
            .collect();
        if let Some(composed) = compose_loop(&outer, &inner) {
            self.resolve(composed, depth + 1, at, out);
            return;
        }
        let segments = unroll(&outer);
        if segments.len() >= MAX_PASSES {
            self.report.approximated(
                Subject::Clip,
                at.to_owned(),
                format!("a looping clip is cut after {MAX_PASSES} loop passes"),
            );
        }
        let last = segments.len().saturating_sub(1);
        for (index, segment) in segments.iter().enumerate() {
            for clip in &inner {
                let start = segment.content.max(clip.time);
                let end = (segment.content + segment.length).min(clip.time + clip.duration);
                if end - start <= EPSILON {
                    continue;
                }
                let at_outer_start = index == 0 && (start - segment.content).abs() < EPSILON;
                let at_outer_end =
                    index == last && (end - segment.content - segment.length).abs() < EPSILON;
                let at_inner_start = (start - clip.time).abs() < EPSILON;
                let at_inner_end = (end - clip.time - clip.duration).abs() < EPSILON;
                let fade_in = f64::max(
                    if at_outer_start { outer.fade_in } else { 0.0 },
                    if at_inner_start { clip.fade_in } else { 0.0 },
                );
                let fade_out = f64::max(
                    if at_outer_end { outer.fade_out } else { 0.0 },
                    if at_inner_end { clip.fade_out } else { 0.0 },
                );
                let piece = View {
                    name: clip.name.or(outer.name),
                    time: outer.time + segment.offset + (start - segment.content),
                    duration: end - start,
                    play_start: clip.advance(start - clip.time),
                    fade_in,
                    fade_out,
                    ..*clip
                };
                self.resolve(piece, depth + 1, at, out);
            }
        }
    }
}

/// One pass of an unrolled loop: `length` beats of the content from
/// `content`, starting `offset` beats into the clip.
#[derive(Clone, Copy, Debug)]
struct Segment {
    offset: f64,
    length: f64,
    content: f64,
}

/// A window as plain (non-looping) passes over its content.
fn unroll(view: &View<'_, '_>) -> Vec<Segment> {
    let Some((loop_start, loop_end)) = view.looped else {
        return vec![Segment {
            offset: 0.0,
            length: view.duration,
            content: view.play_start,
        }];
    };
    let mut segments = Vec::new();
    let mut position = view.advance(0.0);
    let mut elapsed = 0.0;
    while view.duration - elapsed > EPSILON && segments.len() < MAX_PASSES {
        let length = (loop_end - position).min(view.duration - elapsed);
        if length <= EPSILON {
            position = loop_start;
            continue;
        }
        segments.push(Segment {
            offset: elapsed,
            length,
            content: position,
        });
        elapsed += length;
        position = loop_start;
    }
    segments
}

/// A looping window onto exactly one clip that covers the whole loop and
/// does not loop itself becomes one looping window onto that clip's content.
fn compose_loop<'a, 'input>(
    outer: &View<'a, 'input>,
    inner: &[View<'a, 'input>],
) -> Option<View<'a, 'input>> {
    let (loop_start, loop_end) = outer.looped?;
    let [clip] = inner else {
        return None;
    };
    let first = outer.play_start.min(loop_start);
    let covers = clip.time <= first + EPSILON && clip.time + clip.duration + EPSILON >= loop_end;
    if clip.looped.is_some() || !covers {
        return None;
    }
    let shift = clip.play_start - clip.time;
    Some(View {
        name: clip.name.or(outer.name),
        play_start: outer.play_start + shift,
        looped: Some((loop_start + shift, loop_end + shift)),
        unit: clip.unit,
        content: clip.content,
        ..*outer
    })
}
