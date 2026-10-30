//! Standard MIDI Files: export of MIDI clips and tracks, import of notes
//! into a clip.
//!
//! Files are written at 960 pulses per quarter note, fourfloor's own tick
//! rate, so note times survive a round trip exactly. Reading accepts
//! format 0 and 1 files at any metrical resolution, with running status;
//! SMPTE time division is refused. Only notes, tempo, time signature and
//! track names are read; other events are skipped.

use fourfloor_core::{ClipId, Tempo, Ticks, TICKS_PER_BEAT};
use fourfloor_model::{Clip, ClipContent, MidiNote, Track};

/// Pulses per quarter note of written files.
pub const PPQ: u16 = 960;

const HEADER: &[u8; 4] = b"MThd";
const TRACK: &[u8; 4] = b"MTrk";
/// One 4/4 bar in ticks: imported clips are rounded up to whole bars.
const BAR: i64 = TICKS_PER_BEAT * 4;

/// Why a Standard MIDI File could not be read or written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum SmfError {
    /// The data does not start with an `MThd` chunk.
    #[error("not a Standard MIDI File")]
    NotMidi,
    /// The data ends inside a chunk or an event.
    #[error("the file is truncated at byte {0}")]
    Truncated(usize),
    /// Format 2 (independent sequences) or an unknown format.
    #[error("MIDI file format {0} is not supported")]
    UnsupportedFormat(u16),
    /// The file counts time in SMPTE frames, not in beats.
    #[error("SMPTE time division is not supported")]
    SmpteTiming,
    /// A malformed event.
    #[error("malformed MIDI data at byte {0}")]
    Malformed(usize),
    /// The clip holds audio, not notes.
    #[error("an audio clip has no notes to write")]
    NotMidiClip,
}

/// One track of a read file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SmfTrack {
    /// The track name meta event, if any.
    pub name: Option<String>,
    /// Notes in fourfloor ticks from the start of the file, sorted by start
    /// then pitch.
    pub notes: Vec<MidiNote>,
    /// MIDI channels the notes were on (bit `n` set for channel `n`).
    pub channels: u16,
}

/// A read Standard MIDI File.
#[derive(Clone, Debug, PartialEq)]
pub struct Smf {
    /// File format, 0 or 1.
    pub format: u16,
    /// Pulses per quarter note of the file.
    pub ppq: u16,
    /// The first tempo, if the file sets one.
    pub tempo: Option<Tempo>,
    /// How many tempo events the file has.
    pub tempo_events: usize,
    /// The first time signature, as (numerator, denominator).
    pub time_signature: Option<(u8, u8)>,
    /// The tracks, in file order.
    pub tracks: Vec<SmfTrack>,
}

impl Smf {
    /// All notes of all tracks, sorted.
    pub fn notes(&self) -> Vec<MidiNote> {
        let mut notes: Vec<MidiNote> = self
            .tracks
            .iter()
            .flat_map(|track| track.notes.iter().copied())
            .collect();
        notes.sort_by_key(|note| (note.start, note.pitch));
        notes
    }

    /// A MIDI clip holding every note of the file, starting at the
    /// project start, as long as the notes rounded up to whole 4/4 bars.
    pub fn to_clip(&self, id: ClipId, name: impl Into<String>) -> Clip {
        let notes = self.notes();
        let end = notes
            .iter()
            .map(|note| (note.start + note.length).get())
            .max()
            .unwrap_or(0);
        let length = ((end + BAR - 1) / BAR).max(1) * BAR;
        Clip {
            id,
            name: name.into(),
            start: Ticks::ZERO,
            length: Ticks::new(length),
            offset: Ticks::ZERO,
            loop_length: None,
            content: ClipContent::Midi { notes },
        }
    }
}

fn write_vlq(out: &mut Vec<u8>, value: u32) {
    let mut buffer = [0u8; 5];
    let mut index = buffer.len();
    let mut rest = value;
    loop {
        index -= 1;
        buffer[index] = (rest & 0x7f) as u8;
        if index < buffer.len() - 1 {
            buffer[index] |= 0x80;
        }
        rest >>= 7;
        if rest == 0 {
            break;
        }
    }
    out.extend_from_slice(&buffer[index..]);
}

