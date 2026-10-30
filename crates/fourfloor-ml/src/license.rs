//! The license allow-list.
//!
//! fourfloor ships under the GPL and must stay usable commercially, so a
//! model may only be used when both its code and its weights carry a
//! permissive license that allows commercial use. Everything else is
//! refused: non-commercial Creative Commons variants, research-only terms,
//! custom "community" licenses and anything this module does not recognise.

/// A license this crate recognises as allowing commercial use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AllowedLicense {
    /// MIT.
    Mit,
    /// Apache License 2.0.
    Apache2,
    /// BSD 2-Clause or 3-Clause.
    Bsd,
    /// Creative Commons Zero 1.0 (public domain dedication).
    Cc0,
    /// Creative Commons Attribution (any version), with no further clauses.
    CcBy,
}

/// The SPDX identifiers on the allow-list, lowercase.
const ALLOWED: &[(&str, AllowedLicense)] = &[
    ("mit", AllowedLicense::Mit),
    ("apache-2.0", AllowedLicense::Apache2),
    ("bsd-2-clause", AllowedLicense::Bsd),
    ("bsd-3-clause", AllowedLicense::Bsd),
    ("cc0-1.0", AllowedLicense::Cc0),
    ("cc-by-1.0", AllowedLicense::CcBy),
    ("cc-by-2.0", AllowedLicense::CcBy),
    ("cc-by-2.5", AllowedLicense::CcBy),
    ("cc-by-3.0", AllowedLicense::CcBy),
    ("cc-by-4.0", AllowedLicense::CcBy),
];

/// Human-readable names of the allow-list, for messages and UI.
pub const ALLOWED_LICENSES: &[&str] = &[
    "MIT",
    "Apache-2.0",
    "BSD-2-Clause",
    "BSD-3-Clause",
    "CC0-1.0",
    "CC-BY-4.0 (and earlier CC BY versions)",
];

/// Classifies an SPDX license identifier.
///
/// Matching is on the whole identifier, ignoring case and surrounding
/// space, so `CC-BY-NC-4.0`, `CC-BY-NC-SA-4.0`, `CC-BY-SA-4.0`,
/// `Apache-2.0 WITH ...`, `MIT-0-custom`, `other` and the empty string all
/// return `None`. Hugging Face spells some identifiers without the version
/// (`cc0`, `cc-by-4.0` is already SPDX); the unversioned `cc0` is accepted.
pub fn classify(license: &str) -> Option<AllowedLicense> {
    let key = license.trim().to_ascii_lowercase();
    if key == "cc0" {
        return Some(AllowedLicense::Cc0);
    }
    ALLOWED
        .iter()
        .find(|(name, _)| *name == key)
        .map(|(_, license)| *license)
}

/// Whether `license` is on the allow-list.
pub fn is_allowed(license: &str) -> bool {
    classify(license).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permissive_licenses_are_allowed() {
        for name in [
            "MIT",
            "mit",
            " Apache-2.0 ",
            "BSD-2-Clause",
            "BSD-3-Clause",
            "CC0-1.0",
            "cc0",
            "CC-BY-4.0",
            "CC-BY-3.0",
        ] {
            assert!(is_allowed(name), "{name} should be allowed");
        }
        assert_eq!(classify("apache-2.0"), Some(AllowedLicense::Apache2));
        assert_eq!(classify("CC-BY-4.0"), Some(AllowedLicense::CcBy));
    }

    #[test]
    fn non_commercial_and_unknown_licenses_are_refused() {
        for name in [
            "CC-BY-NC-4.0",
            "CC-BY-NC-SA-4.0",
            "CC-BY-NC-ND-4.0",
            "cc-by-nc-4.0",
            "CC-BY-SA-4.0",
            "CC-BY-ND-4.0",
            "CC-BY",
            "AGPL-3.0",
            "GPL-3.0-or-later",
            "research-only",
            "other",
            "unknown",
            "",
            "MIT AND CC-BY-NC-4.0",
            "MIT OR CC-BY-NC-4.0",
            "Apache-2.0 WITH Commons-Clause",
            "openrail",
            "llama2",
        ] {
            assert!(!is_allowed(name), "{name:?} should be refused");
        }
    }
}
