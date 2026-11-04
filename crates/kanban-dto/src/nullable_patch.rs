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
