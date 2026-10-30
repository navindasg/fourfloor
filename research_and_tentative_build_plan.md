# Open-Source Rust House DAW: Build Plan

Oct 11, 2026 · @Navin Dasgupta

## Summary

Build it in Rust from day one, license it GPL-3.0-or-later, and use nothing that is paid or has non-commercial terms. Once the commercial escape hatch is gone, GPL stops being a constraint and becomes an asset: you can borrow code and ideas from Mixxx, Ardour, Surge XT, Matchering and Rubber Band instead of designing around them.

What changed from the first plan:

- **JUCE and Tracktion Engine are out.** They only made sense as a route to buying commercial licences later, and they are C++ you would end up rewriting.
- **Plugin hosting now has a pure-Rust option.** truce-rack (MIT/Apache) hosts CLAP, VST3, AU v2, AU v3 and LV2 through Rust bindings, with no C++ glue. It first shipped in May 2026, so plan to fork it and own it.
- **The AI layer uses only open weights.** Spleeter (MIT code and models) replaces Demucs for stems. Basic Pitch and LAION CLAP stay. The agent runs on a local open-weights LLM instead of a paid API.
- **The roadmap starts with a Rust spike** on hosting, latency and time-stretch instead of a Tracktion demo app.

The stack in one line per layer:

