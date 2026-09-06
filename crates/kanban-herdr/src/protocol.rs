//! Newline-delimited JSON frames for the per-session Herdr socket.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// What a Kanban client asks one Herdr session for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "snake_case")]
pub enum HerdrRequest {
    /// Capture the full session state.
    Snapshot,
    /// Start receiving push events for normal operation.
    Subscribe,
    /// Block until a scripted condition is met or a timeout elapses.
    Wait { condition: String, timeout_ms: u64 },
    /// Deliver a prompt to one role tab.
    Prompt { role: String, message: String },
    /// Wake one role tab — the Project Coordinator on dispatch —
    /// without launching an implementation agent (DR-HB-14, DR-HB-16).
    Wake {
        role: String,
        dispatch_request_id: u64,
    },
}

/// One line back from Herdr.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HerdrResponse {
    Snapshot(Snapshot),
    Subscribed,
    Event { payload: Value },
    WaitResult { met: bool, detail: Value },
    PromptResult { accepted: bool },
    WakeResult { accepted: bool },
    Error { message: String },
}

/// The full state capture returned by a snapshot request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    /// The exclusive named session.
    pub session: String,
    /// The product workspace this session maps to.
    pub product_workspace: String,
    /// The Herdr workspace bound to the product workspace.
    pub herdr_workspace: String,
    /// The opaque session state Herdr exposes to observers.
    pub state: Value,
    /// When Herdr captured the snapshot.
    pub captured_at: String,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{HerdrRequest, HerdrResponse, Snapshot};

    #[test]
    fn request_frames_round_trip() {
        let frames = [
            HerdrRequest::Snapshot,
            HerdrRequest::Subscribe,
            HerdrRequest::Wait {
                condition: "role.settled".to_owned(),
                timeout_ms: 1_000,
            },
            HerdrRequest::Prompt {
                role: "implementer".to_owned(),
                message: "continue".to_owned(),
            },
            HerdrRequest::Wake {
                role: "coordinator".to_owned(),
                dispatch_request_id: 17,
            },
        ];

        for frame in frames {
            let encoded = serde_json::to_string(&frame).expect("the frame encodes");
            let decoded: HerdrRequest = serde_json::from_str(&encoded).expect("the frame decodes");
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn snapshot_payload_round_trips() {
        let snapshot = Snapshot {
            session: "kanban-main".to_owned(),
            product_workspace: "/workspaces/kanban.seed".to_owned(),
            herdr_workspace: "kanban.seed".to_owned(),
            state: json!({ "roles": [] }),
            captured_at: "2026-09-05T04:46:00Z".to_owned(),
        };
        let response = HerdrResponse::Snapshot(snapshot.clone());
        let encoded = serde_json::to_string(&response).expect("the response encodes");
        let decoded: HerdrResponse = serde_json::from_str(&encoded).expect("the response decodes");
        assert_eq!(decoded, HerdrResponse::Snapshot(snapshot));
    }
}
