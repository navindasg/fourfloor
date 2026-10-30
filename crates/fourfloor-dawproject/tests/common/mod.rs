//! Shared test material: the device registry, a rich project using
//! built-in devices, and rendering helpers.

#![allow(dead_code)]

pub mod xsd;

use std::path::Path;
use std::sync::Arc;

use fourfloor_compile::RenderOptions;
use fourfloor_core::{
    ClipId, DeviceId, DeviceRegistry, ParamId, SampleRate, Tempo, Ticks, TrackId,
};
use fourfloor_model::{
    AudioPool, AudioRef, AutomationLane, AutomationPoint, Clip, ClipContent, ControllerMapping,
    DeviceInstance, Groove, LfoRate, LfoShape, Macro, MacroId, MacroTarget, MidiNote, MixerLane,
    MixerParam, ModSource, ModTarget, Modulator, ModulatorId, Param, PluginStateStore, Project,
    Scene, SceneId, Send, SessionSlot, SidechainSource, SwingGrid, Track, TrackKind, Warp,
};
use fourfloor_testkit::{sine, white_noise, StereoBuffer};

pub const BEAT: i64 = 960;
pub const BAR: i64 = BEAT * 4;
pub const RATE: u32 = 48_000;

pub fn registry() -> DeviceRegistry {
    let mut registry = DeviceRegistry::new();
    fourfloor_engine::devices::register(&mut registry).unwrap();
    fourfloor_instruments::register(&mut registry).unwrap();
    fourfloor_effects::register(&mut registry).unwrap();
    registry
}

/// A device with every parameter at its default, as the app would add it.
pub fn device(registry: &DeviceRegistry, id: u64, kind: &str, name: &str) -> DeviceInstance {
    let descriptor = registry
        .descriptor(kind)
        .unwrap_or_else(|| panic!("no device `{kind}`"));
    DeviceInstance {
        id: DeviceId::new(id),
        kind: kind.to_owned(),
        name: name.to_owned(),
        bypassed: false,
        params: descriptor
            .params
            .iter()
            .map(|info| Param {
                id: info.id,
                name: info.name.to_string(),
                value: info.default,
                min: info.min,
                max: info.max,
            })
            .collect(),
        state_blob: None,
        sidechain: None,
    }
}

/// The id of the first parameter whose name contains `part`.
pub fn param(device: &DeviceInstance, part: &str) -> ParamId {
    device
        .params
        .iter()
        .find(|p| {
            p.name
                .to_ascii_lowercase()
                .contains(&part.to_ascii_lowercase())
        })
        .map(|p| p.id)
        .unwrap_or_else(|| {
            let names: Vec<_> = device.params.iter().map(|p| p.name.as_str()).collect();
            panic!("{} has no `{part}` parameter: {names:?}", device.kind)
        })
}

pub fn set(device: DeviceInstance, id: ParamId, value: f32) -> DeviceInstance {
    let params = device
        .params
        .iter()
        .map(|p| Param {
            value: if p.id == id {
                value.clamp(p.min, p.max)
            } else {
                p.value
            },
            ..p.clone()
        })
        .collect();
    DeviceInstance { params, ..device }
}

pub fn note(start: i64, length: i64, pitch: u8, velocity: u8) -> MidiNote {
    MidiNote {
        start: Ticks::new(start),
        length: Ticks::new(length),
        pitch,
        velocity,
    }
}

pub fn midi_clip(id: u64, name: &str, start: i64, length: i64, notes: Vec<MidiNote>) -> Clip {
    let mut notes = notes;
    notes.sort_by_key(|n| (n.start, n.pitch));
    Clip {
        id: ClipId::new(id),
        name: name.to_owned(),
        start: Ticks::new(start),
        length: Ticks::new(length),
        offset: Ticks::ZERO,
        loop_length: None,
        content: ClipContent::Midi { notes },
    }
}

pub fn point(time: i64, value: f32) -> AutomationPoint {
    AutomationPoint {
        time: Ticks::new(time),
        value,
    }
}

