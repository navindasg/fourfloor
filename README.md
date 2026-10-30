# fourfloor

An open-source digital audio workstation for house music, written in Rust.

Licensed GPL-3.0-or-later. Every dependency, model and sample must carry an
OSI-approved license (for weights and audio: MIT, Apache-2.0, CC0 or
CC BY, so commercial use is allowed). See
`research_and_tentative_build_plan.md` for the plan this follows.

**State (2026-10-16): a working early build.** The engine, 14 instruments,
12 effects, the edit model, the project compiler, CLAP and Audio Unit
hosting, a DJ mode, a sample library, audio analysis, three open-weights
ML models, DAWproject interchange, an MCP agent server, a command line and
an egui desktop app all exist and are tested on rendered audio. Nothing has
been used for real music-making by a person yet, and the gaps at the end of
this file are real.

## Build and run

Needs stable Rust 1.90 or newer (`rust-toolchain.toml` selects stable). On
Linux, audio output needs the ALSA development headers (cpal). The optional
`stretch` features need a C++ compiler and libclang.

```sh
cargo build --workspace
cargo test --workspace

cargo run -p fourfloor-cli -- demo my-track                  # full house track, 28 bars
cargo run -p fourfloor-cli -- render my-track -o my-track.wav
cargo run -p fourfloor-cli -- analyze my-track.wav           # BPM, key, loudness
cargo run -p fourfloor-cli -- play my-track                  # default audio output
cargo run -p fourfloor-app -- my-track                       # the desktop app
```

A rendered copy of the demo is kept (untracked) in `demo-out/demo.wav`,
with its project in `demo-out/project` and stems in `demo-out/stems`.

### Command line (`fourfloor`)

| Command | What it does |
| --- | --- |
| `new <dir> [--name] [--bpm] [--bars] [--swing]` | A kick and hat pattern on the 909 kick and hat devices, built through the command bus. |
| `demo <dir> [--name] [--bpm]` | A full house track built only through edit commands: 124 BPM, A minor, 8-bar intro (kick and hats), 4-bar build (pad, clap roll, sixteenth hats, acid line under a rising filter), 16-bar drop. 909 drums with 56 % swing on a glued drum bus, offbeat FM organ bass, a 303 line through a saturator and filter, poly pad chords; bass and pad ducked by the kick through sidechained compressors; reverb and delay returns fed by sends; filter-riser automation; a "Master" bus with glue and a -1 dBTP true-peak limiter. |
| `render <dir> -o <wav> [--rate] [--tail] [--normalize <lufs>]` | Offline render to a 32-bit float stereo WAV (RF64 past 4 GiB). |
| `stems <dir> -o <dir> [same options]` | `master.wav` plus one WAV per track or bus that feeds the master, from one pass, so they sum to the master. |
| `play <dir> [--loops] [--volume]` | Plays through the default output device. |
| `meter <file>` | Integrated LUFS, true peak, sample peak. |
| `analyze <file>` | Tempo and first beat, key (with Camelot code), loudness, range, true peak, crest, stereo correlation. |
| `export <dir> -o <file.dawproject>` / `import <file.dawproject> <dir>` | DAWproject 1.0 interchange; prints what could not be carried exactly. |
| `library scan <folder>... [--db]` / `library search [words] [--bpm lo-hi] [--key Am\|8A] [--kind] [--limit] [--db]` | The SQLite sample library (default database in the user data folder, shared with the app). |
| `plugins scan` / `plugins list` | Scans CLAP plugins and Audio Units into the plugin cache that the CLI, agent and app read. |
| `agent serve <dir> [--http [ADDR]] [--no-autosave]` / `agent do <dir> <words...>` / `agent tools` | The MCP server (stdio, or Streamable HTTP on localhost) and plain-language commands. The same commands are in the `fourfloor-agent` binary. |

Errors print `error: ...` on stderr; exit code 2 for bad arguments, 1 for
anything else.

### Desktop app (`fourfloor-app`)

`fourfloor-app [DIR] [--no-audio] [--plugins] [--smoke [N]]`. egui/eframe,
dark theme. Views: arrangement (clips, automation lanes, audio import),
clip launcher, step sequencer, piano roll, mixer (sends, routing, meters),
device dock with knobs generated from parameter descriptors, add-device
browser, master meters (LUFS, true peak, correlation, spectrum) with
reference A/B, DJ decks (2 or 4), sample browser with audition, settings
(audio/MIDI, mappings, models, plugins, assistant) and an AI panel (phrase
commands, a local OpenAI-compatible model, drum/bass generators, stem
separation, audio to MIDI, mix report, MCP server). `--smoke` runs N frames
headless and prints a summary.

