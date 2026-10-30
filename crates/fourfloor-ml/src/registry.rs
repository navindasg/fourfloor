//! The model registry: every model fourfloor may run, with where it comes
//! from, what it must hash to, and the licenses of its code and weights.
//!
//! [`ModelRegistry::register`] refuses any entry whose code or weights
//! license is not on the allow-list in [`crate::license`], so a
//! non-commercial model cannot be registered, and
//! [`crate::cache::ModelCache::ensure`] checks again before it touches the
//! network or the disk.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{LicenseField, MlError};
use crate::license;

/// One downloadable file of a model.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    /// File name inside the model's cache directory (no path separators).
    pub name: String,
    /// Where to download it from. `https://` or, for local mirrors,
    /// `file://`.
    pub url: String,
    /// SHA-256 of the file, 64 lowercase hex characters.
    pub sha256: String,
    /// Size in bytes.
    pub size: u64,
}

impl ModelFile {
    /// A file entry.
    pub fn new(name: &str, url: &str, sha256: &str, size: u64) -> Self {
        Self {
            name: name.to_owned(),
            url: url.to_owned(),
            sha256: sha256.to_owned(),
            size,
        }
    }
}

/// A model: its files and its licenses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelSpec {
    /// Registry name: lowercase letters, digits, `.`, `-` and `_`.
    pub name: String,
    /// What the model does, for the UI.
    pub description: String,
    /// Project page of the model.
    pub homepage: String,
    /// SPDX identifier of the license of the model's code.
    pub code_license: String,
    /// SPDX identifier of the license of the trained weights.
    pub weights_license: String,
    /// The files to download.
    pub files: Vec<ModelFile>,
}

impl ModelSpec {
    /// Total download size in bytes.
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }

    /// The file called `name`, if the model has one.
    pub fn file(&self, name: &str) -> Option<&ModelFile> {
        self.files.iter().find(|file| file.name == name)
    }

    /// Checks both licenses against the allow-list.
    pub fn check_license(&self) -> Result<(), MlError> {
        for (field, value) in [
            (LicenseField::Code, &self.code_license),
            (LicenseField::Weights, &self.weights_license),
        ] {
            if !license::is_allowed(value) {
                return Err(MlError::LicenseRefused {
                    model: self.name.clone(),
                    field,
                    license: value.clone(),
                });
            }
        }
        Ok(())
    }

    /// Checks the licenses and that the entry is well formed.
    pub fn validate(&self) -> Result<(), MlError> {
        self.check_license()?;
        let invalid = |reason: String| MlError::InvalidEntry {
            model: self.name.clone(),
            reason,
        };
        if !is_safe_name(&self.name) {
            return Err(invalid(
                "name must be lowercase a-z, 0-9, `.`, `-`, `_`".into(),
            ));
        }
        if self.files.is_empty() {
            return Err(invalid("no files".into()));
        }
        for (index, file) in self.files.iter().enumerate() {
            if !is_safe_name(&file.name) {
                return Err(invalid(format!(
                    "file name `{}` is not a plain name",
                    file.name
                )));
            }
            if self.files[..index]
                .iter()
                .any(|other| other.name == file.name)
            {
                return Err(invalid(format!("file `{}` is listed twice", file.name)));
            }
            if !(file.url.starts_with("https://") || file.url.starts_with("file://")) {
                return Err(invalid(format!(
                    "`{}` must be an https:// or file:// URL",
                    file.url
                )));
            }
            let hex = file.sha256.len() == 64
                && file
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
            if !hex {
                return Err(invalid(format!(
                    "sha256 of `{}` must be 64 lowercase hex characters",
                    file.name
                )));
            }
            if file.size == 0 {
                return Err(invalid(format!("size of `{}` is zero", file.name)));
            }
        }
        Ok(())
    }
}

/// A name that is safe to use as one path component.
fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 96
        && !name.starts_with('.')
        && name.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"._-".contains(&byte)
        })
}

/// Name of the Basic Pitch audio-to-MIDI model.
pub const BASIC_PITCH: &str = "basic-pitch";
/// Name of the Spleeter two-stem separation model.
pub const SPLEETER_2STEMS: &str = "spleeter-2stems";
/// Name of the LAION CLAP embedding model.
pub const CLAP_GENERAL: &str = "clap-larger-general";

/// The set of models fourfloor knows.
#[derive(Clone, Debug, Default)]
pub struct ModelRegistry {
    models: BTreeMap<String, ModelSpec>,
}

impl ModelRegistry {
    /// An empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// The models fourfloor ships support for.
    pub fn builtin() -> Self {
        let mut registry = Self::new();
        for spec in builtin_specs() {
            // The built-in table is checked by a unit test, so a failure
            // here cannot happen in a released build; an entry that somehow
            // failed is left out rather than loaded.
            let _ = registry.register(spec);
        }
        registry
    }

