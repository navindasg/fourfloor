//! Arrangement editing: the vertical layout of tracks and automation
//! lanes, clip hit-testing, and the commands a drag, split or duplicate
//! produces.

use std::collections::HashSet;

use fourfloor_core::{ClipId, DeviceId, ParamId, Ticks, TrackId};
use fourfloor_model::command::{
    DeleteClip, DuplicateClip, MoveClip, ResizeClip, SplitClip, WriteAutomation,
    WriteTrackAutomation,
};
use fourfloor_model::{AutomationPoint, Command, MixerParam, Project, Track};

use crate::geometry::{Snap, TimeMap};

/// Height of a track row.
pub const TRACK_HEIGHT: f32 = 70.0;
/// Height of an automation lane.
pub const LANE_HEIGHT: f32 = 44.0;
/// Pixels from a clip's right edge that grab the resize handle.
pub const CLIP_EDGE_PX: f32 = 7.0;

/// What an automation lane controls.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LaneTarget {
    /// The track's fader or pan.
    Mixer(MixerParam),
    /// A device parameter.
    Device {
        /// The device.
        device: DeviceId,
        /// The parameter.
        param: ParamId,
    },
}

/// One automation lane under a track.
#[derive(Clone, Debug, PartialEq)]
pub struct LaneRow {
    /// What it controls.
    pub target: LaneTarget,
    /// Label.
    pub label: String,
    /// Top y.
    pub top: f32,
    /// Lowest value.
    pub min: f32,
    /// Highest value.
    pub max: f32,
    /// The lane's breakpoints.
    pub points: Vec<AutomationPoint>,
}

/// One track row and its lanes.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    /// The track.
    pub track: TrackId,
    /// Top y of the clip row.
    pub top: f32,
    /// Automation lanes shown under it.
    pub lanes: Vec<LaneRow>,
}

impl Row {
    /// Bottom y including lanes.
    pub fn bottom(&self) -> f32 {
        self.top + TRACK_HEIGHT + LANE_HEIGHT * self.lanes.len() as f32
    }
}

fn lanes_for(track: &Track) -> Vec<(LaneTarget, String, f32, f32, Vec<AutomationPoint>)> {
    let mixer = |param: MixerParam| {
        track
            .mixer_lane(param)
            .map(|lane| lane.points.clone())
            .unwrap_or_default()
    };
    let mut lanes = vec![
        (
            LaneTarget::Mixer(MixerParam::Gain),
            "Volume".to_owned(),
            -60.0,
            6.0,
            mixer(MixerParam::Gain),
        ),
        (
            LaneTarget::Mixer(MixerParam::Pan),
            "Pan".to_owned(),
            -1.0,
            1.0,
            mixer(MixerParam::Pan),
        ),
    ];
    for lane in &track.automation {
        let Some(device) = track.device(lane.device) else {
            continue;
        };
        let Some(param) = device.param(lane.param) else {
            continue;
        };
        lanes.push((
            LaneTarget::Device {
                device: lane.device,
                param: lane.param,
            },
            format!("{} {}", device.name, param.name),
            param.min,
            param.max,
            lane.points.clone(),
        ));
    }
    lanes
}

/// Lays out every track from `top`, with lanes for tracks in `expanded`.
pub fn layout_rows(project: &Project, top: f32, expanded: &HashSet<TrackId>) -> Vec<Row> {
    let mut y = top;
    project
        .tracks
        .iter()
        .map(|track| {
            let row_top = y;
            y += TRACK_HEIGHT;
            let lanes = if expanded.contains(&track.id) {
                lanes_for(track)
                    .into_iter()
                    .map(|(target, label, min, max, points)| {
                        let lane = LaneRow {
                            target,
                            label,
                            top: y,
                            min,
                            max,
                            points,
                        };
                        y += LANE_HEIGHT;
                        lane
                    })
                    .collect()
            } else {
                Vec::new()
            };
            Row {
                track: track.id,
                top: row_top,
                lanes,
            }
        })
        .collect()
}

/// The track row (not a lane) under `y`.
pub fn track_at(rows: &[Row], y: f32) -> Option<TrackId> {
    rows.iter()
        .find(|row| y >= row.top && y < row.top + TRACK_HEIGHT)
        .map(|row| row.track)
}

/// Which part of a clip a pointer is over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipPart {
    /// Dragging moves the clip.
    Body,
    /// Dragging resizes it.
    End,
}

/// The clip under `(x, y)`.
pub fn hit_clip(
    project: &Project,
    rows: &[Row],
    map: &TimeMap,
    x: f32,
    y: f32,
) -> Option<(TrackId, ClipId, ClipPart)> {
    let track_id = track_at(rows, y)?;
    let track = project.track(track_id)?;
    let ticks = map.ticks_at(x);
    let clip = track
        .clips
        .iter()
        .filter(|clip| ticks >= clip.start.get() as f64 && ticks < clip.end().get() as f64)
        .max_by_key(|clip| clip.start)?;
    let end_x = map.x_of_ticks(clip.end());
    let part = if end_x - x <= CLIP_EDGE_PX {
        ClipPart::End
    } else {
        ClipPart::Body
    };
    Some((track_id, clip.id, part))
}

