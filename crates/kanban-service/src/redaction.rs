//! Scrubbing configured secrets out of logs and diagnostic bundles
//! (DR-SS-15: secrets never enter logs or diagnostic bundles).
//!
//! The redactor learns secret values from the managed configuration:
//! every field whose key marks it a secret contributes its value, and
//! both the log write path and the diagnostic bundle assembly path
//! scrub those values before anything reaches disk or leaves the
//! machine. A secret also carries the forms it takes inside
//! serialized JSON — escaped quotes, escaped backslashes, and
//! `\u`-escaped non-ASCII included — because the bundle copies
//! serialized log text, where the raw bytes never appear.

use std::borrow::Cow;
use std::path::Path;

use serde_json::Value;

use kanban_storage::paths::config_file_name;

/// What every redacted value is replaced with.
pub const REDACTED: &str = "[REDACTED]";

/// Key fragments that mark a configuration field as a secret, matched
/// against the key with every separator removed.
const SECRET_KEY_MARKERS: &[&str] = &[
    "token",
    "secret",
    "password",
    "passphrase",
    "credential",
    "apikey",
    "privatekey",
];

/// Why the managed configuration could not feed the redactor.
#[derive(Debug, thiserror::Error)]
pub enum RedactionSourceError {
    /// The configuration file refused to be read.
    #[error("the configuration could not be read: {source}")]
    Read {
        /// The underlying failure.
        source: std::io::Error,
    },
    /// The configuration is not valid JSON.
    #[error("the configuration is not valid JSON: {source}")]
    Parse {
        /// The underlying failure.
        source: serde_json::Error,
    },
}

impl RedactionSourceError {
    /// A fixed, content-free label naming which failure this is. A
    /// caller that had to withhold a record carries this instead of
    /// the error's own message, because that message is built from
    /// the very source nobody could vouch for.
    pub fn reason(&self) -> &'static str {
        match self {
            Self::Read { .. } => "configuration_unreadable",
            Self::Parse { .. } => "configuration_malformed",
        }
    }
}

/// Scrubs a fixed set of secret values from text and JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redactor {
    secret_values: Vec<String>,
    /// Every form the secrets take on disk, ordered so the form that
    /// carries more escaping replaces first: substituting a raw form
    /// inside an escaped occurrence would leave broken halves behind.
    scrub_forms: Vec<String>,
}

impl Redactor {
    /// A redactor scrubbing exactly the given values, in every
    /// serialized form each of them can take.
    pub fn new(secret_values: Vec<String>) -> Self {
        let secret_values: Vec<String> = secret_values
            .into_iter()
            .filter(|value| !value.is_empty())
            .collect();
        let mut scrub_forms = Vec::new();
        for secret in &secret_values {
            for form in [
                json_escaped(&json_escaped(secret)),
                json_escaped(secret),
                unicode_escaped(secret),
                secret.clone(),
            ] {
                if !scrub_forms.contains(&form) {
                    scrub_forms.push(form);
                }
            }
        }
        scrub_forms.sort_by_key(|form| std::cmp::Reverse(form.len()));
        Self {
            secret_values,
            scrub_forms,
        }
    }

    /// A redactor carrying every secret-valued field in the managed
    /// configuration, or the reason that configuration cannot feed
    /// one. An absent configuration is not a failure — there is
    /// nothing to learn — but an unreadable or malformed one is: the
    /// secrets it holds stay unknown, and a caller told only "no
    /// secrets" would write them out in the clear.
    pub fn from_config(data_dir: &Path) -> Result<Self, RedactionSourceError> {
        Ok(match read_managed_config(data_dir)? {
            Some(configuration) => Self::from_config_json(&configuration),
            None => Self::default(),
        })
    }

    /// A redactor carrying every secret-valued field in an already
    /// parsed configuration.
    pub fn from_config_json(configuration: &Value) -> Self {
        let mut secret_values = Vec::new();
        collect_secret_values(configuration, &mut secret_values);
        Self::new(secret_values)
    }

    /// A redactor scrubbing everything either side knows. Refreshed
    /// knowledge must never forget a value the configuration rotated
    /// away from: a retired secret is still a secret.
    pub(crate) fn union(&self, other: &Redactor) -> Redactor {
        let mut secret_values = self.secret_values.clone();
        for secret in &other.secret_values {
            if !secret_values.contains(secret) {
                secret_values.push(secret.clone());
            }
        }
        Redactor::new(secret_values)
    }

