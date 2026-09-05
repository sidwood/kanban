//! Scrubbing configured secrets out of logs and diagnostic bundles
//! (DR-SS-15: secrets never enter logs or diagnostic bundles).
//!
//! The redactor learns secret values from the managed configuration:
//! every field whose key marks it a secret contributes its value, and
//! both the log write path and the diagnostic bundle assembly path
//! scrub those values before anything reaches disk or leaves the
//! machine.

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

/// Scrubs a fixed set of secret values from text and JSON.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Redactor {
    secret_values: Vec<String>,
}

impl Redactor {
    /// A redactor scrubbing exactly the given values.
    pub fn new(secret_values: Vec<String>) -> Self {
        Self {
            secret_values: secret_values
                .into_iter()
                .filter(|v| !v.is_empty())
                .collect(),
        }
    }

    /// A redactor carrying every secret-valued field in the managed
    /// configuration; an absent or unreadable configuration redacts
    /// nothing.
    pub fn from_config(data_dir: &Path) -> Self {
        let Ok(text) = std::fs::read_to_string(data_dir.join(config_file_name())) else {
            return Self::default();
        };
        let Ok(value) = serde_json::from_str::<Value>(&text) else {
            return Self::default();
        };
        let mut secret_values = Vec::new();
        collect_secret_values(&value, &mut secret_values);
        Self::new(secret_values)
    }

    /// Replaces every known secret value in `text`.
    pub fn redact_text<'a>(&self, text: &'a str) -> Cow<'a, str> {
        let carries_secret = self
            .secret_values
            .iter()
            .any(|secret| text.contains(secret.as_str()));
        if !carries_secret {
            return Cow::Borrowed(text);
        }
        let mut scrubbed = text.to_owned();
        for secret in &self.secret_values {
            scrubbed = scrubbed.replace(secret.as_str(), REDACTED);
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

/// Collects every string under a secret-marked key inside `value`.
fn collect_secret_values(value: &Value, secret_values: &mut Vec<String>) {
    match value {
        Value::Object(fields) => {
            for (key, field) in fields {
                if is_secret_key(key)
                    && let Value::String(secret) = field
                {
                    secret_values.push(secret.clone());
                }
                collect_secret_values(field, secret_values);
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use serde_json::json;

    use tempfile::TempDir;

    use super::{REDACTED, Redactor, is_secret_key};

    const PLANTED_TOKEN: &str = "kct_t61_planted_install_token";
    const PLANTED_PASSPHRASE: &str = "t61-planted-backup-passphrase";

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

        let redactor = Redactor::from_config(dir.path());

        assert_eq!(redactor.secret_count(), 2);
        assert!(
            !redactor
                .redact_text(&format!("held {PLANTED_TOKEN} and {PLANTED_PASSPHRASE}"))
                .contains(PLANTED_TOKEN),
            "both planted values must scrub"
        );

        let empty = Redactor::from_config(Path::new("/nonexistent-kanban-t61"));
        assert_eq!(
            empty.secret_count(),
            0,
            "absent configuration redacts nothing"
        );
    }
}
