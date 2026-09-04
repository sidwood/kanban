//! The ordered event stream: one sequence for the whole core,
//! delivered to every live subscriber in order.

use std::sync::Mutex;
use std::sync::mpsc::{Receiver, sync_channel};

use kanban_app::EventSink;
use serde_json::Value;

use crate::frame::ResponseFrame;
use kanban_dto::EventEnvelope;

/// How many undelivered event lines a subscriber may queue before
/// the broker disconnects it rather than block the core.
const PER_SUBSCRIBER_BUFFER: usize = 256;

/// Assigns the global event sequence and fans each event out to
/// every live subscriber, in sequence order.
#[derive(Debug, Default)]
pub struct EventBroker {
    inner: Mutex<BrokerInner>,
}

#[derive(Debug, Default)]
struct BrokerInner {
    next_sequence: u64,
    next_subscriber_id: u64,
    subscribers: Vec<Subscriber>,
}

#[derive(Debug)]
struct Subscriber {
    id: u64,
    tx: std::sync::mpsc::SyncSender<String>,
}

impl EventBroker {
    /// A broker with no events delivered and no subscribers
    /// attached.
    pub fn new() -> Self {
        Self::default()
    }

    /// Attach a subscriber; events emitted after this call are
    /// delivered, in order, on the returned receiver until
    /// [`Self::unsubscribe`] removes it.
    pub fn subscribe(&self) -> (u64, Receiver<String>) {
        let mut inner = self.inner.lock().expect("the broker lock is sound");
        inner.next_subscriber_id += 1;
        let id = inner.next_subscriber_id;
        let (tx, rx) = sync_channel(PER_SUBSCRIBER_BUFFER);
        inner.subscribers.push(Subscriber { id, tx });
        (id, rx)
    }

    /// Detach a subscriber; its receiver drains and then
    /// disconnects.
    pub fn unsubscribe(&self, id: u64) {
        self.inner
            .lock()
            .expect("the broker lock is sound")
            .subscribers
            .retain(|subscriber| subscriber.id != id);
    }
}

impl EventSink for EventBroker {
    fn emit(&self, event_type: &str, payload: Value) {
        let line = {
            let mut inner = self.inner.lock().expect("the broker lock is sound");
            inner.next_sequence += 1;
            let envelope = ResponseFrame::Event {
                event: EventEnvelope {
                    sequence: inner.next_sequence,
                    event_type: event_type.to_owned(),
                    payload,
                },
            };
            serde_json::to_string(&envelope).expect("an event frame encodes")
        };

        // A full queue means the subscriber stopped reading; drop it
        // instead of stalling every command that emits. try_send, not
        // send: send would block the core on the stalled reader.
        self.inner
            .lock()
            .expect("the broker lock is sound")
            .subscribers
            .retain(|subscriber| subscriber.tx.try_send(line.clone()).is_ok());
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use kanban_app::EventSink;
    use serde_json::json;

    use super::EventBroker;

    fn drain(rx: &std::sync::mpsc::Receiver<String>) -> Vec<String> {
        let mut lines = Vec::new();
        while let Ok(line) = rx.try_recv() {
            lines.push(line);
        }
        lines
    }

    #[test]
    fn events_reach_every_subscriber_in_order() {
        let broker = EventBroker::new();
        let (_first_id, first) = broker.subscribe();
        let (_second_id, second) = broker.subscribe();

        broker.emit("counter.bumped", json!({ "to": 1 }));
        broker.emit("counter.bumped", json!({ "to": 2 }));
        broker.emit("counter.bumped", json!({ "to": 3 }));

        let expected = [
            r#"{"kind":"event","event":{"sequence":1,"event_type":"counter.bumped","payload":{"to":1}}}"#,
            r#"{"kind":"event","event":{"sequence":2,"event_type":"counter.bumped","payload":{"to":2}}}"#,
            r#"{"kind":"event","event":{"sequence":3,"event_type":"counter.bumped","payload":{"to":3}}}"#,
        ];
        assert_eq!(drain(&first), expected);
        assert_eq!(drain(&second), expected);
    }

    #[test]
    fn unsubscribed_receivers_are_disconnected() {
        let broker = EventBroker::new();
        let (id, rx) = broker.subscribe();

        broker.emit("counter.bumped", json!({ "to": 1 }));
        broker.unsubscribe(id);
        broker.emit("counter.bumped", json!({ "to": 2 }));

        assert_eq!(
            rx.recv().expect("the queued event is delivered first"),
            r#"{"kind":"event","event":{"sequence":1,"event_type":"counter.bumped","payload":{"to":1}}}"#
        );
        assert!(
            rx.recv().is_err(),
            "no further events arrive after unsubscribe"
        );
    }

    #[test]
    fn a_stalled_subscriber_is_dropped_not_served_stale() {
        let broker = EventBroker::new();
        // This subscriber never drains its queue.
        let (_stalled_id, stalled) = broker.subscribe();
        let (_live_id, live) = broker.subscribe();

        let total = super::PER_SUBSCRIBER_BUFFER + 10;
        let mut live_received = 0;
        for to in 1..=total {
            broker.emit("counter.bumped", json!({ "to": to }));
            if to % 16 == 0 {
                live_received += drain(&live).len();
            }
        }
        live_received += drain(&live).len();

        assert_eq!(
            live_received, total,
            "the subscriber that keeps reading gets every event"
        );

        let stalled_lines = drain(&stalled);
        assert!(
            stalled_lines.len() <= super::PER_SUBSCRIBER_BUFFER,
            "the stalled subscriber gets at most its buffer"
        );

        // The broker dropped the stalled subscriber; emitting still
        // works and reaches remaining subscribers.
        broker.emit("counter.bumped", json!({ "to": 0 }));
        assert_eq!(drain(&live).len(), 1);
    }

    #[test]
    fn sequences_are_global_across_event_types() {
        let broker = Arc::new(EventBroker::new());
        let sink = broker.clone() as Arc<dyn EventSink>;

        sink.emit("first.happened", json!(null));
        sink.emit("second.happened", json!(null));

        let (_id, rx) = broker.subscribe();
        sink.emit("third.happened", json!(null));

        let delivered = drain(&rx);
        assert_eq!(delivered.len(), 1);
        assert!(delivered[0].contains(r#""sequence":3"#));
    }
}