/// Two seconds of 44.1 kHz stereo test audio: a tone with noise bursts on
/// every half second, different on each side.
pub fn loop_audio() -> StereoBuffer {
    let rate = SampleRate::new(44_100).unwrap();
    let tone = sine(330.0, 0.3, 2.0, rate);
    let noise = white_noise(7, 0.4, 2.0, rate);
    let left: Vec<f32> = tone
        .iter()
        .zip(&noise)
        .enumerate()
        .map(|(i, (t, n))| t + if (i % 22_050) < 2_000 { *n } else { 0.0 })
        .collect();
    let right: Vec<f32> = tone.iter().map(|t| t * 0.5).collect();
    StereoBuffer::new(rate, left, right)
}

/// Stores the test audio in the project folder's pool.
pub fn store_audio(dir: &Path) -> AudioRef {
    let wav = dir.join("loop-source.wav");
    loop_audio().write_wav(&wav).unwrap();
    let source = AudioPool::new(dir).import_file(&wav).unwrap();
    std::fs::remove_file(&wav).unwrap();
    source
}

fn audio_content(
    source: &AudioRef,
    offset_frames: u64,
    gain_db: f32,
    warp: Option<Warp>,
) -> ClipContent {
    ClipContent::Audio {
        source: source.clone(),
        source_offset_frames: offset_frames,
        gain_db,
        fade_in: Ticks::new(120),
        fade_out: Ticks::new(240),
        warp,
    }
}