## Crate map

| Crate | Purpose |
| --- | --- |
| `fourfloor-core` | The contract: ids, musical time (960 ticks per beat), `AudioMessage`, note/controller `Event`s, `ParamInfo`, the `Device` trait and `DeviceRegistry`. |
| `fourfloor-engine` | Audio graph (audio, sidechain and event edges, delay compensation), transport and loop, lock-free engine, built-in devices (`builtin.*`, `note.arp`, `note.chord`, note/audio-clip players, clip launcher), block-rate automation and modulation, MIDI parsing/mapping/learn/clock (`midi` feature for ports), offline render, cpal output (`cpal` feature). |
| `fourfloor-model` | Edit model: 57 typed, undoable commands with JSON Schemas, command bus with transactions, versioned project folder, swing and grooves, house pattern writer. |
| `fourfloor-audio` | Decode (symphonia), WAV/RF64 read and write, windowed-sinc resampler, EBU R 128 metering, offline pitch-preserving stretch (`stretch` feature, Signalsmith). |
| `fourfloor-compile` | Project to graph through the registry: chains, buses, sends, sidechains, audio clips with warp, launcher scenes, automation, LFOs, macros; render, stems, loudness normalisation. |
| `fourfloor-instruments` | `drum.kick909/kick808/snare909/clap909/hat909/rim909/tom909/cowbell808/kit`, `synth.acid303`, `fm.organ`, `fm.stab`, `poly.pad`, `sampler` (pitched and sliced). |
| `fourfloor-effects` | `fx.eq/isolator/filter/compressor/glue/limiter/pump/delay/reverb/chorus/saturator/utility`. The limiter has an optional 4x true-peak detector. |
| `fourfloor-host` | CLAP hosting (clack-host) and Audio Unit v2 (own AudioToolbox bindings) as `Device`s, a plugin scan cache, a plugin main thread. Kinds `plugin.clap:<id>`, `plugin.au:<type.subtype.manufacturer>`. |
| `fourfloor-analysis` | Onsets, tempo and beat grid, downbeat guess, key (Camelot/Open Key), level meters, spectrum, reference A/B report, feature vectors, waveform overviews. |
| `fourfloor-dj` | 2-4 decks with beat sync, key lock (WSOLA, or Signalsmith with `stretch`), loops, hot cues, EQ/filter, crossfader, headphone cue, recording; 40 JSON commands; offline renderer. |
| `fourfloor-library` | SQLite sample/track library: incremental scan with analysis, search, tags, crates, playlists, similarity, preview plans, a CC0 factory sample set rendered by fourfloor. |
| `fourfloor-ml` | Basic Pitch (audio to MIDI), Spleeter 2-stem separation, LAION CLAP embeddings (`clap` feature), all in-process with tract; DSP fallbacks; humanizer; license-checked model cache. |
| `fourfloor-dawproject` | DAWproject import/export (validated against the official XSDs) and Standard MIDI File read/write. |
| `fourfloor-agent` | MCP server (rmcp; 72 tools), pattern generators, phrase parser, local LLM backends. Binary `fourfloor-agent`. |
| `fourfloor-testkit` | Dev-only: offline render helpers and audio assertions (spectrum, onsets, RMS, envelope, LUFS, true peak, correlation, null tests, allocation guard). |
| `fourfloor-cli` | The `fourfloor` binary. |
| `fourfloor-app` | The `fourfloor-app` desktop binary. |
| `fourfloor-spikes` | Phase 0 spikes kept for reference (truce-rack and clack scans, the original stretch spike). Nothing depends on it. |

The device registry the CLI, agent and project compiler use holds the
engine built-ins, all instruments and effects, and the plugins in the cached
plugin scan. The app adds cached plugins only with `--plugins`.

## Feature status

Graded against the feature matrix in `research_and_tentative_build_plan.md`
by an audit on 2026-10-16. **Works**: implemented and proven by a passing
test that measures rendered audio or behaviour, or by running the CLI/app.
**Partial**: real but incomplete (what is missing is named). **Stub**: API
only. **Missing**: not there.

### Production

