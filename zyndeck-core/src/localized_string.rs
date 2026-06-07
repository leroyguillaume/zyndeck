use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{InvalidLanguageCode, LanguageCode};

/// A piece of text translated into one or more languages.
///
/// Keys are [`LanguageCode`]s (ISO 639-1), values the text in that language —
/// e.g. `{"fr": "Marvel Champions", "en": "Marvel Champions"}`. Backed by a
/// [`BTreeMap`] so serialisation is deterministic (stable JSONB output and
/// predictable tests).
///
/// The key type guarantees the language-code invariant, so a `LocalizedString`
/// can never hold an invalid code — including when it comes off the wire, since
/// each key is deserialised through [`LanguageCode`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, JsonSchema)]
// A newtype struct already (de)serialises as its inner value, so no
// `#[serde(transparent)]` is needed — and adding it would make schemars skip the
// `extend` below. schemars 0.9 does not derive `propertyNames` from the key
// type, so constrain the object keys explicitly to mirror the `LanguageCode`
// schema.
#[schemars(extend("propertyNames" = serde_json::json!({
    "type": "string",
    "pattern": "^[a-z]{2}$",
    "description": "ISO 639-1 language code, e.g. \"fr\" or \"en\"."
})))]
// A concrete example so the API reference shows a localised object rather than
// a placeholder like `{ "additionalProp1": "string" }`.
#[schemars(extend("example" = serde_json::json!({
    "en": "Marvel Champions",
    "fr": "Marvel Champions"
})))]
pub struct LocalizedString(BTreeMap<LanguageCode, String>);

impl LocalizedString {
    /// Creates an empty localised string.
    pub fn new() -> Self {
        Self::default()
    }

    /// Builds a localised string from `(language, text)` pairs, validating each
    /// language code.
    pub fn from_pairs<K, V>(
        pairs: impl IntoIterator<Item = (K, V)>,
    ) -> Result<Self, InvalidLanguageCode>
    where
        K: Into<String>,
        V: Into<String>,
    {
        let mut value = Self::new();
        for (language, text) in pairs {
            value.try_insert(language, text)?;
        }
        Ok(value)
    }

    /// Inserts (or replaces) the `text` for an already-validated [`LanguageCode`],
    /// returning the previous value if one was set.
    pub fn insert(&mut self, language: LanguageCode, text: impl Into<String>) -> Option<String> {
        self.0.insert(language, text.into())
    }

    /// Inserts (or replaces) the `text` for `language`, validating the code.
    /// Returns the previous value if one was set.
    pub fn try_insert(
        &mut self,
        language: impl Into<String>,
        text: impl Into<String>,
    ) -> Result<Option<String>, InvalidLanguageCode> {
        Ok(self.insert(LanguageCode::new(language)?, text))
    }

    /// Returns the text for `language`, if present.
    pub fn get(&self, language: &str) -> Option<&str> {
        self.0.get(language).map(String::as_str)
    }

    /// Returns `true` if no translation is set.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Returns the number of translations.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Iterates over `(language, text)` pairs, ordered by language code.
    pub fn iter(&self) -> impl Iterator<Item = (&LanguageCode, &str)> {
        self.0.iter().map(|(k, v)| (k, v.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn try_insert_accepts_iso_codes_and_round_trips() {
        let mut name = LocalizedString::new();
        assert!(name.is_empty());

        assert_eq!(name.try_insert("fr", "Marvel Champions"), Ok(None));
        assert_eq!(
            name.try_insert("fr", "Marvel Champions JCE"),
            Ok(Some("Marvel Champions".to_owned()))
        );

        assert_eq!(name.get("fr"), Some("Marvel Champions JCE"));
        assert_eq!(name.get("en"), None);
        assert_eq!(name.len(), 1);
    }

    #[test]
    fn insert_takes_a_language_code_without_validation() {
        let mut name = LocalizedString::new();
        assert_eq!(name.insert(LanguageCode::ENGLISH, "Fox"), None);
        assert_eq!(name.get("en"), Some("Fox"));
    }

    #[test]
    fn try_insert_rejects_non_iso_codes() {
        let mut name = LocalizedString::new();

        for bad in ["french", "FR", "f", "zz", ""] {
            assert_eq!(
                name.try_insert(bad, "x"),
                Err(InvalidLanguageCode(bad.to_owned())),
                "{bad:?} should be rejected"
            );
        }
        assert!(name.is_empty());
    }

    #[test]
    fn from_pairs_validates_every_key() {
        assert!(LocalizedString::from_pairs([("fr", "Renard"), ("en", "Fox")]).is_ok());
        assert_eq!(
            LocalizedString::from_pairs([("fr", "Renard"), ("xx", "Fox")]),
            Err(InvalidLanguageCode("xx".to_owned()))
        );
    }

    #[test]
    fn serialises_as_a_bare_object() {
        let name =
            LocalizedString::from_pairs([("fr", "Marvel Champions"), ("en", "Marvel Champions")])
                .expect("valid codes");

        let json = serde_json::to_value(&name).expect("serialise");
        assert_eq!(
            json,
            serde_json::json!({"en": "Marvel Champions", "fr": "Marvel Champions"})
        );

        let parsed: LocalizedString = serde_json::from_value(json).expect("deserialise");
        assert_eq!(parsed, name);
    }

    #[test]
    fn deserialisation_rejects_invalid_codes() {
        let result: Result<LocalizedString, _> =
            serde_json::from_value(serde_json::json!({"french": "Marvel Champions"}));
        assert!(
            result.is_err(),
            "an invalid language code must not deserialise"
        );
    }

    #[test]
    fn json_schema_constrains_keys_to_language_codes() {
        let schema =
            serde_json::to_value(schemars::schema_for!(LocalizedString)).expect("serialise schema");

        assert_eq!(schema["type"], "object");
        assert_eq!(schema["additionalProperties"]["type"], "string");
        // Keys are constrained to the LanguageCode schema (ISO 639-1 pattern).
        assert_eq!(schema["propertyNames"]["pattern"], "^[a-z]{2}$");
        // A localised example is provided for the docs.
        assert_eq!(schema["example"]["fr"], "Marvel Champions");
    }

    #[test]
    fn iterates_in_language_code_order() {
        let name = LocalizedString::from_pairs([("fr", "Renard"), ("de", "Fuchs"), ("en", "Fox")])
            .expect("valid codes");

        let langs: Vec<&str> = name.iter().map(|(lang, _)| lang.as_str()).collect();
        assert_eq!(langs, ["de", "en", "fr"]);
    }
}