/// Built-in devices only, so everything renders. Uses every feature the
/// model has, including the ones DAWproject cannot express.
pub fn rich_project(registry: &DeviceRegistry, dir: &Path) -> Project {
    let source = store_audio(dir);
    let state = PluginStateStore::new(dir)
        .put(b"opaque plugin state \x00\x01\x02")
        .unwrap();

    // Ids: tracks 1-6, devices 10-19, clips 20-39, scenes 40-41, mods 50+.
    let kick_drum = device(registry, 10, "drum.kick909", "Kick 909");
    let acid = device(registry, 11, "synth.acid303", "Acid");
    let cutoff = param(&acid, "cutoff");
    let acid = set(acid, cutoff, 900.0);
    let delay = device(registry, 12, "fx.delay", "Delay");
    let pad = device(registry, 13, "poly.pad", "Pad");
    let compressor = DeviceInstance {
        sidechain: Some(SidechainSource {
            track: TrackId::new(1),
            pre_fader: true,
        }),
        ..device(registry, 14, "fx.compressor", "Ducker")
    };
    let eq = device(registry, 15, "fx.eq", "Group EQ");
    let reverb = device(registry, 16, "fx.reverb", "Hall");
    let plugin = DeviceInstance {
        id: DeviceId::new(17),
        // Escaped the way fourfloor-host builds kinds.
        kind: "plugin.clap:org.example.not%20installed".to_owned(),
        name: "Missing Plugin".to_owned(),
        bypassed: false,
        params: vec![Param {
            id: ParamId::new(3_000_000_000),
            name: "Big id".to_owned(),
            value: 0.25,
            min: 0.0,
            max: 1.0,
        }],
        state_blob: Some(state),
        sidechain: None,
    };
    let bypassed = DeviceInstance {
        bypassed: true,
        ..device(registry, 18, "fx.saturator", "Dirt (off)")
    };
    let chord = device(registry, 19, "note.chord", "Chord");

    let kick_notes = (0..4).map(|b| note(b * BEAT, 240, 36, 120)).collect();
    let kick = Track {
        devices: vec![Arc::new(kick_drum.clone())],
        clips: vec![Arc::new(Clip {
            loop_length: Some(Ticks::new(BAR)),
            ..midi_clip(20, "Four on the floor", 0, BAR * 4, kick_notes)
        })],
        automation: vec![Arc::new(AutomationLane {
            device: kick_drum.id,
            param: param(&kick_drum, "level"),
            points: vec![
                point(0, 0.8),
                point(BAR * 2, 0.5),
                point(BAR * 2, 0.9),
                point(BAR * 4, 0.6),
            ],
        })],
        gain_db: -6.5,
        ..Track::new(TrackId::new(1), "Kick", TrackKind::Midi)
    };

    let acid_notes = vec![
        note(0, 240, 36, 100),
        note(240, 240, 48, 110),
        note(480, 480, 39, 80),
        note(1_200, 240, 43, 127),
        note(1_920, 960, 36, 64),
        note(3_000, 400, 41, 90),
    ];
    let acid_track = Track {
        devices: vec![Arc::new(acid.clone()), Arc::new(delay), Arc::new(bypassed)],
        clips: vec![
            Arc::new(midi_clip(21, "Acid A", 0, BAR * 2, acid_notes.clone())),
            // The right half of a split: an offset into its content.
            Arc::new(Clip {
                offset: Ticks::new(BAR),
                ..midi_clip(22, "Acid B", BAR * 2, BAR, acid_notes.clone())
            }),
            // A loop with an offset past one loop length.
            Arc::new(Clip {
                offset: Ticks::new(BAR + 480),
                loop_length: Some(Ticks::new(BAR)),
                ..midi_clip(23, "Acid C", BAR * 3, BAR, acid_notes.clone())
            }),
        ],
        automation: vec![Arc::new(AutomationLane {
            device: acid.id,
            param: cutoff,
            points: vec![point(0, 400.0), point(BAR * 4, 3_000.0)],
        })],
        mixer_automation: vec![
            Arc::new(MixerLane {
                param: MixerParam::Gain,
                points: vec![point(0, -12.0), point(BAR, -3.0), point(BAR * 3, -6.25)],
            }),
            Arc::new(MixerLane {
                param: MixerParam::Pan,
                points: vec![point(0, -0.5), point(BAR * 4, 0.5)],
            }),
        ],
        sends: vec![Send {
            target: TrackId::new(6),
            gain_db: -6.0,
            pre_fader: false,
        }],
        slots: vec![SessionSlot {
            scene: SceneId::new(40),
            clip: Arc::new(Clip {
                loop_length: Some(Ticks::new(BAR)),
                ..midi_clip(24, "Acid Slot", 0, BAR, acid_notes)
            }),
        }],
        pan: -0.3,
        ..Track::new(TrackId::new(2), "Acid & Co <303>", TrackKind::Midi)
    };

    let pad_track = Track {
        devices: vec![Arc::new(chord), Arc::new(pad), Arc::new(compressor)],
        clips: vec![Arc::new(midi_clip(
            25,
            "Pad",
            0,
            BAR * 4,
            vec![note(0, BAR * 2, 57, 90), note(BAR * 2, BAR * 2, 53, 90)],
        ))],
        output: Some(TrackId::new(5)),
        sends: vec![Send {
            target: TrackId::new(6),
            gain_db: -10.0,
            pre_fader: true,
        }],
        mute: false,
        ..Track::new(TrackId::new(3), "Pad", TrackKind::Midi)
    };

    let loop_track = Track {
        clips: vec![
            // Looped, with a file offset.
            Arc::new(Clip {
                id: ClipId::new(26),
                name: "Loop".to_owned(),
                start: Ticks::new(0),
                length: Ticks::new(BAR * 2),
                offset: Ticks::new(BEAT / 2),
                loop_length: Some(Ticks::new(BAR)),
                content: audio_content(&source, 4_410, 0.0, None),
            }),
            // Warped (record-deck style, so no time-stretch is needed),
            // with clip gain.
            Arc::new(Clip {
                id: ClipId::new(27),
                name: "Warped".to_owned(),
                start: Ticks::new(BAR * 2),
                length: Ticks::new(BAR),
                offset: Ticks::ZERO,
                loop_length: None,
                content: audio_content(
                    &source,
                    0,
                    -3.0,
                    Some(Warp {
                        source_bpm: 96.5,
                        key_lock: false,
                    }),
                ),
            }),
            // Not looped, with both a file offset and a content offset.
            Arc::new(Clip {
                id: ClipId::new(28),
                name: "Split".to_owned(),
                start: Ticks::new(BAR * 3),
                length: Ticks::new(BAR),
                offset: Ticks::new(BEAT),
                loop_length: None,
                content: audio_content(&source, 2_205, 0.0, None),
            }),
        ],
        gain_db: -8.0,
        pan: 0.25,
        solo: false,
        ..Track::new(TrackId::new(4), "Loop", TrackKind::Audio)
    };

    let group = Track {
        devices: vec![Arc::new(eq)],
        gain_db: -1.0,
        ..Track::new(TrackId::new(5), "Group", TrackKind::Bus)
    };
    let reverb_bus = Track {
        devices: vec![Arc::new(reverb), Arc::new(plugin)],
        gain_db: -3.0,
        ..Track::new(TrackId::new(6), "Reverb", TrackKind::Bus)
    };

    let project = Project {
        name: "Rich Project".to_owned(),
        tempo: Tempo::new(124.0).unwrap(),
        groove: Groove {
            swing_percent: 58.0,
            grid: SwingGrid::Sixteenth,
        },
        tracks: [kick, acid_track, pad_track, loop_track, group, reverb_bus]
            .into_iter()
            .map(Arc::new)
            .collect(),
        next_id: 60,
        scenes: vec![
            Scene {
                id: SceneId::new(40),
                name: "Drop".to_owned(),
            },
            Scene {
                id: SceneId::new(41),
                name: "Empty".to_owned(),
            },
        ],
        launch_quantize: Ticks::new(BEAT),
        modulators: vec![Arc::new(Modulator {
            id: ModulatorId::new(50),
            name: "Wobble".to_owned(),
            source: ModSource::Lfo {
                shape: LfoShape::Sine,
                rate: LfoRate::Beats(2.0),
                phase: 0.0,
            },
            targets: vec![ModTarget {
                device: DeviceId::new(11),
                param: cutoff,
                depth: 0.2,
            }],
        })],
        macros: vec![Arc::new(Macro {
            id: MacroId::new(51),
            name: "Space".to_owned(),
            value: 0.5,
            targets: vec![MacroTarget {
                device: DeviceId::new(16),
                param: ParamId::new(0),
                min: 0.0,
                max: 1.0,
            }],
        })],
        controller_map: vec![ControllerMapping {
            channel: None,
            controller: 74,
            device: DeviceId::new(11),
            param: cutoff,
            min: 200.0,
            max: 4_000.0,
        }],
    };
    project.validate().unwrap();
    project
}