| Feature | Grade | Notes |
| --- | --- | --- |
| Low-latency engine | Partial | Lock-free engine, allocation guard on every callback, fixed buffer size selectable in the app. There is no audio input, so the round-trip latency has never been measured. |
| AU + VST3 hosting with delay compensation | Partial | Audio Units load and their latency passes the PDC null test (`fourfloor-host/tests/au_stock.rs`). VST3 is a stub: it is listed but never loaded. |
| CLAP hosting | Works | Fixture plugins built from source; reported latency nulls (`clap_fixtures.rs`). No plugin GUIs. |
| LV2 hosting | Missing | Marked Skip in the plan. |
| Clip launcher + arrangement | Partial | MIDI slots, scenes and quantized launches work. Audio clips in launcher slots are not played. One constant tempo, and no time signature. |
| Step sequencer, swing, grooves | Works | The swing and groove are baked into the notes when they are written. |
| Sampler with slicing and transient detection | Works | The registered `sampler` now detects slices from its file (before this audit, detection ran only in tests). |
| Warping and time-stretch with key lock | Partial | Varispeed warp works. Key-locked warp needs the non-default `stretch` feature. The app has no warp control, and imported or library audio is never warped to the project tempo. Agent `set_clip_warp` and DAWproject can set a warp. DJ key lock (WSOLA) works by default. |
| Piano roll with scale and chord tools | Works | Draw, chord stamp, snap to scale, quantize. It now scrolls to the clip's notes. |
| MIDI effects (arp, chord) | Works | `note.arp`, `note.chord`. |
| Sidechain + pumping | Works | Sidechained compressor and `fx.pump`; the demo's pad ducks to 0.14. |
| Tempo-synced filters and delays | Works | `fx.filter` LFO and `fx.delay` follow the tempo. |
| Macros | Works | |
| Automation + modulation | Works | Block rate (values change once per block). |
| Ableton Link + MIDI clock | Partial | MIDI clock out exists and is tested in `fourfloor-engine`, but no app or CLI path turns it on. Link is missing. |
| MIDI controller mapping | Works | MIDI learn and mappings in the app settings. |

### Sound

| Feature | Grade | Notes |
| --- | --- | --- |
| 909/808 drums | Works | Nine `drum.*` voices. |
| Organ basses and M1-style stabs | Works | `fm.organ`, `fm.stab`. |
| Piano and chord stabs | Partial | Chord stabs come from `fm.stab`. There is no piano voice, and hosted Surge XT or Dexed has not been tried. |
| Juno-style pads | Works | `poly.pad`. |
| 303-style acid | Works | `synth.acid303`: slide, accent, filter envelope. |
| Chopped vocals | Partial | Spleeter stems and the slicing sampler both work, but nothing sends a separated stem straight into a sliced sampler. |
| Effects chain | Works | Twelve `fx.*` devices. The saturator has no oversampling. Hosted reverbs are untested. |

### Mixdown and mastering

| Feature | Grade | Notes |
| --- | --- | --- |
| Buses, sends, groups | Works | |
| Metering (LUFS, true peak, correlation, spectrum) | Works | |
| Reference A/B with loudness matching | Works | |
| Stem and master export | Partial | The master works. Stems split only at what feeds the output, so a project with a Master bus (like `fourfloor demo`) exports one stem, the bus itself. |

### DJ mixing

| Feature | Grade | Notes |
| --- | --- | --- |
| 2 to 4 decks | Works | |
| Beatgrid + BPM analysis | Works | The demo is detected at 124.00 BPM. |
| Key detection + harmonic mixing | Works | "What mixes next" prefers compatible keys. |
| Sync | Works | Kicks land within 0.5 ms of the demo's. |
| EQ + filter per deck | Works | |
| Loops + hot cues | Works | |
| Deck effects | Works | Delay and reverb slots per deck. |
| Library management | Works | SQLite scan, search, tags, crates and playlists. |
| Mix recording | Works | |
| Stem-based mixing | Partial | `fourfloor-dj` mixes per-stem gain and mute (tested), but neither the app nor the CLI loads or separates stems for a deck. |
| Vinyl / DVS | Missing | Marked Skip in the plan. |

### Sample library

| Feature | Grade | Notes |
| --- | --- | --- |
| Browser with BPM, key and tags | Works | App browser and `fourfloor library search`. Files under 60 s count as loops, so the 55 s demo is classed as a loop. |
| Preview in project tempo and key | Works | Key-locked preview uses the DJ WSOLA stretcher. |
| Similarity search | Works | DSP feature vectors by default. LAION CLAP text search needs the app's opt-in `clap-search` feature. There is no bliss-rs and no vector index (brute-force cosine). |

