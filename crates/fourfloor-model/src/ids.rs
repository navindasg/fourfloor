//! Ids for objects only the edit model knows: scenes, macros and modulators.
//!
//! They come from the project's one id counter, like track, clip and device
//! ids, so no two objects in a project share a number.

use std::fmt;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

macro_rules! model_id {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
            JsonSchema,
        )]
        #[serde(transparent)]
        pub struct $name(pub u64);

        impl $name {
            /// Wraps a raw id value.
            pub const fn new(raw: u64) -> Self {
                Self(raw)
            }

            /// Returns the raw id value.
            pub const fn get(self) -> u64 {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

model_id!(
    /// Identifies a scene: one row of the clip launcher.
    SceneId
);
model_id!(
    /// Identifies a macro control.
    MacroId
);
model_id!(
    /// Identifies a modulator (an LFO or an envelope).
    ModulatorId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_plain_numbers_on_the_wire() {
        assert_eq!(serde_json::to_string(&SceneId::new(7)).unwrap(), "7");
        assert_eq!(serde_json::from_str::<MacroId>("9").unwrap(), MacroId(9));
        assert_eq!(ModulatorId::new(3).get(), 3);
        assert_eq!(SceneId(4).to_string(), "4");
    }
}
