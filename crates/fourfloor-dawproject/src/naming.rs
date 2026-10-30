//! XML ids written on export, and reading model ids back out of them.
//!
//! `xs:ID` values must be XML names, so every id gets a letter prefix that
//! also says what kind of object it names. Model ids come from one counter,
//! so a prefix plus the number is unique in the document. On import of a
//! file fourfloor wrote itself, the numbers are read back so that the ids in
//! the extension file keep their meaning.

use fourfloor_core::{DeviceId, ParamId, TrackId};
use fourfloor_model::SceneId;

pub(crate) const MASTER_TRACK: &str = "master";
pub(crate) const MASTER_CHANNEL: &str = "chmaster";
pub(crate) const MASTER_VOLUME: &str = "volmaster";
pub(crate) const MASTER_PAN: &str = "panmaster";
pub(crate) const MASTER_MUTE: &str = "mutemaster";
pub(crate) const TEMPO: &str = "tempo";
pub(crate) const TIME_SIGNATURE: &str = "timesig";
pub(crate) const ARRANGEMENT: &str = "arrangement";
pub(crate) const ARRANGEMENT_LANES: &str = "lanes";

pub(crate) fn track(id: TrackId) -> String {
    format!("t{id}")
}
pub(crate) fn channel(id: TrackId) -> String {
    format!("ch{id}")
}
pub(crate) fn volume(id: TrackId) -> String {
    format!("vol{id}")
}
pub(crate) fn pan(id: TrackId) -> String {
    format!("pan{id}")
}
pub(crate) fn mute(id: TrackId) -> String {
    format!("mute{id}")
}
pub(crate) fn send(from: TrackId, to: TrackId) -> String {
    format!("send{from}_{to}")
}
pub(crate) fn send_volume(from: TrackId, to: TrackId) -> String {
    format!("sendvol{from}_{to}")
}
pub(crate) fn device(id: DeviceId) -> String {
    format!("d{id}")
}
pub(crate) fn device_enabled(id: DeviceId) -> String {
    format!("on{id}")
}
pub(crate) fn param(device: DeviceId, param: ParamId) -> String {
    format!("p{device}_{}", param.get())
}
pub(crate) fn track_lanes(id: TrackId) -> String {
    format!("lanes{id}")
}
pub(crate) fn track_clips(id: TrackId) -> String {
    format!("clips{id}")
}
pub(crate) fn notes(clip: u64) -> String {
    format!("notes{clip}")
}
pub(crate) fn audio(clip: u64) -> String {
    format!("audio{clip}")
}
pub(crate) fn warps(clip: u64) -> String {
    format!("warps{clip}")
}
pub(crate) fn param_lane(device: DeviceId, param: ParamId) -> String {
    format!("auto{device}_{}", param.get())
}
pub(crate) fn volume_lane(id: TrackId) -> String {
    format!("autovol{id}")
}
pub(crate) fn pan_lane(id: TrackId) -> String {
    format!("autopan{id}")
}
pub(crate) fn scene(id: SceneId) -> String {
    format!("scene{id}")
}
pub(crate) fn scene_lanes(id: SceneId) -> String {
    format!("scenelanes{id}")
}
pub(crate) fn slot(track: TrackId, scene: SceneId) -> String {
    format!("slot{track}_{scene}")
}

/// The number after `prefix`, when the rest of `id` is all digits.
pub(crate) fn number(id: Option<&str>, prefix: &str) -> Option<u64> {
    let digits = id?.strip_prefix(prefix)?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// A clip id from the id of the timeline inside the clip.
pub(crate) fn clip_number(id: Option<&str>) -> Option<u64> {
    ["notes", "audio", "warps"]
        .into_iter()
        .find_map(|prefix| number(id, prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_come_back_out_of_ids() {
        let track_id = TrackId::new(17);
        assert_eq!(number(Some(&track(track_id)), "t"), Some(17));
        assert_eq!(number(Some("t17x"), "t"), None);
        assert_eq!(number(Some("t"), "t"), None);
        assert_eq!(number(None, "t"), None);
        assert_eq!(clip_number(Some(&warps(9))), Some(9));
        assert_eq!(clip_number(Some(&notes(3))), Some(3));
        assert_eq!(clip_number(Some("id23")), None);
        assert_eq!(param(DeviceId::new(4), ParamId::new(2)), "p4_2");
    }
}
