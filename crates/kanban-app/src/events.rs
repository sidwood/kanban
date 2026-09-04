//! The port commands use to publish ordered events.

use serde_json::Value;

/// Where commands publish events. The transport's broker implements
/// this, assigning sequence numbers and delivering to every
/// subscriber in order; the application layer only says what
/// happened.
pub trait EventSink: Send + Sync {
    /// Publish one event.
    fn emit(&self, event_type: &str, payload: Value);
}

/// An [`EventSink`] that discards events, for cores that run without
/// a broker.
#[derive(Debug, Default)]
pub struct NoopEventSink;

impl EventSink for NoopEventSink {
    fn emit(&self, _event_type: &str, _payload: Value) {}
}