/// A note event to write: (tick, order, bytes). Offs sort before ons.
type Event = (i64, u8, [u8; 3]);

fn track_chunk(name: Option<&str>, tempo: Option<Tempo>, notes: &[MidiNote]) -> Vec<u8> {
    let mut body = Vec::new();
    if let Some(name) = name {
        body.extend_from_slice(&[0x00, 0xff, 0x03]);
        let bytes = name.as_bytes();
        write_vlq(&mut body, bytes.len().min(0x0fff_ffff) as u32);
        body.extend_from_slice(bytes);
    }
    if let Some(tempo) = tempo {
        let micros = (60_000_000.0 / tempo.bpm())
            .round()
            .clamp(1.0, 16_777_215.0) as u32;
        body.extend_from_slice(&[0x00, 0xff, 0x51, 0x03]);
        body.extend_from_slice(&micros.to_be_bytes()[1..]);
        body.extend_from_slice(&[0x00, 0xff, 0x58, 0x04, 4, 2, 24, 8]);
    }
    let mut events: Vec<Event> = notes
        .iter()
        .filter(|note| note.start >= Ticks::ZERO && note.length > Ticks::ZERO)
        .flat_map(|note| {
            let pitch = note.pitch.min(127);
            let velocity = note.velocity.clamp(1, 127);
            [
                (note.start.get(), 1, [0x90, pitch, velocity]),
                ((note.start + note.length).get(), 0, [0x80, pitch, 0]),
            ]
        })
        .collect();
    events.sort_by_key(|(tick, order, bytes)| (*tick, *order, bytes[1]));
    let mut last = 0i64;
    for (tick, _, bytes) in events {
        let delta = u32::try_from(tick - last)
            .unwrap_or(u32::MAX)
            .min(0x0fff_ffff);
        write_vlq(&mut body, delta);
        body.extend_from_slice(&bytes);
        last = tick;
    }
    body.extend_from_slice(&[0x00, 0xff, 0x2f, 0x00]);
    let mut chunk = Vec::with_capacity(body.len() + 8);
    chunk.extend_from_slice(TRACK);
    chunk.extend_from_slice(&(body.len() as u32).to_be_bytes());
    chunk.extend_from_slice(&body);
    chunk
}

fn header(format: u16, tracks: u16) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(HEADER);
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&format.to_be_bytes());
    out.extend_from_slice(&tracks.to_be_bytes());
    out.extend_from_slice(&PPQ.to_be_bytes());
    out
}

/// A format 0 file holding `notes` (ticks from the file start), with the
/// tempo, a 4/4 time signature and a track name.
pub fn write_notes(notes: &[MidiNote], tempo: Tempo, name: &str) -> Vec<u8> {
    let mut out = header(0, 1);
    out.extend(track_chunk(Some(name), Some(tempo), notes));
    out
}

/// A format 0 file of a MIDI clip as it sounds: its offset and loop are
/// applied and notes are cut at the clip end. Times start at the clip start.
pub fn clip_to_smf(clip: &Clip, tempo: Tempo) -> Result<Vec<u8>, SmfError> {
    if !clip.is_midi() {
        return Err(SmfError::NotMidiClip);
    }
    Ok(write_notes(&clip.sounding_notes(), tempo, &clip.name))
}

/// A format 1 file of a MIDI track's arrangement: a tempo track, then one
/// track with every timeline clip's sounding notes at its timeline position.
pub fn track_to_smf(track: &Track, tempo: Tempo) -> Vec<u8> {
    let mut notes: Vec<MidiNote> = track
        .clips
        .iter()
        .filter(|clip| clip.is_midi())
        .flat_map(|clip| {
            clip.sounding_notes().into_iter().map(|note| MidiNote {
                start: note.start + clip.start,
                ..note
            })
        })
        .collect();
    notes.sort_by_key(|note| (note.start, note.pitch));
    let mut out = header(1, 2);
    out.extend(track_chunk(None, Some(tempo), &[]));
    out.extend(track_chunk(Some(&track.name), None, &notes));
    out
}

struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl Reader<'_> {
    fn byte(&mut self) -> Result<u8, SmfError> {
        let byte = *self
            .bytes
            .get(self.at)
            .ok_or(SmfError::Truncated(self.at))?;
        self.at += 1;
        Ok(byte)
    }

    fn take(&mut self, count: usize) -> Result<&[u8], SmfError> {
        let end = self
            .at
            .checked_add(count)
            .ok_or(SmfError::Truncated(self.at))?;
        let slice = self
            .bytes
            .get(self.at..end)
            .ok_or(SmfError::Truncated(self.at))?;
        self.at = end;
        Ok(slice)
    }

    fn vlq(&mut self) -> Result<u32, SmfError> {
        let mut value = 0u32;
        for _ in 0..4 {
            let byte = self.byte()?;
            value = (value << 7) | u32::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(SmfError::Malformed(self.at))
    }

    fn u32_be(&mut self) -> Result<u32, SmfError> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn u16_be(&mut self) -> Result<u16, SmfError> {
        let b = self.take(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
}

/// What a track adds to the whole file besides its notes.
#[derive(Default)]
struct TrackMeta {
    tempos: Vec<u32>,
    signature: Option<(u8, u8)>,
}

fn to_ticks(pulses: u64, ppq: u16) -> Ticks {
    let ticks = (pulses as f64 * TICKS_PER_BEAT as f64 / f64::from(ppq)).round();
    Ticks::new(ticks.min(fourfloor_model::validate::MAX_TICKS as f64) as i64)
}

fn read_track(body: &[u8], offset: usize, ppq: u16) -> Result<(SmfTrack, TrackMeta), SmfError> {
    let mut reader = Reader { bytes: body, at: 0 };
    let mut track = SmfTrack::default();
    let mut meta = TrackMeta::default();
    let mut now: u64 = 0;
    let mut status = 0u8;
    // Open notes per (channel, pitch): start pulse and velocity, oldest first.
    let mut open: Vec<Vec<(u64, u8)>> = vec![Vec::new(); 16 * 128];
    let finish = |open: &mut Vec<(u64, u8)>, pitch: u8, end: u64, track: &mut SmfTrack| {
        if open.is_empty() {
            return;
        }
        let (start, velocity) = open.remove(0);
        let start_ticks = to_ticks(start, ppq);
        track.notes.push(MidiNote {
            start: start_ticks,
            length: (to_ticks(end, ppq) - start_ticks).max(Ticks::new(1)),
            pitch,
            velocity,
        });
    };
    while reader.at < body.len() {
        now += u64::from(reader.vlq().map_err(|e| shift(e, offset))?);
        let first = reader.byte().map_err(|e| shift(e, offset))?;
        let (kind, data0) = if first & 0x80 != 0 {
            (first, None)
        } else if status != 0 {
            (status, Some(first))
        } else {
            return Err(SmfError::Malformed(offset + reader.at));
        };
        match kind {
            0xff => {
                let meta_type = reader.byte().map_err(|e| shift(e, offset))?;
                let length = reader.vlq().map_err(|e| shift(e, offset))? as usize;
                let data = reader.take(length).map_err(|e| shift(e, offset))?;
                match meta_type {
                    0x03 if track.name.is_none() => {
                        track.name = Some(String::from_utf8_lossy(data).into_owned());
                    }
                    0x51 if data.len() == 3 => {
                        meta.tempos
                            .push(u32::from_be_bytes([0, data[0], data[1], data[2]]));
                    }
                    0x58 if data.len() >= 2 && meta.signature.is_none() => {
                        meta.signature =
                            Some((data[0], 1u8.checked_shl(u32::from(data[1])).unwrap_or(0)));
                    }
                    0x2f => break,
                    _ => {}
                }
            }
            0xf0 | 0xf7 => {
                let length = reader.vlq().map_err(|e| shift(e, offset))? as usize;
                reader.take(length).map_err(|e| shift(e, offset))?;
            }
            0x80..=0xef => {
                status = kind;
                let a = match data0 {
                    Some(a) => a,
                    None => reader.byte().map_err(|e| shift(e, offset))?,
                };
                let two = !matches!(kind & 0xf0, 0xc0 | 0xd0);
                let b = if two {
                    reader.byte().map_err(|e| shift(e, offset))?
                } else {
                    0
                };
                if a > 127 || b > 127 {
                    return Err(SmfError::Malformed(offset + reader.at));
                }
                let channel = usize::from(kind & 0x0f);
                let slot = channel * 128 + usize::from(a);
                match kind & 0xf0 {
                    0x90 if b > 0 => {
                        open[slot].push((now, b));
                        track.channels |= 1 << channel;
                    }
                    0x80 | 0x90 => finish(&mut open[slot], a, now, &mut track),
                    _ => {}
                }
            }
            _ => return Err(SmfError::Malformed(offset + reader.at)),
        }
    }
    for (slot, notes) in open.iter_mut().enumerate() {
        while !notes.is_empty() {
            finish(notes, (slot % 128) as u8, now, &mut track);
        }
    }
    track.notes.sort_by_key(|note| (note.start, note.pitch));
    Ok((track, meta))
}

fn shift(error: SmfError, offset: usize) -> SmfError {
    match error {
        SmfError::Truncated(at) => SmfError::Truncated(at + offset),
        SmfError::Malformed(at) => SmfError::Malformed(at + offset),
        other => other,
    }
}

/// Reads a Standard MIDI File.
pub fn read_smf(bytes: &[u8]) -> Result<Smf, SmfError> {
    let mut reader = Reader { bytes, at: 0 };
    if reader.take(4).ok() != Some(&HEADER[..]) {
        return Err(SmfError::NotMidi);
    }
    let header_length = reader.u32_be()? as usize;
    if header_length < 6 {
        return Err(SmfError::Malformed(4));
    }
    let format = reader.u16_be()?;
    let count = reader.u16_be()?;
    let division = reader.u16_be()?;
    reader.take(header_length - 6)?;
    if format > 1 {
        return Err(SmfError::UnsupportedFormat(format));
    }
    if division & 0x8000 != 0 {
        return Err(SmfError::SmpteTiming);
    }
    if division == 0 {
        return Err(SmfError::Malformed(12));
    }
    let mut tracks = Vec::new();
    let mut tempos = Vec::new();
    let mut signature = None;
    while tracks.len() < usize::from(count) && reader.at < bytes.len() {
        let kind: [u8; 4] = reader
            .take(4)?
            .try_into()
            .map_err(|_| SmfError::Malformed(reader.at))?;
        let length = reader.u32_be()? as usize;
        let start = reader.at;
        let body = reader.take(length)?;
        if &kind != TRACK {
            continue;
        }
        let (track, meta) = read_track(body, start, division)?;
        tempos.extend(meta.tempos);
        signature = signature.or(meta.signature);
        tracks.push(track);
    }
    if tracks.len() < usize::from(count) {
        // The header promises more tracks than the data holds.
        return Err(SmfError::Truncated(bytes.len()));
    }
    let tempo = tempos.first().and_then(|micros| {
        let bpm = 60_000_000.0 / f64::from(*micros);
        let snapped = (bpm * 1_000.0).round() / 1_000.0;
        let bpm = if (snapped - bpm).abs() < 1e-4 {
            snapped
        } else {
            bpm
        };
        Tempo::new(bpm).ok()
    });
    Ok(Smf {
        format,
        ppq: division,
        tempo,
        tempo_events: tempos.len(),
        time_signature: signature,
        tracks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn note(start: i64, length: i64, pitch: u8, velocity: u8) -> MidiNote {
        MidiNote {
            start: Ticks::new(start),
            length: Ticks::new(length),
            pitch,
            velocity,
        }
    }

    #[test]
    fn variable_length_quantities_match_the_spec_examples() {
        for (value, bytes) in [
            (0u32, &[0x00][..]),
            (0x40, &[0x40]),
            (0x7f, &[0x7f]),
            (0x80, &[0x81, 0x00]),
            (0x2000, &[0xc0, 0x00]),
            (0x3fff, &[0xff, 0x7f]),
            (0x0fff_ffff, &[0xff, 0xff, 0xff, 0x7f]),
        ] {
            let mut out = Vec::new();
            write_vlq(&mut out, value);
            assert_eq!(out, bytes, "{value:#x}");
            assert_eq!(Reader { bytes, at: 0 }.vlq().unwrap(), value);
        }
    }

    #[test]
    fn notes_tempo_and_name_round_trip_exactly() {
        let notes = vec![
            note(0, 240, 36, 127),
            note(0, 960, 60, 1),
            note(480, 960, 60, 90),
            note(1_000, 7, 127, 64),
            note(3_839, 1, 0, 100),
        ];
        let tempo = Tempo::new(124.0).unwrap();
        let bytes = write_notes(&notes, tempo, "Bass line");
        let smf = read_smf(&bytes).unwrap();
        assert_eq!(smf.format, 0);
        assert_eq!(smf.ppq, PPQ);
        assert_eq!(smf.tempo, Some(tempo));
        assert_eq!(smf.time_signature, Some((4, 4)));
        assert_eq!(smf.tracks.len(), 1);
        assert_eq!(smf.tracks[0].name.as_deref(), Some("Bass line"));
        assert_eq!(smf.notes(), notes);
    }

    #[test]
    fn running_status_other_resolutions_and_velocity_zero_offs_are_read() {
        // Format 1, 96 PPQ: a tempo track, then a track using running
        // status and note-on velocity 0 as note-off, plus a program change
        // and a sysex to skip.
        let mut file = header(1, 2);
        file[12..14].copy_from_slice(&96u16.to_be_bytes());
        file.extend(track_chunk(None, Some(Tempo::new(90.0).unwrap()), &[]));
        let body = [
            0x00, 0xc0, 0x05, // program change, one data byte
            0x00, 0xf0, 0x02, 0x7e, 0xf7, // sysex
            0x00, 0x91, 0x3c, 0x64, // note on ch 1
            0x30, 0x3c, 0x00, // running status: velocity 0 = off, 48 pulses later
            0x00, 0x3e, 0x50, // running status note on
            0x60, 0x81, 0x3e, 0x00, // explicit off, 96 pulses later
            0x00, 0xff, 0x2f, 0x00,
        ];
        file.extend_from_slice(TRACK);
        file.extend_from_slice(&(body.len() as u32).to_be_bytes());
        file.extend_from_slice(&body);
        let smf = read_smf(&file).unwrap();
        assert_eq!(smf.ppq, 96);
        assert_eq!(smf.tempo, Some(Tempo::new(90.0).unwrap()));
        assert_eq!(smf.tracks[1].channels, 0b10);
        assert_eq!(
            smf.notes(),
            vec![note(0, 480, 60, 100), note(480, 960, 62, 80)]
        );
        let clip = smf.to_clip(ClipId::new(5), "Imported");
        assert_eq!(clip.length, Ticks::new(BAR));
        assert_eq!(clip.notes().len(), 2);
    }

    #[test]
    fn bad_files_are_errors_not_panics() {
        assert_eq!(read_smf(b"RIFF....").unwrap_err(), SmfError::NotMidi);
        let good = write_notes(&[note(0, 960, 60, 100)], Tempo::DEFAULT, "x");
        for cut in 0..good.len() {
            let _ = read_smf(&good[..cut]);
        }
        let mut smpte = good.clone();
        smpte[12] = 0xe7;
        assert_eq!(read_smf(&smpte).unwrap_err(), SmfError::SmpteTiming);
        let mut format2 = good;
        format2[9] = 2;
        assert_eq!(
            read_smf(&format2).unwrap_err(),
            SmfError::UnsupportedFormat(2)
        );
    }
}
