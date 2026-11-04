//! Deserialises JSON `null` as `Some(None)` for patch fields on issue save.

use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer};

pub fn deserialize_string_patch<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(value) => Ok(Some(Some(
            String::deserialize(value).map_err(D::Error::custom)?,
        ))),
    }
}

pub fn deserialize_u64_patch<'de, D>(deserializer: D) -> Result<Option<Option<u64>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(value) => Ok(Some(Some(
            u64::deserialize(value).map_err(D::Error::custom)?,
        ))),
    }
}

/// Accepts absent, JSON `null` (clear), a numeric ticket id, or an issue key string.
pub fn deserialize_parent_patch<'de, D>(
    deserializer: D,
) -> Result<Option<Option<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    match Option::<serde_json::Value>::deserialize(deserializer)? {
        None | Some(serde_json::Value::Null) => Ok(Some(None)),
        Some(value) => match value {
            serde_json::Value::Number(number) => Ok(Some(Some(
                number
                    .as_u64()
                    .ok_or_else(|| D::Error::custom("parent_id must be a positive integer"))?
                    .to_string(),
            ))),
            serde_json::Value::String(text) => Ok(Some(Some(text))),
            _ => Err(D::Error::custom(
                "parent_id must be null, a ticket id, or an issue key",
            )),
        },
    }
}