    /// Adds a model. Refuses a license that is not on the allow-list, a
    /// malformed entry and a duplicate name.
    pub fn register(&mut self, spec: ModelSpec) -> Result<(), MlError> {
        spec.validate()?;
        if self.models.contains_key(&spec.name) {
            return Err(MlError::DuplicateModel(spec.name));
        }
        self.models.insert(spec.name.clone(), spec);
        Ok(())
    }

    /// Adds every model of a JSON array of entries, stopping at the first
    /// one that is refused.
    pub fn register_json(&mut self, json: &str) -> Result<usize, MlError> {
        let specs: Vec<ModelSpec> =
            serde_json::from_str(json).map_err(|error| MlError::InvalidEntry {
                model: "<json>".to_owned(),
                reason: error.to_string(),
            })?;
        let count = specs.len();
        for spec in specs {
            self.register(spec)?;
        }
        Ok(count)
    }

    /// The model called `name`.
    pub fn get(&self, name: &str) -> Result<&ModelSpec, MlError> {
        self.models
            .get(name)
            .ok_or_else(|| MlError::UnknownModel(name.to_owned()))
    }

    /// Whether a model called `name` is registered.
    pub fn contains(&self, name: &str) -> bool {
        self.models.contains_key(name)
    }

    /// Every model, sorted by name.
    pub fn iter(&self) -> impl Iterator<Item = &ModelSpec> {
        self.models.values()
    }

