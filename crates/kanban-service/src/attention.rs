//! Production attention projection scheduling and runtime observation adapters.

use crate::herdr::{HerdrObserver, LiveHerdrDiagnostics};
use crate::logs::{LogLevel, LogRecord, LogWriter};
use kanban_app::HerdrDiagnostics;
use kanban_app::attention::{AttentionProjector, RuntimeAttentionFeed, RuntimeAttentionSnapshot};
use kanban_domain::Project;
use kanban_dto::ApiError;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

impl RuntimeAttentionFeed for HerdrObserver {
    fn snapshot(&self, project: &Project) -> Result<RuntimeAttentionSnapshot, ApiError> {
        let registration = project.registration();
        Ok(RuntimeAttentionSnapshot {
            diagnostics: LiveHerdrDiagnostics::new(self).for_project(
                project.id().value(),
                registration.herdr_session(),
                registration.seed_workspace(),
                registration.herdr_workspace(),
            ),
            signals: self.attention_signals(project.id().value()),
        })
    }
}

pub(crate) struct AttentionScheduler {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl AttentionScheduler {
    pub fn spawn(projector: AttentionProjector, log: Arc<LogWriter>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                if let Err(error) = projector.refresh(&crate::schedule_scheduler::now_stored()) {
                    let _ = log.append(&LogRecord::new(
                        LogLevel::Error,
                        "attention",
                        format!("attention projection failed: {}", error.message),
                    ));
                }
                thread::park_timeout(Duration::from_secs(1));
            }
        });
        Self {
            stop,
            worker: Some(worker),
        }
    }
}
impl Drop for AttentionScheduler {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.thread().unpark();
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::test_client::{Client, boot};
    use serde_json::json;
    use std::time::{Duration, Instant};

    #[test]
    fn attention_consolidation_reaches_the_serving_core_without_a_manual_refresh() {
        let dir = tempfile::TempDir::new().unwrap();
        let repository = dir.path().join("repository");
        std::fs::create_dir_all(repository.join(".git")).unwrap();
        let repository = repository.canonicalize().unwrap();
        let core = boot(&dir);
        let mut client = Client::connect(core.socket_path());
        client.command(
            "project.register",
            json!({
                "mutation":{"optimistic_version":0,"idempotency_key":"attention-project"},
                "code":"CORE","name":"Attention project","repository":repository,
                "seed_workspace":repository,"default_branch":"main","herdr_workspace":"kanban.seed",
            }),
        );
        let ticket=client.command("ticket.create",json!({
            "mutation":{"optimistic_version":0,"idempotency_key":"attention-task"},"project_id":1,
            "kind":"task","priority":"normal","title":"Check the approval","subtype":"operational",
            "mode":"human","completion":["Approval is checked."],
        }));
        client.command(
            "ticket.blocker.add",
            json!({
                "mutation":{"optimistic_version":1,"idempotency_key":"attention-blocker"},
                "ticket_id":ticket["id"],"description":"Need operator approval",
            }),
        );
        let expected_ticket = ticket["id"].as_u64().unwrap().to_string();
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            let inbox = client.query_with("attention.list", json!({}));
            if inbox["items"].as_array().unwrap().iter().any(|item| {
                item["kind"] == "blocker"
                    && item["subject_id"].as_str() == Some(expected_ticket.as_str())
            }) {
                break;
            }
            assert!(
                Instant::now() < until,
                "the serving core never projected the blocker"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        core.shutdown();
    }
}
