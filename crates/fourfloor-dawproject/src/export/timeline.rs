//! Export of the arrangement (clips and automation per track) and of the
//! clip launcher (scenes and slots).

use fourfloor_core::Ticks;
use fourfloor_model::{AutomationPoint, Clip, ClipContent, MixerParam, Track};

use super::{audio_path, location, Exporter};
use crate::naming;
use crate::report::Subject;
use crate::units::{
    db_to_linear, pan_to_normalized, ticks_to_beats, ticks_to_seconds, velocity_to_unit,
};
use crate::xml::{El, XmlWriter};

impl Exporter<'_> {
    pub(super) fn arrangement(&mut self, w: &mut XmlWriter) {
        w.open(El::new("Arrangement").attr("id", naming::ARRANGEMENT));
        w.open(
            El::new("Lanes")
                .attr("id", naming::ARRANGEMENT_LANES)
                .attr("timeUnit", "beats"),
        );
        let tracks = self.project.tracks.clone();
        for track in &tracks {
            w.open(
                El::new("Lanes")
                    .attr("id", naming::track_lanes(track.id))
                    .attr("track", naming::track(track.id)),
            );
            w.open(El::new("Clips").attr("id", naming::track_clips(track.id)));
            for clip in &track.clips {
                self.clip(w, track, clip, clip.start);
            }
            w.close();
            self.automation(w, track);
            w.close();
        }
        w.close();
        w.close();
    }

    pub(super) fn scenes(&mut self, w: &mut XmlWriter) {
        w.open(El::new("Scenes"));
        let tracks = self.project.tracks.clone();
        for scene in &self.project.scenes {
            w.open(
                El::new("Scene")
                    .attr("id", naming::scene(scene.id))
                    .attr("name", &scene.name),
            );
            w.open(
                El::new("Lanes")
                    .attr("id", naming::scene_lanes(scene.id))
                    .attr("timeUnit", "beats"),
            );
            for track in &tracks {
                let Some(slot) = track.slot(scene.id) else {
                    continue;
                };
                w.open(
                    El::new("ClipSlot")
                        .attr("id", naming::slot(track.id, scene.id))
                        .attr("track", naming::track(track.id))
                        .attr("hasStop", true),
                );
                self.clip(w, track, &slot.clip, Ticks::ZERO);
                w.close();
            }
            w.close();
            w.close();
        }
        w.close();
    }

    fn clip(&mut self, w: &mut XmlWriter, track: &Track, clip: &Clip, start: Ticks) {
        match &clip.content {
            ClipContent::Midi { notes } => {
                let mut el = El::new("Clip")
                    .attr("name", &clip.name)
                    .attr("time", ticks_to_beats(start))
                    .attr("duration", ticks_to_beats(clip.length));
                let offset = match clip.loop_length {
                    Some(period) => Ticks::new(clip.offset.get().rem_euclid(period.get())),
                    None => clip.offset,
                };
                el = el.attr("playStart", ticks_to_beats(offset));
                if let Some(period) = clip.loop_length {
                    el = el
                        .attr("loopStart", 0)
                        .attr("loopEnd", ticks_to_beats(period));
                }
                w.open(el);
                w.open(El::new("Notes").attr("id", naming::notes(clip.id.get())));
                for note in notes {
                    w.leaf(
                        El::new("Note")
                            .attr("time", ticks_to_beats(note.start))
                            .attr("duration", ticks_to_beats(note.length))
                            .attr("channel", 0)
                            .attr("key", note.pitch)
                            .attr("vel", velocity_to_unit(note.velocity)),
                    );
                }
                w.close();
                w.close();
            }
            ClipContent::Audio { .. } => self.audio_clip(w, track, clip, start),
        }
    }

    fn audio_clip(&mut self, w: &mut XmlWriter, track: &Track, clip: &Clip, start: Ticks) {
        let ClipContent::Audio {
            source,
            source_offset_frames,
            fade_in,
            fade_out,
            warp,
            ..
        } = &clip.content
        else {
            return;
        };
        let Some(info) = self.audio_info(track, source) else {
            self.report.missing(
                Subject::Clip,
                location(track, &clip.name),
                "the clip's audio file is unavailable, so the clip is left out",
            );
            return;
        };
        let tempo = self.project.tempo;
        // Content units per second of the file, and a tick span in content
        // units: seconds for a plain clip, beats of the source for a warp.
        let (per_second, span): (f64, Box<dyn Fn(Ticks) -> f64>) = match warp {
            Some(warp) => (warp.source_bpm / 60.0, Box::new(ticks_to_beats)),
            None => (1.0, Box::new(move |t| ticks_to_seconds(t, tempo))),
        };
        let file_offset = *source_offset_frames as f64 / f64::from(info.sample_rate) * per_second;
        let mut el = El::new("Clip")
            .attr("name", &clip.name)
            .attr("time", ticks_to_beats(start))
            .attr("duration", ticks_to_beats(clip.length))
            .attr(
                "contentTimeUnit",
                if warp.is_some() { "beats" } else { "seconds" },
            );
        match clip.loop_length {
            Some(period) => {
                let offset = Ticks::new(clip.offset.get().rem_euclid(period.get()));
                el = el
                    .attr("playStart", file_offset + span(offset))
                    .attr("loopStart", file_offset)
                    .attr("loopEnd", file_offset + span(period));
            }
            None => el = el.attr("playStart", file_offset + span(clip.offset)),
        }
        if *fade_in != Ticks::ZERO || *fade_out != Ticks::ZERO {
            el = el
                .attr("fadeTimeUnit", "beats")
                .attr("fadeInTime", ticks_to_beats(*fade_in))
                .attr("fadeOutTime", ticks_to_beats(*fade_out));
        }
        w.open(el);
        let audio = |w: &mut XmlWriter, algorithm: Option<&str>| {
            w.open(
                El::new("Audio")
                    .attr("id", naming::audio(clip.id.get()))
                    .opt("algorithm", algorithm)
                    .attr("channels", info.channels)
                    .attr("sampleRate", info.sample_rate)
                    .attr("duration", info.seconds),
            );
            w.leaf(El::new("File").attr("path", audio_path(source)));
            w.close();
        };
        match warp {
            Some(warp) => {
                w.open(
                    El::new("Warps")
                        .attr("id", naming::warps(clip.id.get()))
                        .attr("timeUnit", "beats")
                        .attr("contentTimeUnit", "seconds"),
                );
                audio(w, Some(if warp.key_lock { "stretch" } else { "repitch" }));
                w.leaf(El::new("Warp").attr("time", 0).attr("contentTime", 0));
                w.leaf(
                    El::new("Warp")
                        .attr("time", info.seconds * per_second)
                        .attr("contentTime", info.seconds),
                );
                w.close();
            }
            None => audio(w, None),
        }
        w.close();
    }

    fn automation(&mut self, w: &mut XmlWriter, track: &Track) {
        for lane in &track.automation {
            let target = naming::param(lane.device, lane.param);
            let id = naming::param_lane(lane.device, lane.param);
            points(w, &id, &target, "linear", &lane.points, f64::from);
        }
        for lane in &track.mixer_automation {
            match lane.param {
                MixerParam::Gain => {
                    let varies = lane.points.windows(2).any(|p| p[0].value != p[1].value);
                    if varies {
                        self.report.approximated(
                            Subject::Automation,
                            location(track, "volume automation"),
                            "written as linear gain: other applications ramp the gain linearly between points, fourfloor ramps in decibels",
                        );
                    }
                    let (id, target) = (naming::volume_lane(track.id), naming::volume(track.id));
                    points(w, &id, &target, "linear", &lane.points, db_to_linear);
                }
                MixerParam::Pan => {
                    let (id, target) = (naming::pan_lane(track.id), naming::pan(track.id));
                    points(
                        w,
                        &id,
                        &target,
                        "normalized",
                        &lane.points,
                        pan_to_normalized,
                    );
                }
            }
        }
    }
}

fn points(
    w: &mut XmlWriter,
    id: &str,
    target: &str,
    unit: &'static str,
    points: &[AutomationPoint],
    value: impl Fn(f32) -> f64,
) {
    w.open(El::new("Points").attr("id", id).attr("unit", unit));
    w.leaf(El::new("Target").attr("parameter", target));
    for point in points {
        w.leaf(
            El::new("RealPoint")
                .attr("time", ticks_to_beats(point.time))
                .attr("value", value(point.value))
                .attr("interpolation", "linear"),
        );
    }
    w.close();
}