    /// Number of models.
    pub fn len(&self) -> usize {
        self.models.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Download size of every model together, in bytes.
    pub fn total_size(&self) -> u64 {
        self.models.values().map(ModelSpec::total_size).sum()
    }
}

const BASIC_PITCH_REV: &str = "fa5997af0a8210982619003269994a1be25eddf3";
const SPLEETER_REV: &str = "7001ba316a615cacddb3f9ef3ec416661a277e26";
const CLAP_REV: &str = "d78c3994912c441f0151a5cc44d86f6b7bf4c861";

fn builtin_specs() -> Vec<ModelSpec> {
    let spleeter = |file: &str| {
        format!(
            "https://huggingface.co/csukuangfj/sherpa-onnx-spleeter-2stems/resolve/{SPLEETER_REV}/{file}"
        )
    };
    let clap = |file: &str| {
        format!("https://huggingface.co/Xenova/larger_clap_general/resolve/{CLAP_REV}/{file}")
    };
    vec![
        ModelSpec {
            name: BASIC_PITCH.to_owned(),
            description: "Spotify Basic Pitch (ICASSP 2022): polyphonic audio to MIDI notes".to_owned(),
            homepage: "https://github.com/spotify/basic-pitch".to_owned(),
            code_license: "Apache-2.0".to_owned(),
            weights_license: "Apache-2.0".to_owned(),
            files: vec![ModelFile::new(
                "nmp.onnx",
                &format!(
                    "https://raw.githubusercontent.com/spotify/basic-pitch/{BASIC_PITCH_REV}/basic_pitch/saved_models/icassp_2022/nmp.onnx"
                ),
                "2c3c1d144bfa61ad236e92e169c13535c880469a12a047d4e73451f2c059a0ec",
                230_444,
            )],
        },
        ModelSpec {
            name: SPLEETER_2STEMS.to_owned(),
            description: "Deezer Spleeter 2 stems (vocals / accompaniment), ONNX export published by the sherpa-onnx project".to_owned(),
            homepage: "https://github.com/deezer/spleeter".to_owned(),
            code_license: "MIT".to_owned(),
            weights_license: "MIT".to_owned(),
            files: vec![
                ModelFile::new(
                    "vocals.onnx",
                    &spleeter("vocals.onnx"),
                    "bdc16ab6bf6117ddd4842c19e80e40e2be188fc555295064d424616b0224ac97",
                    39_318_336,
                ),
                ModelFile::new(
                    "accompaniment.onnx",
                    &spleeter("accompaniment.onnx"),
                    "671ace17acd3720674a2bc14de32ac6292453dec20d9eb0ba4255d4ad8e3d8c0",
                    39_318_343,
                ),
            ],
        },
        ModelSpec {
            name: CLAP_GENERAL.to_owned(),
            description: "LAION CLAP larger_clap_general: joint text and audio embeddings (8-bit quantized ONNX export by Xenova)".to_owned(),
            homepage: "https://huggingface.co/laion/larger_clap_general".to_owned(),
            code_license: "Apache-2.0".to_owned(),
            weights_license: "Apache-2.0".to_owned(),
            files: vec![
                ModelFile::new(
                    "audio_model_quantized.onnx",
                    &clap("onnx/audio_model_quantized.onnx"),
                    "320b831d0043d36aa8cfad0f36b64b7c597fa9ff01a5a9b505e37632923cf9ac",
                    78_155_433,
                ),
                ModelFile::new(
                    "text_model_quantized.onnx",
                    &clap("onnx/text_model_quantized.onnx"),
                    "045614ce697750cf8876cb965ebfad3ef2d0ae7900dda897d7569f7be86521cd",
                    126_603_263,
                ),
                ModelFile::new(
                    "tokenizer.json",
                    &clap("tokenizer.json"),
                    "dc239041d98de27ffc3975473a1a23e3db4c937b23c138c38bbc66588bd247e5",
                    2_108_774,
                ),
            ],
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(name: &str, code: &str, weights: &str) -> ModelSpec {
        ModelSpec {
            name: name.to_owned(),
            description: String::new(),
            homepage: String::new(),
            code_license: code.to_owned(),
            weights_license: weights.to_owned(),
            files: vec![ModelFile::new(
                "model.onnx",
                "https://example.org/model.onnx",
                &"ab".repeat(32),
                10,
            )],
        }
    }

    #[test]
    fn builtin_entries_are_valid_and_under_one_gigabyte() {
        let specs = builtin_specs();
        for spec in &specs {
            spec.validate().unwrap_or_else(|error| panic!("{error}"));
        }
        let registry = ModelRegistry::builtin();
        assert_eq!(registry.len(), specs.len());
        for name in [BASIC_PITCH, SPLEETER_2STEMS, CLAP_GENERAL] {
            assert!(registry.contains(name), "{name}");
        }
        assert!(registry.total_size() < 1_000_000_000);
        assert!(registry.total_size() > 200_000_000);
    }

    #[test]
    fn non_commercial_weights_are_refused() {
        // The real terms of models fourfloor must never fetch.
        let refused = [
            ("demucs-htdemucs", "MIT", "CC-BY-NC-4.0"),
            ("musicgen-small", "MIT", "CC-BY-NC-4.0"),
            ("essentia-discogs-effnet", "AGPL-3.0", "CC-BY-NC-SA-4.0"),
            ("stable-audio-open", "MIT", "other"),
            ("mystery", "MIT", ""),
        ];
        for (name, code, weights) in refused {
            let mut registry = ModelRegistry::new();
            let error = registry.register(spec(name, code, weights)).unwrap_err();
            assert!(
                matches!(error, MlError::LicenseRefused { .. }),
                "{name}: {error}"
            );
            assert!(registry.is_empty());
            assert!(registry.get(name).is_err());
        }
    }

    #[test]
    fn the_refusal_names_the_field_and_license() {
        let error = spec("x", "MIT", "CC-BY-NC-4.0")
            .check_license()
            .unwrap_err();
        match error {
            MlError::LicenseRefused {
                model,
                field,
                license,
            } => {
                assert_eq!(model, "x");
                assert_eq!(field, LicenseField::Weights);
                assert_eq!(license, "CC-BY-NC-4.0");
            }
            other => panic!("{other}"),
        }
        let error = spec("x", "GPL-3.0-only", "MIT")
            .check_license()
            .unwrap_err();
        assert!(matches!(
            error,
            MlError::LicenseRefused {
                field: LicenseField::Code,
                ..
            }
        ));
    }

    #[test]
    fn allowed_licenses_register() {
        let mut registry = ModelRegistry::new();
        for (index, (code, weights)) in [
            ("MIT", "MIT"),
            ("Apache-2.0", "CC-BY-4.0"),
            ("BSD-3-Clause", "CC0-1.0"),
        ]
        .into_iter()
        .enumerate()
        {
            registry
                .register(spec(&format!("m{index}"), code, weights))
                .unwrap();
        }
        assert_eq!(registry.len(), 3);
        assert!(matches!(
            registry.register(spec("m0", "MIT", "MIT")),
            Err(MlError::DuplicateModel(_))
        ));
    }

    #[test]
    fn malformed_entries_are_refused() {
        let mut bad_hash = spec("a", "MIT", "MIT");
        bad_hash.files[0].sha256 = "xyz".to_owned();
        let mut http = spec("a", "MIT", "MIT");
        http.files[0].url = "http://example.org/m.onnx".to_owned();
        let mut traversal = spec("a", "MIT", "MIT");
        traversal.files[0].name = "../m.onnx".to_owned();
        let mut empty = spec("a", "MIT", "MIT");
        empty.files.clear();
        for entry in [bad_hash, http, traversal, empty, spec("A/b", "MIT", "MIT")] {
            assert!(matches!(
                entry.validate(),
                Err(MlError::InvalidEntry { .. })
            ));
        }
    }

    #[test]
    fn json_entries_go_through_the_same_checks() {
        let good = serde_json::to_string(&vec![spec("j", "MIT", "MIT")]).unwrap();
        let bad = serde_json::to_string(&vec![spec("k", "MIT", "CC-BY-NC-4.0")]).unwrap();
        let mut registry = ModelRegistry::new();
        assert_eq!(registry.register_json(&good).unwrap(), 1);
        assert!(matches!(
            registry.register_json(&bad),
            Err(MlError::LicenseRefused { .. })
        ));
        assert!(!registry.contains("k"));
        assert!(registry.register_json("not json").is_err());
    }
}