### AI-native features

| Feature | Grade | Notes |
| --- | --- | --- |
| Stem separation | Works | Spleeter 2-stem (vocals and accompaniment) on the real model. There is no 4- or 5-stem model. |
| Audio to MIDI | Works | Basic Pitch recovers all 16 demo bass notes. |
| Sample search by text and similarity | Works | Real CLAP model with `--features clap`. Opt-in in the app. |
| DJ track similarity | Works | Uses its own feature vectors, not bliss-rs. |
| Generative drums and basslines | Partial | The rule-based generators work. GrooVAE is missing; a rule-based humanizer stands in. |
| Mixdown suggestions | Partial | `mix_report` gives rule-based level and spectrum suggestions. Its per-bus view collapses to the Master bus. No Matchering port. No LLM explanation has been run. |
| Natural-language control | Partial | The phrase parser and MCP tools work. No real LLM has been run (mock server only); `mistralrs` only compiles. |

### Platform

| Feature | Grade | Notes |
| --- | --- | --- |
| Agent API (MCP, 72 tools) | Works | stdio and localhost HTTP. `fourfloor agent serve` was deadlocked on stdout until this audit; it is fixed, with a test in `fourfloor-cli/tests/agent_serve.rs`. |
| Project folder (versioned, migrations, audio pool) | Works | |
| DAWproject import/export | Works | The XSD-validated export and re-import of the demo renders bit-identically. |
| egui views | Works | Arrangement, launcher, sequencer, piano roll, mixer, device dock, DJ, sample browser, settings and AI panel. UI tests click through them; the snapshot tests check only that each view renders content, with no golden images. |
| Plugin GUIs, out-of-process hosting | Stub | `fourfloor-host` `gui` and `sandbox` return `Unsupported`. |
| Audio / MIDI recording | Missing | Arm only routes live MIDI. |

## Known gaps

- One constant tempo; no tempo map, no time-signature field (DAWproject
  imports become 4/4).
- A graph rebuild restarts devices with no crossfade; latency is read once
  at compile time; event edges are not delay-compensated.
- No audio or MIDI recording (arm only routes live MIDI).
- Swing and grooves are baked into notes when written.
- `render` and `stems` hold the whole render in memory.
- Analysis is constant-tempo, one key per file; the library classed the
  55 s demo as a loop, not a track.
- The sampler does not band-limit upward transposition; the saturator has
  no oversampling; the limiter's true-peak mode can read up to 0.1 dB from
  a BS.1770 meter.
- MIDI clock out runs on a timer thread; Ableton Link is not implemented.
- VST3, plugin GUIs and sandboxed scanning are stubs.
- There is no CI. `--all-features` (including `stretch`, `mistralrs`,
  ML `clap` and the spike features) builds, passes clippy and passes its
  tests on macOS arm64, last checked 2026-10-16.

## Checks

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

Tests need no audio device, display or installed plugins; those that do
are `#[ignore]`d or feature-gated. Model tests skip with a notice when a
model is not cached; set `FOURFLOOR_REQUIRE_MODELS=1` to make that a
failure, and fetch models with `cargo test -p fourfloor-ml -- --ignored`.

## Real-time rule

No allocation, locks or logging on the audio thread. `Device::prepare` may
allocate; `process`, `set_param` and `reset` may not. Debug builds run the
audio callback under `assert_no_alloc`: the `fourfloor` binary, the
engine, instrument, effect, DJ, app and CLAP-host tests install the guard
allocator, so the whole demo render is checked. Non-test code in the
workspace has no `unwrap`/`expect`/`panic!`.

## Phase 0 spike verdicts (2026-10-11)

- truce-rack 1.1.5 was not used: no latency API, stereo-only wrappers and
  per-block allocation in its CLAP/VST3 process paths. Hosting is built on
  clack-host 0.2 plus hand-written Audio Unit bindings instead.
- signalsmith-stretch 0.1.3 (vendors Signalsmith Stretch 1.3.1) works
  offline; it now lives in `fourfloor-audio` behind `stretch`.
- ebur128 0.1.10 and symphonia 0.6.1 work and live in `fourfloor-audio`;
  symphonia cannot read RF64, so `fourfloor-audio` has its own reader.
