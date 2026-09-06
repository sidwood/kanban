use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// A transport-visible failure returned to every client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApiError {
    pub code: ErrorCode,
    pub message: String,
    /// The aggregate's current version, carried by `stale_version`
    /// rejections so the client can correct and retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<u64>,
}

impl ApiError {
    /// A mutation arrived carrying a field no command knows.
    pub fn unknown_field(field: &str) -> Self {
        Self {
            code: ErrorCode::UnknownField,
            message: format!("unknown field `{field}`"),
            current_version: None,
        }
    }

    /// A mutation's optimistic version disagrees with the aggregate;
    /// `current` is returned for correction.
    pub fn stale_version(expected: u64, current: u64) -> Self {
        Self {
            code: ErrorCode::StaleVersion,
            message: format!(
                "optimistic version {expected} is stale; the aggregate is at version {current}"
            ),
            current_version: Some(current),
        }
    }

    /// An idempotency key was reused for a different request.
    pub fn duplicate_idempotency_key(key: &str) -> Self {
        Self {
            code: ErrorCode::DuplicateIdempotencyKey,
            message: format!("idempotency key `{key}` was already used by a different request"),
            current_version: None,
        }
    }

    /// An idempotency key was spent by an outcome recorded before
    /// fingerprints named their operation, so no retry can prove it
    /// replays that outcome. The row stays recorded for audit; the
    /// request needs a fresh key (KAN-T135).
    pub fn ambiguous_idempotency_key(key: &str) -> Self {
        Self {
            code: ErrorCode::AmbiguousIdempotencyKey,
            message: format!(
                "idempotency key `{key}` was recorded without an operation and \
                 cannot prove a replay; retry with a fresh idempotency key"
            ),
            current_version: None,
        }
    }

    /// The named operation or aggregate does not exist.
    pub fn not_found(subject: &str) -> Self {
        Self {
            code: ErrorCode::NotFound,
            message: format!("{subject} was not found"),
            current_version: None,
        }
    }

    /// The payload is malformed: a required field is missing or a
    /// value has the wrong shape.
    pub fn invalid_request(reason: &str) -> Self {
        Self {
            code: ErrorCode::InvalidRequest,
            message: reason.to_owned(),
            current_version: None,
        }
    }

    /// An unexpected failure inside the core.
    pub fn internal(reason: &str) -> Self {
        Self {
            code: ErrorCode::Internal,
            message: reason.to_owned(),
            current_version: None,
        }
    }
}

/// Stable error codes shared by commands, queries, and events.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    UnknownField,
    StaleVersion,
    DuplicateIdempotencyKey,
    AmbiguousIdempotencyKey,
    NotFound,
    InvalidRequest,
    Internal,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{ApiError, ErrorCode};

    #[test]
    fn stale_version_rejection_carries_the_current_version() {
        let error = ApiError::stale_version(3, 5);

        assert_eq!(error.code, ErrorCode::StaleVersion);
        assert_eq!(error.current_version, Some(5));
        assert_eq!(
            serde_json::to_value(&error).expect("error serialises"),
            json!({
                "code": "stale_version",
                "message": "optimistic version 3 is stale; the aggregate is at version 5",
                "current_version": 5,
            })
        );
    }

    #[test]
    fn errors_without_a_version_omit_the_field() {
        let error = ApiError::unknown_field("surprise");

        let encoded = serde_json::to_value(&error).expect("error serialises");
        assert_eq!(
            encoded,
            json!({
                "code": "unknown_field",
                "message": "unknown field `surprise`",
            })
        );
    }

    #[test]
    fn malformed_requests_map_to_the_invalid_request_code() {
        let error = ApiError::invalid_request("the mutation context is missing");

        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            serde_json::to_value(&error).expect("error serialises"),
            json!({
                "code": "invalid_request",
                "message": "the mutation context is missing",
            })
        );
    }

    #[test]
    fn api_error_rejects_unknown_fields() {
        let raw = json!({
            "code": "not_found",
            "message": "gone",
            "current_version": 7,
            "surprise": true,
        });

        let parsed: Result<ApiError, _> = serde_json::from_value(raw);
        assert!(parsed.is_err(), "ApiError must reject unknown fields");
    }

    #[test]
    fn duplicate_idempotency_key_error_names_the_key() {
        let error = ApiError::duplicate_idempotency_key("retry-1");

        assert_eq!(error.code, ErrorCode::DuplicateIdempotencyKey);
        assert!(
            error.message.contains("retry-1"),
            "the message should name the reused key"
        );
        assert_eq!(error.current_version, None);
    }

    #[test]
    fn ambiguous_idempotency_key_error_requires_a_fresh_key() {
        let error = ApiError::ambiguous_idempotency_key("legacy-1");

        assert_eq!(error.code, ErrorCode::AmbiguousIdempotencyKey);
        assert!(
            error.message.contains("legacy-1"),
            "the message should name the refused key"
        );
        assert!(
            error.message.contains("fresh idempotency key"),
            "the message should require a fresh key: {}",
            error.message
        );
        assert_eq!(error.current_version, None);
        assert_eq!(
            serde_json::to_value(error.code).expect("the code serialises"),
            json!("ambiguous_idempotency_key")
        );
    }
}