/// The rich project cut down to what DAWproject itself can say: no
/// swing, modulation, sidechain or clip gain, and no audio clip whose
/// file offset and content offset both matter.
pub fn plain_project(rich: &Project) -> Project {
    let tracks = rich
        .tracks
        .iter()
        .map(|track| {
            let devices = track
                .devices
                .iter()
                .map(|d| {
                    Arc::new(DeviceInstance {
                        sidechain: None,
                        ..(**d).clone()
                    })
                })
                .collect();
            let fix = |clip: &Arc<Clip>| {
                let offset = match clip.loop_length {
                    Some(period) => Ticks::new(clip.offset.get() % period.get()),
                    None if clip.is_midi() => clip.offset,
                    None => Ticks::ZERO,
                };
                let content = match &clip.content {
                    ClipContent::Audio {
                        source,
                        source_offset_frames,
                        fade_in,
                        fade_out,
                        warp,
                        ..
                    } => ClipContent::Audio {
                        source: source.clone(),
                        source_offset_frames: *source_offset_frames,
                        gain_db: 0.0,
                        fade_in: *fade_in,
                        fade_out: *fade_out,
                        warp: *warp,
                    },
                    midi => midi.clone(),
                };
                Arc::new(Clip {
                    offset,
                    content,
                    ..(**clip).clone()
                })
            };
            let clips = track.clips.iter().map(fix).collect();
            let slots = track
                .slots
                .iter()
                .map(|slot| SessionSlot {
                    scene: slot.scene,
                    clip: fix(&slot.clip),
                })
                .collect();
            Arc::new(Track {
                devices,
                clips,
                slots,
                ..(**track).clone()
            })
        })
        .collect();
    let project = Project {
        tracks,
        groove: Groove::STRAIGHT,
        launch_quantize: fourfloor_model::DEFAULT_LAUNCH_QUANTIZE,
        modulators: Vec::new(),
        macros: Vec::new(),
        controller_map: Vec::new(),
        ..rich.clone()
    };
    project.validate().unwrap();
    project
}