/// A clip drag in progress.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipDrag {
    /// The clip.
    pub clip: ClipId,
    /// Its track when grabbed.
    pub track: TrackId,
    /// Moving or resizing.
    pub part: ClipPart,
    /// Clip start when grabbed.
    pub start: i64,
    /// Clip length when grabbed.
    pub length: i64,
    /// Pointer position in ticks when grabbed.
    pub grab: f64,
}

impl ClipDrag {
    /// The clip's start and length if the pointer is now at `ticks`.
    pub fn preview(&self, ticks: f64, snap: Snap) -> (i64, i64) {
        let raw = (ticks - self.grab).round() as i64;
        match self.part {
            ClipPart::Body => ((self.start + snap.round(raw)).max(0), self.length),
            ClipPart::End => {
                let end = snap.round(self.start + self.length + raw);
                (self.start, (end - self.start).max(snap.ticks().max(1)))
            }
        }
    }

    /// The command for releasing at `ticks` over `track`, or `None` when
    /// nothing changed. Moves only change track between tracks of the same
    /// kind.
    pub fn finish(
        &self,
        project: &Project,
        ticks: f64,
        track: Option<TrackId>,
        snap: Snap,
    ) -> Option<Command> {
        let (start, length) = self.preview(ticks, snap);
        match self.part {
            ClipPart::Body => {
                let from = project.track(self.track)?;
                let target = track
                    .filter(|id| *id != self.track)
                    .and_then(|id| project.track(id))
                    .filter(|to| to.kind == from.kind)
                    .map(|to| to.id);
                if start == self.start && target.is_none() {
                    return None;
                }
                Some(Command::MoveClip(MoveClip {
                    clip: self.clip,
                    start: Ticks::new(start),
                    track: target,
                }))
            }
            ClipPart::End => (length != self.length).then(|| {
                Command::ResizeClip(ResizeClip {
                    clip: self.clip,
                    length: Ticks::new(length),
                })
            }),
        }
    }
}

/// A split of `clip` at `at`, if `at` lies strictly inside it.
pub fn split(project: &Project, clip: ClipId, at: i64) -> Option<Command> {
    let (_, found) = project.find_clip(clip)?;
    (at > found.start.get() && at < found.end().get()).then(|| {
        Command::SplitClip(SplitClip {
            clip,
            at: Ticks::new(at),
        })
    })
}

/// A copy of `clip` right after it.
pub fn duplicate(clip: ClipId) -> Command {
    Command::DuplicateClip(DuplicateClip {
        clip,
        start: None,
        track: None,
    })
}

/// Deletes `clip`.
pub fn delete(clip: ClipId) -> Command {
    Command::DeleteClip(DeleteClip { clip })
}

/// Lane value to y inside a lane of height [`LANE_HEIGHT`] at `top`.
pub fn value_to_y(lane: &LaneRow, value: f32) -> f32 {
    let span = (lane.max - lane.min).max(f32::EPSILON);
    let t = ((value - lane.min) / span).clamp(0.0, 1.0);
    lane.top + LANE_HEIGHT - 4.0 - t * (LANE_HEIGHT - 8.0)
}

/// y inside a lane to a value.
pub fn y_to_value(lane: &LaneRow, y: f32) -> f32 {
    let t = ((lane.top + LANE_HEIGHT - 4.0 - y) / (LANE_HEIGHT - 8.0)).clamp(0.0, 1.0);
    lane.min + t * (lane.max - lane.min)
}

/// Points with one added (replacing any at the same time), sorted.
pub fn with_point(points: &[AutomationPoint], time: i64, value: f32) -> Vec<AutomationPoint> {
    let mut out: Vec<AutomationPoint> = points
        .iter()
        .filter(|point| point.time.get() != time)
        .copied()
        .collect();
    out.push(AutomationPoint {
        time: Ticks::new(time.max(0)),
        value,
    });
    out.sort_by_key(|point| point.time);
    out
}

/// Points without the one nearest `time` (within `tolerance` ticks).
pub fn without_point(
    points: &[AutomationPoint],
    time: f64,
    tolerance: f64,
) -> Option<Vec<AutomationPoint>> {
    let (index, _) = points
        .iter()
        .enumerate()
        .map(|(i, point)| (i, (point.time.get() as f64 - time).abs()))
        .filter(|(_, distance)| *distance <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    Some(
        points
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != index)
            .map(|(_, point)| *point)
            .collect(),
    )
}

