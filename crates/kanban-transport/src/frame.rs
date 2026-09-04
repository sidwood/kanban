//! The newline-delimited JSON frames exchanged over the socket.

use kanban_dto::{ApiError, EventEnvelope};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a client asks the core for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameKind {
    /// Serve a named query.
    Query,
    /// Serve a named command through the mutation guard.
    Command,
    /// Start receiving the ordered event stream on this connection.
    Subscribe,
}

/// One request line from a client. `operation` and `payload` are
/// absent on `subscribe` frames.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestFrame {
    pub kind: FrameKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

/// One line back to a client: a response, a failure, an event, or
/// the acknowledgement that a subscription is live.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ResponseFrame {
    Response { payload: Value },
    Error { error: ApiError },
    Event { event: EventEnvelope },
    Subscribed {},
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{FrameKind, RequestFrame, ResponseFrame};
    use kanban_dto::{ApiError, ErrorCode, EventEnvelope};

    #[test]
    fn a_query_frame_round_trips() {
        let frame = RequestFrame {
            kind: FrameKind::Query,
            operation: Some("health.get".to_owned()),
            payload: Some(json!({})),
        };

        let encoded = serde_json::to_string(&frame).expect("the frame encodes");
        assert_eq!(
            encoded,
            r#"{"kind":"query","operation":"health.get","payload":{}}"#
        );

        let decoded: RequestFrame = serde_json::from_str(&encoded).expect("the frame decodes");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn a_subscribe_frame_omits_operation_and_payload() {
        let frame = RequestFrame {
            kind: FrameKind::Subscribe,
            operation: None,
            payload: None,
        };

        let encoded = serde_json::to_string(&frame).expect("the frame encodes");
        assert_eq!(encoded, r#"{"kind":"subscribe"}"#);

        let decoded: RequestFrame = serde_json::from_str(&encoded).expect("the frame decodes");
        assert_eq!(decoded, frame);
    }

    #[test]
    fn request_frames_reject_unknown_fields() {
        let raw = r#"{"kind":"query","operation":"health.get","payload":{},"surprise":1}"#;

        let decoded: Result<RequestFrame, _> = serde_json::from_str(raw);
        assert!(decoded.is_err(), "unknown frame fields are rejected");
    }

    #[test]
    fn an_error_response_keeps_the_current_version() {
        let frame = ResponseFrame::Error {
            error: ApiError::stale_version(3, 5),
        };

        let encoded = serde_json::to_value(&frame).expect("the frame encodes");
        assert_eq!(
            encoded,
            json!({
                "kind": "error",
                "error": {
                    "code": "stale_version",
                    "message":
                        "optimistic version 3 is stale; the aggregate is at version 5",
                    "current_version": 5,
                },
            })
        );
    }

    #[test]
    fn an_event_response_carries_the_envelope() {
        let frame = ResponseFrame::Event {
            event: EventEnvelope {
                sequence: 2,
                event_type: "counter.bumped".to_owned(),
                payload: json!({ "to": 7 }),
            },
        };

        let encoded = serde_json::to_value(&frame).expect("the frame encodes");
        assert_eq!(
            encoded,
            json!({
                "kind": "event",
                "event": {
                    "sequence": 2,
                    "event_type": "counter.bumped",
                    "payload": { "to": 7 },
                },
            })
        );
    }

    #[test]
    fn error_codes_round_trip_through_frames() {
        let frame = ResponseFrame::Error {
            error: ApiError::unknown_field("surprise"),
        };

        let encoded = serde_json::to_string(&frame).expect("the frame encodes");
        let decoded: ResponseFrame = serde_json::from_str(&encoded).expect("the frame decodes");

        assert_eq!(
            decoded,
            ResponseFrame::Error {
                error: ApiError::unknown_field("surprise")
            }
        );
        match decoded {
            ResponseFrame::Error { error } => {
                assert_eq!(error.code, ErrorCode::UnknownField);
            }
            _ => panic!("the frame should stay an error"),
        }
    }

    #[test]
    fn a_subscribed_acknowledgement_is_a_bare_kind() {
        let encoded =
            serde_json::to_string(&ResponseFrame::Subscribed {}).expect("the frame encodes");
        assert_eq!(encoded, r#"{"kind":"subscribed"}"#);
    }
}