/// The project with ids renumbered in the order an import without the
/// extension file hands them out: tracks, then devices in track order, then
/// timeline clips in track order, then each scene followed by its slots.
/// Only for projects without modulators, macros, mappings or sidechains.
pub fn renumber(project: &Project) -> Project {
    use std::collections::HashMap;
    let mut map: HashMap<u64, u64> = HashMap::new();
    let mut next = 1u64;
    let mut claim = |old: u64, map: &mut HashMap<u64, u64>| {
        map.insert(old, next);
        next += 1;
    };
    for track in &project.tracks {
        claim(track.id.get(), &mut map);
    }
    for track in &project.tracks {
        for device in &track.devices {
            claim(device.id.get(), &mut map);
        }
    }
    for track in &project.tracks {
        for clip in &track.clips {
            claim(clip.id.get(), &mut map);
        }
    }
    for scene in &project.scenes {
        claim(scene.id.get(), &mut map);
        for track in &project.tracks {
            if let Some(slot) = track.slot(scene.id) {
                claim(slot.clip.id.get(), &mut map);
            }
        }
    }
    let id = |old: u64| map[&old];
    let track_id = |old: TrackId| TrackId::new(id(old.get()));
    let clip = |c: &Arc<Clip>| {
        Arc::new(Clip {
            id: ClipId::new(id(c.id.get())),
            ..(**c).clone()
        })
    };
    let tracks = project
        .tracks
        .iter()
        .map(|t| {
            Arc::new(Track {
                id: track_id(t.id),
                output: t.output.map(track_id),
                sends: t
                    .sends
                    .iter()
                    .map(|s| Send {
                        target: track_id(s.target),
                        ..*s
                    })
                    .collect(),
                devices: t
                    .devices
                    .iter()
                    .map(|d| {
                        Arc::new(DeviceInstance {
                            id: DeviceId::new(id(d.id.get())),
                            ..(**d).clone()
                        })
                    })
                    .collect(),
                automation: t
                    .automation
                    .iter()
                    .map(|l| {
                        Arc::new(AutomationLane {
                            device: DeviceId::new(id(l.device.get())),
                            ..(**l).clone()
                        })
                    })
                    .collect(),
                clips: t.clips.iter().map(clip).collect(),
                slots: t
                    .slots
                    .iter()
                    .map(|s| SessionSlot {
                        scene: SceneId::new(id(s.scene.get())),
                        clip: clip(&s.clip),
                    })
                    .collect(),
                ..(**t).clone()
            })
        })
        .collect();
    let scenes = project
        .scenes
        .iter()
        .map(|s| Scene {
            id: SceneId::new(id(s.id.get())),
            name: s.name.clone(),
        })
        .collect();
    Project {
        tracks,
        scenes,
        next_id: next,
        ..project.clone()
    }
}

pub fn render(project: &Project, registry: &DeviceRegistry, dir: &Path) -> StereoBuffer {
    let mut options = RenderOptions::new(SampleRate::new(RATE).unwrap()).with_tail(1.0);
    options.compile = options.compile.with_project_dir(dir);
    fourfloor_testkit::render_project_with(project, registry, &options).unwrap()
}
