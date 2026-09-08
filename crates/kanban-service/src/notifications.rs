//! Native notification and Herdr mirror adapters; neither owns acknowledgement.

use crate::logs::{LogLevel, LogRecord, LogWriter};
use kanban_app::notifications::{
    NotificationDispatcher, NotificationMessage, NotificationOutcome, NotificationPermissionPort,
    NotificationSink,
};
use kanban_dto::{
    ApiError, NotificationPermissionRecord, NotificationPermissionState, NotificationTarget,
};
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::sync::Mutex;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::Duration;

#[derive(Default)]
pub(crate) struct NativeNotifications {
    pending: Arc<AtomicBool>,
    #[cfg(target_os = "macos")]
    fault: Arc<Mutex<Option<String>>>,
}
/// macOS consent belongs to the application process, not an individual Project.
pub(crate) fn shared_native() -> Arc<NativeNotifications> {
    static NATIVE: std::sync::OnceLock<Arc<NativeNotifications>> = std::sync::OnceLock::new();
    NATIVE
        .get_or_init(|| Arc::new(NativeNotifications::default()))
        .clone()
}

impl NotificationPermissionPort for NativeNotifications {
    fn status(&self) -> NotificationPermissionRecord {
        let pending = self.pending.load(Ordering::Acquire);
        #[cfg(target_os = "macos")]
        {
            use mac_usernotifications::AuthorizationStatus;
            if mac_usernotifications::check_bundle().is_err() {
                return NotificationPermissionRecord {
                    state: NotificationPermissionState::Unavailable,
                    reason: Some(
                        "Notifications require the installed, signed Kanban app bundle.".to_owned(),
                    ),
                    request_pending: pending,
                };
            }
            let settings = match mac_usernotifications::blocking::get_notification_settings() {
                Ok(settings) => settings,
                Err(_) => {
                    return NotificationPermissionRecord {
                        state: NotificationPermissionState::Unavailable,
                        reason: Some("macOS notification settings could not be read.".to_owned()),
                        request_pending: pending,
                    };
                }
            };
            let state = match settings.authorization_status {
                AuthorizationStatus::Authorized
                | AuthorizationStatus::Provisional
                | AuthorizationStatus::Ephemeral => NotificationPermissionState::Granted,
                AuthorizationStatus::NotDetermined => NotificationPermissionState::NotDetermined,
                AuthorizationStatus::Denied => NotificationPermissionState::Denied,
                AuthorizationStatus::Unknown => NotificationPermissionState::Unavailable,
            };
            let reason = if state == NotificationPermissionState::NotDetermined {
                self.fault.lock().unwrap().clone()
            } else {
                None
            };
            NotificationPermissionRecord {
                state,
                reason,
                request_pending: pending,
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            NotificationPermissionRecord {
                state: NotificationPermissionState::Unavailable,
                reason: Some(
                    "Local macOS notifications are unavailable on this platform.".to_owned(),
                ),
                request_pending: pending,
            }
        }
    }
    fn request(&self) -> Result<bool, ApiError> {
        let status = self.status();
        match status.state {
            NotificationPermissionState::Granted => return Ok(false),
            NotificationPermissionState::Denied => {
                return Err(ApiError::invalid_request(
                    "notification permission must be changed in System Settings",
                ));
            }
            NotificationPermissionState::Unavailable => {
                return Err(ApiError::invalid_request(
                    status
                        .reason
                        .as_deref()
                        .unwrap_or("native notifications are unavailable"),
                ));
            }
            NotificationPermissionState::NotDetermined => {}
        }
        #[cfg(target_os = "macos")]
        {
            if self.pending.swap(true, Ordering::AcqRel) {
                return Ok(false);
            }
            *self.fault.lock().unwrap() = None;
            let pending = self.pending.clone();
            let fault = self.fault.clone();
            if thread::Builder::new()
                .name("notification-permission".to_owned())
                .spawn(move || {
                    if mac_usernotifications::blocking::request_auth().is_err() {
                        *fault.lock().unwrap() =
                            Some("macOS could not complete the permission request.".to_owned());
                    }
                    pending.store(false, Ordering::Release);
                })
                .is_err()
            {
                self.pending.store(false, Ordering::Release);
                *self.fault.lock().unwrap() =
                    Some("The permission request worker could not start.".to_owned());
                return Err(ApiError::internal(
                    "the permission request worker could not start",
                ));
            }
            Ok(true)
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err(ApiError::invalid_request(
                "native notifications are unavailable",
            ))
        }
    }
}
impl NotificationSink for NativeNotifications {
    fn deliver(&self, message: &NotificationMessage) -> NotificationOutcome {
        let permission = self.status();
        if permission.state != NotificationPermissionState::Granted {
            return NotificationOutcome::NotSent {
                reason: permission
                    .reason
                    .unwrap_or_else(|| "macOS notification permission is not granted".to_owned()),
            };
        }
        #[cfg(target_os = "macos")]
        {
            let id = format!("kanban-attention-{}", message.delivery_id);
            match mac_usernotifications::Notification::new()
                .id(&id)
                .title(&message.title)
                .message(&message.body)
                .send_blocking()
            {
                Ok(handle) => NotificationOutcome::Submitted {
                    receipt: handle.notification_id().to_owned(),
                },
                Err(_) => NotificationOutcome::Uncertain {
                    reason: "macOS did not confirm notification submission.".to_owned(),
                },
            }
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = message;
            NotificationOutcome::NotSent {
                reason: "native notifications are unavailable".to_owned(),
            }
        }
    }
}

pub(crate) struct HerdrMirrors {
    socket_root: PathBuf,
}
impl HerdrMirrors {
    pub fn new(socket_root: PathBuf) -> Self {
        Self { socket_root }
    }
}
impl NotificationSink for HerdrMirrors {
    fn deliver(&self, message: &NotificationMessage) -> NotificationOutcome {
        let NotificationTarget::HerdrMirror {
            session_name,
            product_workspace,
            herdr_workspace,
            role,
        } = &message.target
        else {
            return NotificationOutcome::NotSent {
                reason: "mirror target is missing".to_owned(),
            };
        };
        let session = match session_name {
            Some(name) => match kanban_domain::HerdrSession::named(name) {
                Ok(session) => session,
                Err(_) => {
                    return NotificationOutcome::NotSent {
                        reason: "mirror session is invalid".to_owned(),
                    };
                }
            },
            None => kanban_domain::HerdrSession::Default,
        };
        let mapping =
            kanban_herdr::SessionMapping::new(session, product_workspace, herdr_workspace);
        let mut client = match kanban_herdr::SessionClient::connect(mapping, &self.socket_root) {
            Ok(client) => client,
            Err(_) => {
                return NotificationOutcome::NotSent {
                    reason: "the configured Herdr binding is unavailable".to_owned(),
                };
            }
        };
        match client.prompt(kanban_herdr::PromptRequest {
            role: role.clone(),
            message: message.body.clone(),
        }) {
            Ok(true) => NotificationOutcome::Submitted {
                receipt: format!("herdr-mirror-{}", message.delivery_id),
            },
            Ok(false) => NotificationOutcome::NotSent {
                reason: "Herdr refused the informational mirror".to_owned(),
            },
            Err(_) => NotificationOutcome::Uncertain {
                reason: "Herdr did not confirm mirror submission".to_owned(),
            },
        }
    }
}

pub(crate) struct NotificationRouter {
    local: Arc<NativeNotifications>,
    mirror: HerdrMirrors,
}
impl NotificationRouter {
    pub fn new(local: Arc<NativeNotifications>, socket_root: PathBuf) -> Self {
        Self {
            local,
            mirror: HerdrMirrors::new(socket_root),
        }
    }
}
impl NotificationSink for NotificationRouter {
    fn deliver(&self, message: &NotificationMessage) -> NotificationOutcome {
        match message.target {
            NotificationTarget::Local {} => self.local.deliver(message),
            NotificationTarget::HerdrMirror { .. } => self.mirror.deliver(message),
        }
    }
}

pub(crate) struct NotificationScheduler {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
impl NotificationScheduler {
    pub fn spawn(dispatcher: NotificationDispatcher, log: Arc<LogWriter>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = stop.clone();
        let worker = thread::spawn(move || {
            while !stopping.load(Ordering::Acquire) {
                match dispatcher.dispatch_once(&crate::schedule_scheduler::now_stored()) {
                    Ok(report) if report.submitted + report.failed + report.uncertain > 0 => {
                        let _ = log.append(&LogRecord::new(
                            LogLevel::Info,
                            "notifications",
                            format!(
                                "submitted {}, failed {}, uncertain {}",
                                report.submitted, report.failed, report.uncertain
                            ),
                        ));
                    }
                    Ok(_) => {}
                    Err(_) => {
                        let _ = log.append(&LogRecord::new(
                            LogLevel::Error,
                            "notifications",
                            "notification delivery pass failed",
                        ));
                    }
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
impl Drop for NotificationScheduler {
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
    use super::*;
    use kanban_app::notifications::{
        NotificationMessage, NotificationOutcome, NotificationPermissionPort, NotificationSink,
    };
    use kanban_dto::{NotificationPermissionState, NotificationTarget};
    use kanban_herdr::fixture::{ScriptedSession, SessionScript};

    #[cfg(target_os = "macos")]
    #[test]
    fn notification_no_ack_unbundled_native_process_is_not_reported_as_authorized() {
        let native = NativeNotifications::default();
        assert_eq!(
            native.status().state,
            NotificationPermissionState::Unavailable
        );
        let result = native.deliver(&NotificationMessage {
            delivery_id: 1,
            target: NotificationTarget::Local {},
            title: "Kanban probe".to_owned(),
            body: "An unbundled test must not send this".to_owned(),
        });
        assert!(matches!(result, NotificationOutcome::NotSent { .. }));
        assert!(native.request().is_err());
    }

    #[test]
    fn notification_no_ack_mirrors_use_the_exact_session_binding_without_waking() {
        for named in [true, false] {
            let dir = tempfile::TempDir::new().unwrap();
            let session = if named {
                ScriptedSession::bind(
                    dir.path(),
                    "attention-session",
                    "/workspaces/kanban.seed",
                    SessionScript::default().with_prompt_accepted(true),
                )
            } else {
                ScriptedSession::bind_default(
                    dir.path(),
                    "/workspaces/kanban.seed",
                    SessionScript::default().with_prompt_accepted(true),
                )
            };
            let sink = HerdrMirrors::new(dir.path().to_path_buf());
            let message = NotificationMessage {
                delivery_id: 7,
                title: "Kanban attention".to_owned(),
                body: "Information only. Open the Kanban Inbox.".to_owned(),
                target: NotificationTarget::HerdrMirror {
                    session_name: named.then(|| "attention-session".to_owned()),
                    product_workspace: "/workspaces/kanban.seed".to_owned(),
                    herdr_workspace: "kanban.seed".to_owned(),
                    role: "observer".to_owned(),
                },
            };
            assert!(matches!(
                sink.deliver(&message),
                NotificationOutcome::Submitted { .. }
            ));
            let frames = session.recorded_requests();
            assert!(frames.iter().any(|frame|matches!(frame,kanban_herdr::HerdrRequest::Prompt {role,message} if role=="observer" && message.contains("Information only"))));
            assert!(
                frames
                    .iter()
                    .all(|frame| !matches!(frame, kanban_herdr::HerdrRequest::Wake { .. }))
            );
        }
    }
}
