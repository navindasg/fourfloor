//! What a project means before any graph exists: its length, each track's
//! notes on the timeline, and which faders mute and solo leave open.

use fourfloor_core::{Ticks, TrackId};
use fourfloor_engine::devices::PlayerNote;
use fourfloor_model::{Project, Send, Track, TrackKind};

/// Converts decibels to a linear gain factor.
pub fn db_to_linear(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

/// End of the last clip in the project, or zero for a project with no clips.
pub fn project_length(project: &Project) -> Ticks {
    project
        .tracks
        .iter()
        .flat_map(|track| track.clips.iter())
        .map(|clip| clip.end())
        .max()
        .unwrap_or(Ticks::ZERO)
}

/// Every note a track's timeline clips sound, on the project timeline:
/// each clip's offset and loop are applied and notes are cut off at the
/// clip's end. Launcher slots are not included.
pub fn track_notes(track: &Track) -> Vec<PlayerNote> {
    track
        .clips
        .iter()
        .flat_map(|clip| {
            clip.sounding_notes().into_iter().map(move |note| {
                PlayerNote::new(
                    clip.start + note.start,
                    note.length,
                    note.pitch,
                    note.velocity,
                )
            })
        })
        .collect()
}

/// The bus a send goes to, if its target is an existing bus track other
/// than the sending track.
pub(crate) fn send_bus<'a>(project: &'a Project, track: &Track, send: &Send) -> Option<&'a Track> {
    let bus = project.track(send.target)?;
    (bus.kind == TrackKind::Bus && bus.id != track.id).then_some(bus.as_ref())
}

/// The bus a track feeds, if its `output` names an existing bus track
/// other than itself. Anything else goes to the master.
pub(crate) fn output_bus<'a>(project: &'a Project, track: &Track) -> Option<&'a Track> {
    let bus = project.track(track.output?)?;
    (bus.kind == TrackKind::Bus && bus.id != track.id).then_some(bus.as_ref())
}

/// The buses downstream of `track`, nearest first. A valid project has no
/// routing loops; the hop limit keeps a broken one from spinning here.
pub(crate) fn downstream<'a>(project: &'a Project, track: &Track) -> Vec<&'a Track> {
    let mut stages = Vec::new();
    let mut current = output_bus(project, track);
    while let Some(bus) = current {
        if stages.len() > project.tracks.len() {
            break;
        }
        stages.push(bus);
        current = output_bus(project, bus);
    }
    stages
}

/// Whether `track` (a bus) carries `source`, directly or through other
/// buses.
fn carries(project: &Project, track: TrackId, source: &Track) -> bool {
    downstream(project, source)
        .iter()
        .any(|bus| bus.id == track)
}

/// Whether mute or solo closes this track's own fader.
///
/// With no solo anywhere, only the track's mute counts. With a solo, the
/// fader stays open when the track is soloed, feeds a soloed bus, or is a
/// bus carrying a soloed track.
pub(crate) fn fader_closed(project: &Project, track: &Track) -> bool {
    if track.mute {
        return true;
    }
    if !project.tracks.iter().any(|other| other.solo) {
        return false;
    }
    let soloed_downstream = downstream(project, track).iter().any(|bus| bus.solo);
    let carries_solo = project
        .tracks
        .iter()
        .any(|other| other.solo && carries(project, track.id, other));
    !(track.solo || soloed_downstream || carries_solo)
}

/// Whether sound from `track` reaches the master: its own fader and every
/// fader downstream are open.
pub(crate) fn audible(project: &Project, track: &Track) -> bool {
    !fader_closed(project, track)
        && downstream(project, track)
            .iter()
            .all(|bus| !fader_closed(project, bus))
}

#[cfg(test)]
mod tests {
    use super::*;
    use fourfloor_model::command::{CreateTrack, SetTrackMute, SetTrackOutput, SetTrackSolo};
    use fourfloor_model::sequencer::{write_house_pattern, HousePattern, HousePatternWritten};
    use fourfloor_model::CommandBus;