/// The command that writes a lane's points.
pub fn write_lane(track: TrackId, target: LaneTarget, points: Vec<AutomationPoint>) -> Command {
    match target {
        LaneTarget::Mixer(param) => Command::WriteTrackAutomation(WriteTrackAutomation {
            track,
            param,
            points,
        }),
        LaneTarget::Device { device, param } => Command::WriteAutomation(WriteAutomation {
            device,
            param,
            points,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::{CreateTrack, WriteMidiClip};
    use fourfloor_model::{CommandBus, TrackKind};

    fn project() -> (CommandBus, TrackId, TrackId, ClipId) {
        let mut bus = CommandBus::new(Project::new("Arrange"));
        let mut tracks = Vec::new();
        for name in ["A", "B"] {
            tracks.push(
                bus.execute(CreateTrack {
                    name: name.to_owned(),
                    kind: TrackKind::Midi,
                    index: None,
                })
                .expect("track")
                .track()
                .expect("id"),
            );
        }
        let clip = bus
            .execute(WriteMidiClip {
                track: tracks[0],
                clip: None,
                name: "Clip".to_owned(),
                start: Ticks::new(3840),
                length: Ticks::new(3840),
                notes: vec![],
            })
            .expect("clip")
            .clip()
            .expect("id");
        (bus, tracks[0], tracks[1], clip)
    }

    #[test]
    fn rows_stack_with_their_lanes() {
        let (bus, a, b, _) = project();
        let expanded: HashSet<TrackId> = [a].into();
        let rows = layout_rows(bus.project(), 30.0, &expanded);
        assert_eq!(rows[0].lanes.len(), 2, "volume and pan");
        assert_eq!(rows[1].top, 30.0 + TRACK_HEIGHT + 2.0 * LANE_HEIGHT);
        assert_eq!(track_at(&rows, 31.0), Some(a));
        assert_eq!(
            track_at(&rows, rows[0].lanes[0].top + 1.0),
            None,
            "a lane is not a track"
        );
        assert_eq!(track_at(&rows, rows[1].top + 5.0), Some(b));
    }

    #[test]
    fn dragging_a_clip_moves_it_snapped_and_across_tracks() {
        let (mut bus, a, b, clip) = project();
        let rows = layout_rows(bus.project(), 0.0, &HashSet::new());
        let map = TimeMap::new(0.0, 0.0, 24.0);
        let x = map.x_of(4000.0);
        let (track, hit, part) = hit_clip(bus.project(), &rows, &map, x, 10.0).expect("hit");
        assert_eq!((track, hit, part), (a, clip, ClipPart::Body));
        let drag = ClipDrag {
            clip,
            track,
            part,
            start: 3840,
            length: 3840,
            grab: 4000.0,
        };
        assert_eq!(
            drag.preview(4000.0 + 1000.0, Snap::Beat),
            (3840 + 960, 3840)
        );
        let command = drag
            .finish(bus.project(), 4000.0 + 1000.0, Some(b), Snap::Beat)
            .expect("a move");
        bus.execute(command).expect("moved");
        let (owner, moved) = bus.project().find_clip(clip).expect("clip");
        assert_eq!((owner.id, moved.start.get()), (b, 4800));
        assert!(
            drag.finish(bus.project(), 4010.0, None, Snap::Beat)
                .is_none(),
            "no change"
        );
    }

    #[test]
    fn the_right_edge_resizes_and_splits_need_an_inside_point() {
        let (mut bus, _, _, clip) = project();
        let rows = layout_rows(bus.project(), 0.0, &HashSet::new());
        let map = TimeMap::new(0.0, 0.0, 24.0);
        let edge = map.x_of(7680.0) - 2.0;
        let (track, _, part) = hit_clip(bus.project(), &rows, &map, edge, 10.0).expect("hit");
        assert_eq!(part, ClipPart::End);
        let drag = ClipDrag {
            clip,
            track,
            part,
            start: 3840,
            length: 3840,
            grab: 7680.0,
        };
        let command = drag
            .finish(bus.project(), 7680.0 + 2000.0, None, Snap::Bar)
            .expect("resize");
        bus.execute(command).expect("resized");
        assert_eq!(
            bus.project().find_clip(clip).expect("clip").1.length.get(),
            7680
        );
        assert!(split(bus.project(), clip, 3840).is_none(), "at the start");
        let cut = split(bus.project(), clip, 7680).expect("inside");
        bus.execute(cut).expect("split");
        bus.execute(duplicate(clip)).expect("duplicate");
        assert_eq!(bus.project().tracks[0].clips.len(), 3);
        bus.execute(delete(clip)).expect("delete");
        assert_eq!(bus.project().tracks[0].clips.len(), 2);
    }

    #[test]
    fn automation_points_map_to_lane_space_and_back() {
        let lane = LaneRow {
            target: LaneTarget::Mixer(MixerParam::Pan),
            label: "Pan".to_owned(),
            top: 100.0,
            min: -1.0,
            max: 1.0,
            points: vec![],
        };
        for value in [-1.0f32, -0.25, 0.0, 1.0] {
            assert!((y_to_value(&lane, value_to_y(&lane, value)) - value).abs() < 1e-5);
        }
        let points = with_point(&[], 960, 0.5);
        let points = with_point(&points, 0, -0.5);
        let points = with_point(&points, 960, 0.25);
        assert_eq!(points.len(), 2);
        assert_eq!(points[1].value, 0.25);
        assert_eq!(without_point(&points, 950.0, 30.0).expect("near").len(), 1);
        assert!(without_point(&points, 500.0, 30.0).is_none());
    }
}