    /// Replaces every known form of every known secret in `text`.
    pub fn redact_text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if !self
            .scrub_forms
            .iter()
            .any(|form| text.contains(form.as_str()))
        {
            return Cow::Borrowed(text);
        }
        let mut scrubbed = text.to_owned();
        for form in &self.scrub_forms {
            scrubbed = scrubbed.replace(form.as_str(), REDACTED);
        }
        Cow::Owned(scrubbed)
    }

    /// Replaces secret-valued fields wholesale and scrubs known
    /// secret values from every other string in `value`.
    pub fn redact_json(&self, value: &Value) -> Value {
        match value {
            Value::Object(fields) => {
                let redacted: serde_json::Map<String, Value> = fields
                    .iter()
                    .map(|(key, field)| {
                        let field = if is_secret_key(key) {
                            Value::String(REDACTED.to_owned())
                        } else {
                            self.redact_json(field)
                        };
                        (key.clone(), field)
                    })
                    .collect();
                Value::Object(redacted)
            }
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.redact_json(item)).collect())
            }
            Value::String(text) => Value::String(self.redact_text(text).into_owned()),
            other => other.clone(),
        }
    }

    /// How many distinct secret values this redactor scrubs.
    pub fn secret_count(&self) -> usize {
        self.secret_values.len()
    }
}

/// Reads the managed configuration, distinguishing an absent file
/// (nothing to learn) from one that cannot be read or parsed (the
/// caller decides whether that is survivable).
pub(crate) fn read_managed_config(data_dir: &Path) -> Result<Option<Value>, RedactionSourceError> {
    let path = data_dir.join(config_file_name());
    match std::fs::read_to_string(&path) {
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(RedactionSourceError::Read { source }),
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|source| RedactionSourceError::Parse { source }),
    }
}

/// Whether a configuration key marks its field as a secret.
fn is_secret_key(key: &str) -> bool {
    let normalised: String = key
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .map(|ch| ch.to_ascii_lowercase())
        .collect();
    SECRET_KEY_MARKERS
        .iter()
        .any(|marker| normalised.contains(marker))
}

/// Collects every string a secret-marked key covers: a direct string
/// value, and every string under a secret-marked container, because a
/// whole subtree flagged as credentials is secret material even when
/// its inner keys carry no marker of their own.
fn collect_secret_values(value: &Value, secret_values: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                if is_secret_key(key) {
                    collect_every_string(field, secret_values);
                } else {
                    collect_secret_values(field, secret_values);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_secret_values(item, secret_values);
            }
        }
        _ => {}
    }
}

/// Collects every string inside `value`.
fn collect_every_string(value: &Value, secret_values: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for field in fields.values() {
                collect_every_string(field, secret_values);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_every_string(item, secret_values);
            }
        }
        Value::String(text) => secret_values.push(text.clone()),
        _ => {}
    }
}

/// The secret as it appears inside a JSON string literal, without the
/// surrounding quotes: quotes and backslashes doubled, control
/// characters escaped, non-ASCII left as UTF-8, exactly as
/// `serde_json` writes it.
fn json_escaped(secret: &str) -> String {
    let encoded = serde_json::to_string(secret).expect("a string always serialises");
    encoded[1..encoded.len() - 1].to_owned()
}

