//! Stable id newtypes.
//!
//! Ids are plain integers wrapped so they cannot be mixed up. They are
//! assigned by the edit model, never reused within a project, and survive
//! save/load. All are `Copy` and safe to pass to the audio thread.

use core::fmt;

macro_rules! id_newtype {
    ($(#[$meta:meta])* $name:ident, $repr:ty) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[cfg_attr(feature = "serde", serde(transparent))]
        pub struct $name(pub $repr);

        impl $name {
            /// Wraps a raw id value.
            pub const fn new(raw: $repr) -> Self {
                Self(raw)
            }

            /// Returns the raw id value.
            pub const fn get(self) -> $repr {
                self.0
            }
        }

        impl From<$repr> for $name {
            fn from(raw: $repr) -> Self {
                Self(raw)
            }
        }

        impl From<$name> for $repr {
            fn from(id: $name) -> Self {
                id.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_newtype!(
    /// Identifies a track within a project.
    TrackId,
    u64
);
id_newtype!(
    /// Identifies a clip (audio or MIDI) within a project.
    ClipId,
    u64
);
id_newtype!(
    /// Identifies a device (instrument, effect or hosted plugin) within a
    /// project. Unique across all tracks.
    DeviceId,
    u64
);
id_newtype!(
    /// Identifies a parameter within one device. Only unique together with
    /// a [`DeviceId`]. `u32` matches CLAP and VST3 parameter ids.
    ParamId,
    u32
);

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn round_trips_raw_value() {
        assert_eq!(TrackId::new(7).get(), 7);
        assert_eq!(u64::from(ClipId::from(9)), 9);
        assert_eq!(ParamId::new(3).get(), 3u32);
        assert_eq!(DeviceId(11), DeviceId::new(11));
    }

    #[test]
    fn ids_are_ordered_and_hashable() {
        assert!(TrackId(1) < TrackId(2));
        let set: HashSet<DeviceId> = [DeviceId(1), DeviceId(1), DeviceId(2)]
            .into_iter()
            .collect();
        assert_eq!(set.len(), 2);
    }

    #[test]
    fn display_is_the_raw_number() {
        assert_eq!(TrackId(42).to_string(), "42");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_is_transparent() {
        assert_eq!(serde_json::to_string(&TrackId(5)).unwrap(), "5");
        assert_eq!(serde_json::from_str::<ParamId>("8").unwrap(), ParamId(8));
    }
}
