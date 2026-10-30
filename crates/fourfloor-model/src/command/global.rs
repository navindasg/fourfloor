//! Project-wide commands: tempo, swing and the project name.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{Applied, Apply, Outcome};
use crate::error::ModelError;
use crate::project::{Groove, Project, SwingGrid};
use crate::validate;

/// Set the project tempo.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetTempo {
    /// Tempo in beats per minute, 20 to 999.
    pub bpm: f64,
}

impl Apply for SetTempo {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        let tempo = validate::tempo(self.bpm)?;
        Ok((
            Project {
                tempo,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}

/// Set the project's swing amount, used when patterns are written.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SetSwing {
    /// Swing from 50 (straight) to 75 percent; about 66.7 is triplet feel.
    pub percent: f64,
    /// Step size to swing. Omit to keep the current grid.
    #[serde(default)]
    pub grid: Option<SwingGrid>,
}

impl Apply for SetSwing {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::swing(self.percent)?;
        let groove = Groove {
            swing_percent: self.percent,
            grid: self.grid.unwrap_or(project.groove.grid),
        };
        Ok((
            Project {
                groove,
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}

/// Rename the project.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameProject {
    /// The new name: 1 to 128 characters.
    pub name: String,
}

impl Apply for RenameProject {
    fn apply(&self, project: &Project) -> Result<Applied, ModelError> {
        validate::name("project name", &self.name)?;
        Ok((
            Project {
                name: self.name.clone(),
                ..project.clone()
            },
            Outcome::Done,
        ))
    }
}