/// The secret as it appears inside a JSON string literal written by an
/// escaper that also encodes non-ASCII as `\u` escapes, astral
/// characters as surrogate pairs.
fn unicode_escaped(secret: &str) -> String {
    let mut escaped = String::with_capacity(secret.len());
    for character in secret.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\u{8}' => escaped.push_str("\\b"),
            '\u{c}' => escaped.push_str("\\f"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            other if (other as u32) > 0xFFFF => {
                let supplementary = (other as u32) - 0x1_0000;
                let high = 0xD800 + (supplementary >> 10);
                let low = 0xDC00 + (supplementary & 0x3FF);
                escaped.push_str(&format!("\\u{high:04x}\\u{low:04x}"));
            }
            other if (other as u32) < 0x20 || (other as u32) > 0x7E => {
                escaped.push_str(&format!("\\u{:04x}", other as u32));
            }
            other => escaped.push(other),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use tempfile::TempDir;

    use super::{REDACTED, RedactionSourceError, Redactor, is_secret_key};

    const PLANTED_TOKEN: &str = "kct_t61_planted_install_token";
    const PLANTED_PASSPHRASE: &str = "t61-planted-backup-passphrase";
    const PLANTED_QUOTED: &str = r#"t61 "quoted" planted secret"#;
    const PLANTED_BACKSLASH: &str = r"t61\planted\secret";
    const PLANTED_NON_ASCII: &str = "t61-planted-pässphrase-café";
    const PLANTED_ASTRAL: &str = "t61-planted-🔐-key";

    #[test]
    fn secret_keys_mark_secret_valued_fields() {
        for key in [
            "mcp_install_token",
            "backup_passphrase",
            "api_key",
            "privateKey",
            "PASSWORD",
            "client_credential",
        ] {
            assert!(is_secret_key(key), "`{key}` must read as a secret key");
        }
        for key in ["theme", "backup_retention", "hotkey", "moniker"] {
            assert!(!is_secret_key(key), "`{key}` must stay an ordinary key");
        }
    }

    #[test]
    fn secret_exclusion_scrubs_configured_values_from_text() {
        let redactor = Redactor::new(vec![PLANTED_TOKEN.into(), PLANTED_PASSPHRASE.into()]);
        let text =
            format!("installed with {PLANTED_TOKEN} then unlocked with {PLANTED_PASSPHRASE}");

        assert_eq!(
            redactor.redact_text(&text).as_ref(),
            format!("installed with {REDACTED} then unlocked with {REDACTED}"),
            "every planted value must vanish from free text"
        );
        assert_eq!(
            redactor.redact_text("no secrets appear here"),
            "no secrets appear here",
            "clean text passes through untouched"
        );
        assert_eq!(redactor.secret_count(), 2);
    }

    #[test]
    fn secret_exclusion_replaces_secret_valued_json_fields() {
        let redactor = Redactor::new(vec![PLANTED_TOKEN.into()]);
        let value = json!({
            "theme": "dark",
            "mcp_install_token": PLANTED_TOKEN,
            "nested": { "backup_passphrase": "another-planted-value" },
            "counts": 3,
        });

        let redacted = redactor.redact_json(&value);

        assert_eq!(redacted["theme"], json!("dark"), "ordinary fields survive");
        assert_eq!(redacted["mcp_install_token"], json!(REDACTED));
        assert_eq!(redacted["nested"]["backup_passphrase"], json!(REDACTED));
        assert_eq!(redacted["counts"], json!(3));
    }

    #[test]
    fn secret_exclusion_scrubs_values_hiding_inside_plain_strings() {
        let redactor = Redactor::new(vec![PLANTED_TOKEN.into()]);
        let value = json!({
            "note": format!("token was {PLANTED_TOKEN} at install"),
            "list": [format!("prefix-{PLANTED_TOKEN}")],
        });

        let redacted = redactor.redact_json(&value);

        assert_eq!(
            redacted["note"],
            json!(format!("token was {REDACTED} at install")),
            "a secret inside an ordinary string must still vanish"
        );
        assert_eq!(
            redacted["list"],
            json!([format!("prefix-{REDACTED}")]),
            "array strings are scrubbed too"
        );
    }

    /// A secret nested under a secret-marked container key is learned
    /// even when its own key carries no marker, and the container
    /// still collapses wholesale in exported JSON.
    #[test]
    fn secret_exclusion_learns_values_nested_under_secret_keys() {
        let configuration = json!({
            "theme": "dark",
            "credentials": {
                "herdr": "hct_live_t61_planted",
                "mcp": { "install_token": PLANTED_TOKEN },
            },
        });
        let redactor = Redactor::from_config_json(&configuration);

        assert_eq!(redactor.secret_count(), 2, "the whole subtree is learned");
        assert!(
            !redactor
                .redact_text("held hct_live_t61_planted and more")
                .contains("hct_live_t61_planted"),
            "a string with no marker of its own, under a secret-marked container, scrubs from free text"
        );
        let redacted = redactor.redact_json(&configuration);
        assert_eq!(redacted["credentials"], json!(REDACTED));
        assert_eq!(redacted["theme"], json!("dark"));
    }

    /// Serialized JSON never carries the raw bytes of a secret holding
    /// quotes, backslashes, or non-ASCII text: it carries the escaped
    /// form, once per level of nesting, and `\u`-escaping serializers
    /// carry code-point escapes instead.
    #[test]
    fn secret_exclusion_scrubs_serialized_forms_with_quotes_backslashes_and_non_ascii() {
        let redactor = Redactor::new(vec![
            PLANTED_QUOTED.into(),
            PLANTED_BACKSLASH.into(),
            PLANTED_NON_ASCII.into(),
            PLANTED_ASTRAL.into(),
        ]);
        let embedded = json!({
            "quoted": PLANTED_QUOTED,
            "backslash": PLANTED_BACKSLASH,
            "non_ascii": PLANTED_NON_ASCII,
            "astral": PLANTED_ASTRAL,
        });
        let serialized = serde_json::to_string(&embedded).expect("the fixture serialises");
        let double_serialized =
            serde_json::to_string(&serialized).expect("the fixture serialises again");
        let unicode_serialized = serialized.replace(
            &super::json_escaped(PLANTED_NON_ASCII),
            &super::unicode_escaped(PLANTED_NON_ASCII),
        );

        for (label, text) in [
            ("once-escaped", serialized.as_str()),
            ("twice-escaped", double_serialized.as_str()),
            ("code-point-escaped", unicode_serialized.as_str()),
        ] {
            let scrubbed = redactor.redact_text(text);
            for secret in [
                PLANTED_QUOTED,
                PLANTED_BACKSLASH,
                PLANTED_NON_ASCII,
                PLANTED_ASTRAL,
            ] {
                assert!(
                    !scrubbed.contains(secret),
                    "the {label} form of `{secret}` must vanish: {scrubbed}"
                );
                assert!(
                    !scrubbed.contains(&super::json_escaped(secret)),
                    "the {label} escaped form of `{secret}` must vanish: {scrubbed}"
                );
            }
            assert!(
                scrubbed.contains(REDACTED),
                "the {label} scrub leaves the marker behind: {scrubbed}"
            );
        }
    }

    /// Refreshed knowledge extends what was held: a value added to the
    /// configuration after the redactor was built is scrubbed, and a
    /// value the configuration rotated away from stays scrubbed.
    #[test]
    fn secret_exclusion_union_keeps_rotated_values_and_learns_new_ones() {
        let held = Redactor::new(vec![PLANTED_TOKEN.into()]);
        let rotated_to = Redactor::new(vec![PLANTED_PASSPHRASE.into()]);

        let refreshed = held.union(&rotated_to);

        assert_eq!(refreshed.secret_count(), 2);
        let text = format!("{PLANTED_TOKEN} then {PLANTED_PASSPHRASE}");
        let scrubbed = refreshed.redact_text(&text);
        assert_eq!(
            scrubbed,
            format!("{REDACTED} then {REDACTED}"),
            "both the retired and the rotated-in value must scrub"
        );
    }

    #[test]
    fn from_config_collects_nested_secret_values_only() {
        let dir = TempDir::new().expect("a scratch directory is available");
        std::fs::write(
            dir.path().join("config.json"),
            format!(
                r#"{{"theme":"dark","mcp":{{"install_token":"{PLANTED_TOKEN}"}},"backup_passphrase":"{PLANTED_PASSPHRASE}"}}"#
            ),
        )
        .expect("the configuration plants secrets");

        let redactor =
            Redactor::from_config(dir.path()).expect("the planted configuration feeds redaction");

        assert_eq!(redactor.secret_count(), 2);
        assert!(
            !redactor
                .redact_text(&format!("held {PLANTED_TOKEN} and {PLANTED_PASSPHRASE}"))
                .contains(PLANTED_TOKEN),
            "both planted values must scrub"
        );

        let empty = Redactor::from_config(Path::new("/nonexistent-kanban-t61"))
            .expect("an absent configuration is not a failure");
        assert_eq!(
            empty.secret_count(),
            0,
            "absent configuration redacts nothing"
        );
    }

    /// The source of redaction knowledge is not best-effort. A
    /// configuration that cannot be read or parsed reports itself,
    /// because a caller that cannot tell "nothing to scrub" from
    /// "cannot know what to scrub" writes out secrets it never
    /// learned.
    #[test]
    fn from_config_reports_a_configuration_that_cannot_feed_redaction() {
        let dir = TempDir::new().expect("a scratch directory is available");
        let config_path = dir.path().join("config.json");

        std::fs::write(&config_path, "{ not json").expect("the malformed configuration is written");
        assert!(
            matches!(
                Redactor::from_config(dir.path()),
                Err(RedactionSourceError::Parse { .. })
            ),
            "a malformed configuration must report itself, never read as empty"
        );

        std::fs::remove_file(&config_path).expect("the malformed configuration is removed");
        std::fs::create_dir(&config_path).expect("the unreadable configuration is planted");
        assert!(
            matches!(
                Redactor::from_config(dir.path()),
                Err(RedactionSourceError::Read { .. })
            ),
            "an unreadable configuration must report itself, never read as empty"
        );
    }
}