| Layer | Pick |
| --- | --- |
| Audio and MIDI I/O | cpal, midir |
| Engine | Your own graph, transport and edit model in Rust, lock-free queues (rtrb), ideas borrowed from Firewheel |
| Plugin hosting | truce-rack, built on clack, vst3 and objc2 |
| Your own instruments and effects | Faust compiled to Rust, wrapped with nih-plug (BillyDM's maintained fork) when you want them as standalone plugins |
| Time-stretch | Signalsmith Stretch (MIT) through Rust bindings, then a pure-Rust port |
| UI | egui on wgpu |
| Project format | Versioned serde project folder, plus DAWproject import and export |
| AI inference | In-process Rust (candle or ort), no Python in the shipped app |
| Agent API | MCP server built on the official Rust SDK (rmcp), driven by a local open-weights model |

## Licensing stance

License the app GPL-3.0-or-later. The rule for every dependency, model and sample: an OSI-approved code license, and for weights or audio an open license that allows commercial use (MIT, Apache-2.0, CC0 or CC BY). Nothing with a paid tier, a research-only clause or a non-commercial clause.

Why GPL-3.0-or-later rather than MIT: it is the one license that can absorb everything useful here.

| Component license | Usable in a GPL-3.0-or-later app? | Examples |
| --- | --- | --- |
| MIT, Apache-2.0, ISC, BSD, Zlib, CC0 | Yes | cpal, clack, vst3, truce-rack, Signalsmith Stretch, ebur128, Airwindows, Open303, Basic Pitch |
| GPL-2.0-or-later, GPL-3.0 | Yes | Rubber Band, Mixxx, Ardour, Surge XT, OB-Xf, libKeyFinder, Matchering, bliss-rs |
| LGPL, MPL-2.0 | Yes | SoundTouch, LSP Plugins, symphonia |
| AGPL-3.0 | Technically yes (GPLv3 section 13), but the network clause spreads to that part | Zrythm, Essentia's code. Not needed, so avoid |
| GPL-2.0-only | No | Check each one; don't assume |
| Non-commercial, research-only, freeware | No, by your rule | Demucs weights, Essentia models, Freesound CC BY-NC sounds, OB-Xd 2.12+ binaries |

Three consequences worth knowing:

- **Hosting closed plugins is fine.** Loading Serum or FabFilter at runtime through VST3, AU or CLAP does not affect your license. The rule above applies to what you ship, not what users load.
- **You can still sell it.** GPL allows charging for binaries, which is how Ardour funds itself. The catch is that anyone can rebuild and redistribute it for free.
- **macOS notarization costs money.** It requires the paid Apple Developer Program, so drop it. Ship from source, through a Homebrew formula, or as ad-hoc signed builds that users approve once in Gatekeeper. Windows has no equivalent requirement for unsigned builds, only a SmartScreen warning.

This is not legal advice. The mixing rules above are the standard FSF compatibility guidance, but check anything unusual before you distribute.

## Feature matrix

Every feature from the benchmark stays; only the build path changes. "Best at" is an editorial judgment carried over from the first report, not a measured comparison. Anything marked "own" is code you write in Rust, usually with AI help.

### Production

| Feature | Priority | Best at | Open-source Rust path |
| --- | --- | --- | --- |
| Low-latency engine (under 10 ms round trip) | Must | Logic, Ableton | cpal on CoreAudio, own graph, no allocation or locks on the audio thread |
| AU + VST3 hosting with delay compensation | Must | Logic (AU), Bitwig (sandboxing) | truce-rack for loading; delay compensation is yours: read each plugin's reported latency and align parallel paths |
| CLAP hosting | Nice | Bitwig | truce-rack-clap (clack underneath) |
| LV2 hosting | Skip | Ardour | truce-rack-lv2 exists if you ever want it |
| Clip launcher + linear arrangement | Must | Ableton, Bitwig | Own edit model; DAWproject's schema is a good reference for clips, lanes and warping |
| Step sequencer with swing and groove templates | Must | FL Studio, Ableton | Own; grooves stored as per-step timing and velocity offsets |
| Sampler with slicing and transient detection | Must | Ableton Simpler, FL Slicex | Own sampler, symphonia for decoding, spectral-flux onset detection |
| Warping and time-stretch with key lock | Must | Ableton | Signalsmith Stretch through Rust bindings, then a pure-Rust port |
| Piano roll with scale and chord tools | Must | FL Studio, Logic | Own |
| MIDI effects (arp, chord) | Must | Ableton, Bitwig | Own internal devices |
| Sidechain + volume shaping for pumping | Must | Bitwig, Ableton | Sidechain routing in your graph, own LFO shaper, compressor in Faust |
| Tempo-synced filters and delays | Must | All | Faust compiled to Rust |
| Macros | Must | Ableton, Bitwig | Own parameter-mapping layer |
| Automation + modulation (LFOs, envelopes) | Must | Bitwig | Own modulation system, block-rate first, sample-accurate later |
| Ableton Link + MIDI clock | Nice | Ableton | Link is GPLv2+, fine under GPL-3; MIDI clock through midir |
| MIDI controller mapping | Must | Ableton, Bitwig | midir + MIDI learn |

### Sound

| Feature | Priority | Best at | Open-source Rust path |
| --- | --- | --- | --- |
| 909/808 drums | Must | Logic, Ableton Suite | Own drum voices in Faust, plus CC0 samples |
| Organ basses and M1-style stabs | Must | Logic, Ableton Suite | Dexed (GPL-3) hosted, or your own FM voice |
| Piano and chord stabs | Must | Logic, Ableton Suite | Surge XT and Dexed hosted |
| Juno-style pads | Must | Logic, Ableton Suite | Surge XT, OB-Xf, Odin 2 hosted |
| 303-style acid | Must | Logic, Ableton Suite | Port Open303 (MIT) to Rust as a built-in device |
| Chopped vocals | Must | Ableton | Your sampler + Spleeter stems |
| Effects chain (EQ, comp, glue, reverb, delay, chorus, saturation, limiter) | Must | Logic, Ableton | Faust for EQ, comp, chorus, limiter; Airwindows (MIT) ported for saturation and console glue; Dragonfly Reverb and LSP hosted |

### Mixdown and mastering

| Feature | Priority | Best at | Open-source Rust path |
| --- | --- | --- | --- |
| Buses, sends, groups | Must | All | Your graph |
| Metering (LUFS, true peak, correlation, spectrum) | Must | Logic, Bitwig | ebur128 crate (pure Rust, MIT), rustfft, own correlation meter |
| Reference A/B with loudness matching | Must | None natively | Own, on top of ebur128 |
| Stem and master export | Must | All | Offline render path through the same graph |

### DJ mixing

| Feature | Priority | Best at | Open-source Rust path |
| --- | --- | --- | --- |
| 2 to 4 decks | Must | Traktor, Rekordbox | Decks as tracks on the same engine |
| Beatgrid + BPM analysis | Must | Rekordbox | Port Mixxx's analyzers (GPL, now allowed) |
| Key detection + harmonic mixing | Must | Mixxx, Rekordbox | Port libKeyFinder (GPL-3) to Rust |
| Sync | Must | Traktor | Tempo-master model in your transport |
| EQ + filter per deck | Must | All | Faust |
| Loops + hot cues | Must | Serato, Rekordbox | Own |
| Deck effects | Nice | Traktor | Reuse your devices |
| Library management | Must | Rekordbox | SQLite (public domain) through rusqlite, Mixxx's schema as reference |
| Mix recording | Must | All | Own |
| Stem-based mixing | Nice | Serato, Traktor, Rekordbox, Mixxx 2.6 | Spleeter pre-separation, cached per track |
| Vinyl / DVS control | Skip | Serato | None |

### Sample library

| Feature | Priority | Best at | Open-source Rust path |
| --- | --- | --- | --- |
| Browser with BPM, key and tags | Must | Ableton 12, Logic | Same analyzers as DJ mode, SQLite index |
| Preview in project tempo and key | Must | Ableton | Signalsmith Stretch |
| Similarity search | Nice | Ableton 12 | LAION CLAP embeddings + bliss-rs features + a vector index |

## Component inventory

The Rust audio ecosystem now covers the two hardest pieces, plugin hosting and loudness metering, in pure Rust. Time-stretch and key detection still need a port. The Plan column says what you do with each: **use** it as a dependency, **bind then port** (wrap the C/C++ now, rewrite in Rust later), **port** (rewrite from the start), **host** (load as a separate plugin), or **reference** (read it, don't depend on it). Licenses marked "not re-verified" come from memory or secondary sources; check the LICENSE file before relying on them.

### Engine, I/O and hosting

| Component | What it does | License | Status (Oct 2026) | Plan |
| --- | --- | --- | --- | --- |
| [truce-rack](https://github.com/truce-audio/truce-rack) | Plugin host: CLAP, VST3, AU v2, AU v3, LV2, with GUI embedding and host transport | MIT or Apache-2.0 | v1.1.5 (19 Jun 2026); first release May 2026; about 500 downloads | Use, then fork and own |
| [clack](https://github.com/prokopyl/clack) | Safe CLAP host and plugin wrappers | MIT or Apache-2.0 | 0.1.x | Use (via truce-rack) |
| [vst3](https://github.com/coupler-rs/vst3-rs) | VST3 bindings generated from the SDK headers | MIT or Apache-2.0 | 0.3.0 (Dec 2025), bindings now shipped in the crate | Use (via truce-rack) |
| cpal | Audio I/O (CoreAudio, WASAPI, ASIO) | Apache-2.0 (not re-verified) | 0.18.x | Use |
| midir | MIDI I/O | MIT (not re-verified) | Active | Use |
| [Firewheel](https://github.com/BillyDM/firewheel) | Audio graph engine | MIT or Apache-2.0 | 0.12.x; moving into Bevy; says it is not a DAW engine | Reference for graph design |
| rtrb | Lock-free single-producer, single-consumer ring buffer | MIT or Apache-2.0 (not re-verified) | Stable | Use |
| symphonia | Pure-Rust audio decoding | MPL-2.0 (not re-verified) | Active | Use |

### DSP and analysis

| Component | What it does | License | Status (Oct 2026) | Plan |
| --- | --- | --- | --- | --- |
| [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch) | Time-stretch and pitch-shift (C++) | MIT | 1.1.0 (Jun 2025) | Bind then port |
| [signalsmith-stretch](https://docs.rs/crate/signalsmith-stretch/0.1.2) / [ssstretch](https://lib.rs/crates/ssstretch) | Rust bindings to the above | MIT | 0.1.3 (Sep 2025) / 0.1.0 (Mar 2025) | Use for the MVP |
| [cute-dsp](https://docs.rs/crate/cute-dsp/latest) | Pure-Rust port of Signalsmith DSP, including a stretch module | MIT | 0.0.40; quality vs the C++ original unverified | Evaluate as your port's starting point |
| [Rubber Band](https://breakfastquay.com/rubberband/license.html) | High-quality stretch, R3 engine (C++) | GPL-2.0-or-later | v4.0 (Oct 2024) | Optional offline HQ render through FFI; don't port |
| [ebur128](https://github.com/sdroege/ebur128) | EBU R128 loudness, LRA, true peak; pure Rust | MIT | 0.1.10; passes EBU Tech 3341 and 3342 | Use |
| rustfft | FFT for spectrum and analysis | MIT or Apache-2.0 (not re-verified) | Stable | Use |
| [libKeyFinder](https://github.com/mixxxdj/libkeyfinder) | Key detection (C++) | GPL-3.0-or-later | 2.2.8 | Port |
| [Mixxx](https://github.com/mixxxdj/mixxx) analyzers | Beatgrid and BPM (C++) | GPL | 2.5.x stable | Reference and port |
| [bliss-rs](https://github.com/Polochon-street/bliss-rs) | Tempo, timbre and chroma features for track similarity | GPL-3.0-only | Active (crate: bliss-audio) | Use for DJ library similarity |
| [Open303](https://github.com/maddanio/open303/pull/1/files) | TB-303 emulation (C++) | MIT | Stable, small | Port |
| [Airwindows](https://github.com/baconpaul/airwin2rack) | Hundreds of effects: saturation, consoles, compressors (C++) | MIT | Very active | Port the ones you use; host the rest as "Airwindows Consolidated" |
| Faust | DSP language that compiles to Rust | Compiler GPL/LGPL; generated code is yours | Active | Use for stock devices |

### Plugin frameworks (for shipping your own devices as plugins)

| Component | What it does | License | Status (Oct 2026) | Plan |
| --- | --- | --- | --- | --- |
| [nih-plug (BillyDM fork)](https://codeberg.org/jwagner/nih-plug) | Rust plugin framework (VST3, CLAP) | ISC | Original author stopped maintaining it; hard fork now on Codeberg | Use if you export devices as plugins |
| [truce](https://github.com/Boscop/truce) | Rust plugin framework (CLAP, VST3, LV2, AU v2/v3) | Not re-verified | 0.48.x, active | Alternative to nih-plug; skip its AAX target, which needs paid PACE tooling |

### Hosted instruments and effects

Ship these as separate plugins (or point users to them), never linked into your binary. That keeps your app's dependency tree small and lets each project update on its own schedule.

| Component | Role in house | License | Status (Oct 2026) |
| --- | --- | --- | --- |
| Surge XT | Pads, stabs, basses | GPL-3.0 | 1.3.4 stable |
| [OB-Xf](https://github.com/surge-synthesizer/OB-Xf/wiki) | Oberheim-style pads and brass stabs; replaces OB-Xd | GPL-3.0-or-later | Released by Surge Synth Team; nightlies ongoing |
| Vitalium | Modern wavetable basses and pads (open fork of Vital) | GPL-3.0-or-later | Via DISTRHO-Ports |
| Dexed | DX7 FM: bells, organs, piano stabs | GPL-3.0 | 0.9.9 + nightlies |
| Odin 2 | Analog-style poly | GPL-3.0 | Slow updates |
| Dragonfly Reverb | Hall, room, plate | GPL-3.0 | 3.2.10 |
| LSP Plugins | EQ, compressors, limiter, analyzers | LGPL-3.0 | 1.2.35; build Windows yourself (official Windows builds are paid) |

### UI, project, agent and ML

| Component | What it does | License | Status (Oct 2026) | Plan |
| --- | --- | --- | --- | --- |
| egui + wgpu | Immediate-mode UI on the GPU | MIT or Apache-2.0 (not re-verified) | Active | Use |
| [DAWproject](https://github.com/bitwig/dawproject) | Open XML/ZIP project interchange (Bitwig, Studio One, others) | MIT | Format 1.0, stable | Import and export |
| SQLite via rusqlite | Sample and track library | Public domain / MIT (rusqlite not re-verified) | Stable | Use |
| [rmcp](https://github.com/modelcontextprotocol/rust-sdk) | Official Rust MCP SDK | Apache-2.0 for new code; older contributions MIT | 3.x; implements the 2026-07-28 MCP spec | Use |
| [mistral.rs](https://www.everydev.ai/tools/mistral-rs) | Local LLM inference in Rust, Metal, MCP client | MIT | 0.8.x | Use for the built-in agent |
| candle / ort | In-process model inference (Metal or ONNX Runtime) | MIT or Apache-2.0 (not re-verified) | Active | Use for stems, audio-to-MIDI, embeddings |

## Architecture

One Rust process, a strict split between the real-time audio thread and everything else, and a single command bus that both the UI and the AI agent write to. Plugins run in-process for the MVP and move out-of-process before the tool is trusted on real projects.

&#91;embedded content: architecture · one process, one command path\]

Parameter tweaks skip the graph compiler and reach the audio thread directly through rtrb queues. Meters and the playhead flow back to the UI through atomics.

- **Edit model and command bus.** The project is plain Rust structs (tracks, clips, devices, automation) with serde. Every change is a typed command such as `create_track`, `set_param` or `write_midi_clip`. Commands go on an undo stack, then compile into a new audio graph or a parameter message. Because the UI and the agent share this path, every LLM edit can be undone.
- **Audio thread.** Owns an immutable graph snapshot plus the transport. It receives commands through rtrb queues and publishes meters and playhead position through atomics. No allocation, locks or logging: enforce it with an allocation guard (such as `assert_no_alloc`) in debug builds. Graph rebuilds happen on a worker and swap in at a block boundary; the old graph is freed off-thread.
- **Plugin hosting.** truce-rack loads plugins in-process at first. Delay compensation is yours: read each plugin's reported latency and add delay on shorter parallel paths, recomputing when a plugin reports a change. Later, move plugins into helper processes with shared-memory audio, so a crash takes down one helper, not your session. That costs a block of latency per crossing, which is why Bitwig makes it optional.
- **Worker pool.** Disk streaming for long clips and DJ decks, offline stretching for renders, analysis (BPM, key, onsets, loudness) and ML inference. Nothing in this pool may block the audio thread.
- **UI.** egui on wgpu, one window, custom-painted arrangement, piano roll, waveforms and meters. It reads a UI-side copy of the edit model and only changes things by sending commands.
- **Project format.** A project folder: a versioned `project.json` (with schema migrations), an audio pool with content-hashed filenames, and plugin state blobs. DAWproject import and export lets you move a session into Bitwig or Studio One.
- **Agent API.** An rmcp server inside the app (stdio, or Streamable HTTP on localhost) exposes tools that map one-to-one to commands, plus resources such as a project summary and a loudness report. A built-in chat panel runs a local model through mistral.rs; any external MCP client works too.
- **ML inference.** candle on Metal, or ort, on a worker thread. Models download on first use and are recorded in a registry with their license, so a non-open model can never slip in.
- **Windows.** cpal covers WASAPI and ASIO, and truce-rack covers VST3 and CLAP. AU code sits behind a macOS-only compile flag.

## AI-native features with open weights

Every AI feature survives the open-only rule. The one real loss is stem quality: Spleeter's MIT models are a clear step below Demucs, and no open-weights separator matching Demucs was verified. Everything else keeps its edge, and the agent gets better because it runs locally with no API bill.

| Feature | Model or library | Code license | Weights license | On Apple Silicon | Your edge over commercial DAWs |
| --- | --- | --- | --- | --- | --- |
| Stem separation | [Spleeter](https://joss.theoj.org/papers/10.21105/joss.02154.pdf) (2, 4 or 5 stems) | MIT | MIT (trained on Deezer's internal data) | Offline; the README cites 100x faster than real time on a GPU. Needs a TensorFlow-to-ONNX conversion for Rust | Low on its own (Ableton, Logic, the DJ apps ship stems). The edge is stems flowing straight into your slicer and audio-to-MIDI |
| Audio to MIDI | Basic Pitch (Spotify) | Apache-2.0 | Apache-2.0, ships ONNX and CoreML builds | Faster than real time on CPU | High: separate a disco loop, convert the bass to MIDI, re-voice it in Surge XT |
| Sample search by text and similarity | [LAION CLAP](https://huggingface.co/laion/larger_clap_general/discussions) `larger_clap_general` | CC0 (repo) | Apache-2.0 | Milliseconds per one-shot; index a large library in the background | High: "dusty 909 open hat" across your whole library. Skip `larger_clap_music`, which two reports say behaves as untrained |
| DJ track similarity | bliss-rs | GPL-3.0-only | None (pure DSP) | Fast, batch | Medium: "what mixes well after this" from your own crates |
| Generative drums and basslines | Your own rule-based or Markov generator over MIDI you own; [GrooVAE](https://huggingface.co/AEmotionStudio/drumstudio-groovae-models) for humanizing | Apache-2.0 | Apache-2.0 (trained on the CC BY 4.0 Groove MIDI Dataset) | Real time for rules; GrooVAE near real time after conversion | Medium: house-specific shuffle, offbeat bass and organ patterns |
| Mixdown suggestions | ebur128 + spectrum + rules; Matchering's algorithm ported (GPL-3, now allowed); an LLM explains the findings | MIT / GPL-3 | None | Offline, seconds | High: per-bus comparison against a reference, not just the master |
| Natural-language control | rmcp server + mistral.rs running Qwen3 (4B or 8B) or gpt-oss-20b | Apache-2.0 / MIT | Apache-2.0 | Real time for commands; gpt-oss-20b needs about 16 GB of memory | Highest: "swing the hats to 56%, sidechain the pad to the kick, build an 8-bar filter riser" |

Three rules that keep this open:

- **Weights are checked like code.** The model registry records each model's license and refuses anything non-commercial. That excludes Demucs weights (research-only), Essentia's models (CC BY-NC-ND), MusicGen-style generators (non-commercial weights) and models under custom terms such as Gemma's or Llama's.
- **Better stems are a training problem, not a licensing one.** Demucs's code is MIT, so a separator trained on open data would be fully open. The blocker is that the standard training sets (MUSDB18, MoisesDB) are non-commercial. Treat this as a later research project.
- **Any MCP client still works.** The built-in panel defaults to a local model, but the server is standard MCP, so you can point a different client at it whenever you choose.

## Dropped or replaced

Everything from the first plan that cost money, carried a non-commercial clause, or only existed to keep a closed-source option open.

| From the first plan | Why it's out | Replacement |
| --- | --- | --- |
| JUCE 9 | Recommended for its commercial licence path; C++ you'd rewrite | Own Rust engine, truce-rack for hosting, egui for UI |
| Tracktion Engine | Same, plus a paid per-seat tier for distribution | Own edit model and graph, DAWproject as a schema reference |
| Rubber Band commercial licence | Paid | Rubber Band under GPL as an optional offline renderer; Signalsmith Stretch as the default |
| Ableton Link proprietary licence | Only needed for closed source | Link's GPLv2+ option |
| Demucs weights | Research-only terms | Spleeter (MIT code and models) |
| Essentia | AGPL code, CC BY-NC-ND models | ebur128, bliss-rs, a libKeyFinder port, Mixxx's analyzers |
| Matchering as a walled-off subprocess | The workaround only mattered for closed source | Port its algorithm directly |
| `larger_clap_music` checkpoint | Reported to behave as untrained | `larger_clap_general` (Apache-2.0) |
| OB-Xd 2.12+ | Freeware binary, no source | OB-Xf (GPL-3.0-or-later) |
| Vital official build | Freemium with paid tiers and preset packs | Vitalium |
| sfizz | Archived June 2026 | Your own sampler |
| LSP official Windows builds | Paid | Build from source |
| Freesound CC BY-NC sounds, "royalty-free" 909 packs | Non-commercial or custom terms | CC0 and CC BY sounds only, plus your own synthesized 909/808 voices |
| Apple notarization | Needs the paid Apple Developer Program | Homebrew formula or ad-hoc signed builds |
| Paid LLM APIs as the default | Cost money | Local open-weights models through mistral.rs |
| Python ML sidecar | Not a licensing issue, but conflicts with an all-Rust end state | In-process inference with candle or ort |
| Web UI in a WebView | Same | egui on wgpu |

## Roadmap

Plan on 13 to 18 weeks of evenings and weekends before you finish a track in it. That is longer than the first plan's MVP because you build the edit model and graph yourself instead of inheriting Tracktion's. In exchange, nothing has to be rewritten later.

&#91;embedded content: roadmap · 6 phases, each with an exit gate\]

The C++ that remains along the way is small and fenced off: Signalsmith Stretch bindings until Phase 5, and optionally Rubber Band for offline renders. Everything else is Rust from the first commit, including the ports of Open303, Airwindows, libKeyFinder and Mixxx's analyzers.

### Riskiest pieces and how to validate them

| Risk | Why it's hard | How to validate |
| --- | --- | --- |
| Plugin hosting maturity | truce-rack is about five months old with roughly 500 downloads; AU v3 and plugin editors are the usual failure points | Phase 0: load your 20 most-used plugins in each format; save and restore state and open and close editors 50 times each |
| Latency and dropouts | Allocation or locks on the audio thread; UI and agent contention | Loopback-cable round-trip measurement, an xrun counter, and the allocation guard running in CI |
| Delay compensation | Easy to get subtly wrong with sends, sidechains and latency changes | Null test: route a track through a latent plugin and a bypassed twin, invert one, and confirm they cancel |
| Time-stretch quality | Signalsmith is tuned for modest ratios; drums smear first | Blind A/B against Rubber Band R3 at 0.9x, 1.03x and 1.1x on drums, vocals and full mixes |
| C++ to Rust ports | Small numeric differences change the sound | Golden-file tests: render the same input through the C++ and Rust versions and diff within a tolerance |
| Plugin crashes | In-process, one bad plugin kills the session | Fuzz-load your plugin folder in CI; out-of-process hosting in Phase 5 |
| Stem quality | Spleeter sits below Demucs | Accept it for sampling; listen-test on your own source material |

AI coding tools speed up UI, glue code and ports that have golden-file tests. They help much less with real-time bugs and with judging sound, so those phases keep human-scale estimates.

## Unverified items and caveats

- **truce-rack is unproven.** Everything known about it comes from its README and crates.io: format coverage, GUI embedding and transport support are its own claims. Phase 0 exists to test them.
- **cute-dsp's stretch quality is unknown.** It claims to port Signalsmith's algorithms; nobody has published a comparison against the C++ original.
- **Licenses not re-read from LICENSE files:** cpal, midir, rtrb, symphonia, rustfft, egui, wgpu, candle, ort, rusqlite and the truce framework. Each is widely reported as MIT, Apache-2.0 or MPL-2.0.
- **Spleeter in Rust needs a model conversion.** Its official models are TensorFlow. Exporting them to ONNX is a known path, but no specific conversion was checked here.
- **GrooVAE's weight license** comes from a Hugging Face mirror's card (Apache-2.0) plus the Groove MIDI Dataset's CC BY 4.0 terms, not from Google's original bucket.
- **`larger_clap_music` behaving as untrained** comes from two independent reports in the first research pass. Benchmark any CLAP checkpoint on your own samples.
- **Items carried over from the first report without re-checking:** VST3 SDK 3.8 under MIT, the Demucs weights restriction, Essentia's model license, Freesound's license options, Surge XT, Dexed, Odin 2, Dragonfly and LSP status, and whether Mixxx 2.6 is final.
- **Time estimates are judgment,** assuming one experienced engineer on evenings and weekends with heavy AI coding.
- **Not legal advice.** The compatibility rules in the licensing section follow standard FSF guidance.

## Sources

Checked for this revision:

- Plugin hosting: [truce-rack (README)](https://github.com/Boscop/truce-rack), [truce-rack upstream](https://github.com/truce-audio/truce-rack), [truce-rack on crates.io](https://crates.io/api/v1/crates/truce-rack), [clack](https://github.com/prokopyl/clack), [vst3-rs](https://github.com/coupler-rs/vst3-rs), [vst3 0.3.0 Cargo.toml](https://docs.rs/crate/vst3/latest/source/Cargo.toml), [vst3 0.3.0 release post](https://micahrj.github.io/posts/vst3)
- Engine and frameworks: [Firewheel](https://github.com/BillyDM/firewheel), [Firewheel 0.12 README](https://docs.rs/crate/firewheel/latest), [nih-plug fork notice](https://codeberg.org/jwagner/nih-plug), [nih-plug on Codeberg](https://codeberg.org/BillyDM/nih-plug), [truce](https://github.com/Boscop/truce), [truce on docs.rs](https://docs.rs/crate/truce/latest/source/README.md)
- DSP and analysis: [Signalsmith Stretch](https://github.com/Signalsmith-Audio/signalsmith-stretch), [Signalsmith Stretch on vcpkg](https://vcpkg.io/en/package/signalsmith-stretch.html), [signalsmith-stretch crate](https://docs.rs/crate/signalsmith-stretch/0.1.2), [ssstretch](https://lib.rs/crates/ssstretch), [cute-dsp](https://docs.rs/crate/cute-dsp/latest), [Rubber Band license](https://breakfastquay.com/rubberband/license.html), [ebur128](https://github.com/sdroege/ebur128), [Open303 MIT license](https://github.com/maddanio/open303/pull/1/files), [Airwindows via airwin2rack](https://www.github.com/baconpaul/airwin2rack), [bliss-rs](https://github.com/Polochon-street/bliss-rs), [bliss-audio license field](https://docs.rs/crate/bliss-audio/0.2.0/source/Cargo.toml.orig)
- Instruments: [OB-Xf](https://github.com/surge-synthesizer/OB-Xf/wiki), [OB-Xf beta coverage (CDM)](https://cdm.link/ob-xf-free-beta/)
- Project and agent: [DAWproject](https://github.com/bitwig/dawproject), [DAWproject MIT license (CDM)](https://cdm.link/dawformat-bitwig-presonus/), [rmcp](https://github.com/modelcontextprotocol/rust-sdk), [rmcp LICENSE](https://raw.githubusercontent.com/modelcontextprotocol/rust-sdk/main/LICENSE), [mistral.rs](https://github.com/EricLBuehler/mistral.rs), [mistral.rs license summary](https://www.everydev.ai/tools/mistral-rs)
- AI models: [Spleeter paper (JOSS)](https://joss.theoj.org/papers/10.21105/joss.02154.pdf), [Spleeter](https://github.com/deezer/spleeter), [LAION larger\_clap\_general](https://huggingface.co/laion/larger_clap_general), [GrooVAE checkpoint mirror](https://huggingface.co/AEmotionStudio/drumstudio-groovae-models), [Groove MIDI Dataset](https://magenta.tensorflow.org/datasets/groove), [Qwen3 and gpt-oss licenses](https://huggingface.co/blog/daya-shankar/open-source-llm-models-to-run-locally)

Carried over from the first report: [VST3 licensing](https://steinbergmedia.github.io/vst3_dev_portal/pages/VST+3+Licensing/Index.html), [Essentia licensing](https://essentia.upf.edu/licensing_information.html), [Freesound FAQ](https://freesound.org/help/faq/), [Ableton Link](https://github.com/Ableton/link), [Surge XT](https://github.com/surge-synthesizer/surge), [Mixxx](https://github.com/mixxxdj/mixxx), [libKeyFinder](https://github.com/mixxxdj/libkeyfinder), [Demucs](https://github.com/adefossez/demucs)

