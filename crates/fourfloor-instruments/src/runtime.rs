//! Shared plumbing for instruments: the sample-accurate event loop, voice
//! allocation and parameter clamping.

use fourfloor_core::{EventKind, ParamId, ParamInfo, ProcessIo};

/// An instrument seen as "handle an event, then make one stereo frame".
pub(crate) trait Synth {
    /// Applies one event. Called before the frame it is stamped with.
    fn handle_event(&mut self, kind: EventKind);
    /// Produces the next frame as (left, right).
    fn tick(&mut self) -> (f32, f32);
}

/// Runs `synth` over one block: events take effect on their exact frame
/// and every frame of every output channel is written. With one output
/// channel the two sides are averaged; channels past the second are
/// silenced.
pub(crate) fn run<S: Synth + ?Sized>(synth: &mut S, io: &mut ProcessIo<'_, '_>) {
    let frames = io.frames();
    let mut events = io.events.iter().peekable();
    for frame in 0..frames {
        while let Some(event) = events.next_if(|event| event.frame as usize <= frame) {
            synth.handle_event(event.kind);
        }
        let (left, right) = synth.tick();
        match io.outputs {
            [] => {}
            [mono] => put(mono, frame, 0.5 * (left + right)),
            [first, second, rest @ ..] => {
                put(first, frame, left);
                put(second, frame, right);
                for channel in rest.iter_mut() {
                    put(channel, frame, 0.0);
                }
            }
        }
    }
    // Events stamped past the block break the contract; apply them late
    // rather than lose a note-off.
    for event in events {
        synth.handle_event(event.kind);
    }
}

fn put(channel: &mut [f32], frame: usize, value: f32) {
    if let Some(slot) = channel.get_mut(frame) {
        *slot = value;
    }
}

/// The value to apply for a parameter change: `None` for an unknown id or
/// a value that is not finite, otherwise the value clamped to its range.
pub(crate) fn clamped(params: &[ParamInfo], id: ParamId, value: f32) -> Option<f32> {
    if !value.is_finite() {
        return None;
    }
    params
        .iter()
        .find(|info| info.id == id)
        .map(|info| info.clamp(value))
}

/// Bookkeeping every polyphonic voice carries, used to match note-offs and
/// to choose which voice a new note takes.
#[derive(Clone, Copy, Debug)]
pub(crate) struct VoiceMeta {
    pub pitch: u8,
    pub channel: u8,
    /// Value of the synth's note counter when the voice started.
    pub started: u64,
    /// The key is still down.
    pub gate: bool,
    /// The voice is making sound.
    pub active: bool,
}

impl VoiceMeta {
    pub(crate) const IDLE: Self = Self {
        pitch: 0,
        channel: 0,
        started: 0,
        gate: false,
        active: false,
    };

    pub(crate) fn matches(&self, pitch: u8, channel: u8) -> bool {
        self.active && self.gate && self.pitch == pitch && self.channel == channel
    }
}

/// Picks the voice a new note should use: an idle one if there is one,
/// otherwise the oldest released voice, otherwise the oldest voice.
pub(crate) fn pick_voice<'a>(metas: impl Iterator<Item = &'a VoiceMeta>) -> usize {
    let mut best = 0;
    // Lower is better: (class, started). Class 0 idle, 1 released, 2 held.
    let mut best_key = (u8::MAX, u64::MAX);
    for (index, meta) in metas.enumerate() {
        let class = match (meta.active, meta.gate) {
            (false, _) => 0,
            (true, false) => 1,
            (true, true) => 2,
        };
        let key = (class, if class == 0 { 0 } else { meta.started });
        if key < best_key {
            best_key = key;
            best = index;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(active: bool, gate: bool, started: u64) -> VoiceMeta {
        VoiceMeta {
            active,
            gate,
            started,
            ..VoiceMeta::IDLE
        }
    }

    #[test]
    fn picks_idle_then_oldest_released_then_oldest() {
        let voices = [
            meta(true, true, 1),
            meta(false, false, 9),
            meta(true, false, 2),
        ];
        assert_eq!(pick_voice(voices.iter()), 1);
        let voices = [
            meta(true, true, 1),
            meta(true, false, 5),
            meta(true, false, 2),
        ];
        assert_eq!(pick_voice(voices.iter()), 2);
        let voices = [
            meta(true, true, 4),
            meta(true, true, 3),
            meta(true, true, 7),
        ];
        assert_eq!(pick_voice(voices.iter()), 1);
    }

    #[test]
    fn clamped_rejects_unknown_ids_and_non_finite_values() {
        use fourfloor_core::ParamUnit;
        let params = [ParamInfo::new(
            ParamId(3),
            "X",
            0.0,
            1.0,
            0.5,
            ParamUnit::Generic,
        )];
        assert_eq!(clamped(&params, ParamId(3), 2.0), Some(1.0));
        assert_eq!(clamped(&params, ParamId(4), 0.5), None);
        assert_eq!(clamped(&params, ParamId(3), f32::NAN), None);
        assert_eq!(clamped(&params, ParamId(3), f32::INFINITY), None);
    }
}