    fn house() -> (CommandBus, HousePatternWritten) {
        let mut bus = CommandBus::new(Project::new("Test"));
        let written = write_house_pattern(&mut bus, &HousePattern::default()).unwrap();
        (bus, written)
    }

    fn add_bus(bus: &mut CommandBus, name: &str) -> TrackId {
        bus.execute(CreateTrack {
            name: name.to_owned(),
            kind: TrackKind::Bus,
            index: None,
        })
        .unwrap()
        .track()
        .unwrap()
    }

    fn open(bus: &CommandBus, track: TrackId) -> bool {
        let project = bus.project();
        audible(project, project.track(track).unwrap())
    }

    #[test]
    fn db_conversion_matches_known_points() {
        assert_eq!(db_to_linear(0.0), 1.0);
        assert!((db_to_linear(-6.0) - 0.501_187).abs() < 1e-5);
        assert!((db_to_linear(20.0) - 10.0).abs() < 1e-5);
    }

    #[test]
    fn notes_are_placed_on_the_timeline_and_cut_at_the_clip_end() {
        let mut bus = CommandBus::new(Project::new("Test"));
        write_house_pattern(
            &mut bus,
            &HousePattern {
                start: Ticks::from_whole_beats(8),
                ..HousePattern::default()
            },
        )
        .unwrap();
        let kick = track_notes(&bus.project().tracks[0]);
        let starts: Vec<i64> = kick.iter().map(|n| n.start.get()).collect();
        assert_eq!(starts, [7680, 8640, 9600, 10560]);
        assert!(kick
            .iter()
            .all(|n| n.start + n.length <= Ticks::from_whole_beats(12)));
        assert_eq!(project_length(bus.project()), Ticks::from_whole_beats(12));
        assert_eq!(project_length(&Project::new("Empty")), Ticks::ZERO);
    }

    #[test]
    fn mute_closes_a_track_and_everything_routed_through_a_muted_bus() {
        let (mut bus, written) = house();
        let drums = add_bus(&mut bus, "Drums");
        bus.execute(SetTrackOutput {
            track: written.kick_track,
            output: Some(drums),
        })
        .unwrap();
        assert!(open(&bus, written.kick_track) && open(&bus, written.hat_track));

        bus.execute(SetTrackMute {
            track: drums,
            mute: true,
        })
        .unwrap();
        assert!(!open(&bus, written.kick_track), "its bus is muted");
        assert!(open(&bus, written.hat_track));
        let project = bus.project();
        let kick = project.track(written.kick_track).unwrap();
        assert!(!fader_closed(project, kick), "its own fader stays open");
        assert_eq!(downstream(project, kick).len(), 1);
    }

    #[test]
    fn solo_keeps_the_soloed_track_its_buses_and_what_feeds_a_soloed_bus() {
        let (mut bus, written) = house();
        let drums = add_bus(&mut bus, "Drums");
        let other = add_bus(&mut bus, "Other");
        bus.execute(SetTrackOutput {
            track: written.kick_track,
            output: Some(drums),
        })
        .unwrap();

        // Solo the kick: its bus must stay open to carry it.
        bus.execute(SetTrackSolo {
            track: written.kick_track,
            solo: true,
        })
        .unwrap();
        assert!(open(&bus, written.kick_track));
        assert!(!open(&bus, written.hat_track));
        let project = bus.project();
        assert!(!fader_closed(project, project.track(drums).unwrap()));
        assert!(fader_closed(project, project.track(other).unwrap()));
        bus.undo().unwrap();

        // Solo the bus: what feeds it sounds, the rest does not.
        bus.execute(SetTrackSolo {
            track: drums,
            solo: true,
        })
        .unwrap();
        assert!(open(&bus, written.kick_track));
        assert!(!open(&bus, written.hat_track));
    }
}
