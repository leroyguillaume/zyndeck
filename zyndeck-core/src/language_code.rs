use std::borrow::{Borrow, Cow};
use std::fmt;
use std::str::FromStr;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Returned when a string is not a valid ISO 639-1 language code.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("invalid language code {0:?}: expected an ISO 639-1 code such as \"fr\" or \"en\"")]
pub struct InvalidLanguageCode(pub String);

/// An ISO 639-1 language code — two lowercase letters, e.g. `"fr"` or `"en"`.
///
/// The validity invariant lives here: it is checked at construction and on serde
/// deserialisation, so any `LanguageCode` value is guaranteed to be a known
/// ISO 639-1 code. Types that key data by language (e.g. [`crate::LocalizedString`])
/// use it so they cannot hold an invalid code by construction.
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(try_from = "String", into = "String")]
#[schemars(extend("pattern" = "^[a-z]{2}$"))]
#[schemars(extend("description" = "ISO 639-1 language code, e.g. \"fr\" or \"en\"."))]
// `Cow<'static, str>` so well-known codes can be `const` (borrowed) while
// runtime-parsed ones own their string.
pub struct LanguageCode(Cow<'static, str>);

impl LanguageCode {
    /// English (`"en"`).
    pub const ENGLISH: LanguageCode = LanguageCode(Cow::Borrowed("en"));

    /// Validates `code` and wraps it, failing if it is not an ISO 639-1 code.
    pub fn new(code: impl Into<String>) -> Result<Self, InvalidLanguageCode> {
        let code = code.into();
        if isolang::Language::from_639_1(&code).is_some() {
            Ok(Self(Cow::Owned(code)))
        } else {
            Err(InvalidLanguageCode(code))
        }
    }

    /// Returns the code as a string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for LanguageCode {
    type Error = InvalidLanguageCode;

    fn try_from(code: String) -> Result<Self, Self::Error> {
        Self::new(code)
    }
}

impl FromStr for LanguageCode {
    type Err = InvalidLanguageCode;

    fn from_str(code: &str) -> Result<Self, Self::Err> {
        Self::new(code)
    }
}

impl From<LanguageCode> for String {
    fn from(code: LanguageCode) -> Self {
        code.0.into_owned()
    }
}

impl AsRef<str> for LanguageCode {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

// Lets a `BTreeMap<LanguageCode, _>` be looked up with a plain `&str`.
impl Borrow<str> for LanguageCode {
    fn borrow(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for LanguageCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_constant_is_a_valid_code() {
        assert_eq!(LanguageCode::ENGLISH.as_str(), "en");
        assert_eq!(LanguageCode::new("en"), Ok(LanguageCode::ENGLISH));
    }

    #[test]
    fn new_accepts_iso_codes() {
        assert_eq!(LanguageCode::new("fr").unwrap().as_str(), "fr");
        assert!(LanguageCode::new("en").is_ok());
    }

    #[test]
    fn new_rejects_non_iso_codes() {
        for bad in ["french", "FR", "f", "zz", ""] {
            assert_eq!(
                LanguageCode::new(bad),
                Err(InvalidLanguageCode(bad.to_owned())),
                "{bad:?} should be rejected"
            );
        }
    }

    #[test]
    fn serialises_as_a_bare_string() {
        assert_eq!(
            serde_json::to_value(LanguageCode::ENGLISH).unwrap(),
            serde_json::json!("en")
        );

        let parsed: LanguageCode = serde_json::from_value(serde_json::json!("en")).unwrap();
        assert_eq!(parsed, LanguageCode::ENGLISH);
    }

    #[test]
    fn deserialisation_rejects_invalid_codes() {
        let result: Result<LanguageCode, _> = serde_json::from_value(serde_json::json!("french"));
        assert!(result.is_err());
    }

    #[test]
    fn json_schema_carries_the_iso_pattern() {
        let schema = serde_json::to_value(schemars::schema_for!(LanguageCode)).unwrap();
        assert_eq!(schema["type"], "string");
        assert_eq!(schema["pattern"], "^[a-z]{2}$");
    }
}
