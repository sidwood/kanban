use crate::service_lifecycle::LoginLaunchPort;
use crate::{
    CommandEffects, CommandHandler, Core, ParsedCommand, QueryHandler, RegistrationError,
    parse_payload,
};
use kanban_dto::{
    ApiError, LoginLaunchChangeStatus, LoginLaunchQuery, LoginLaunchSetRequest,
    LoginLaunchSetResponse, LoginLaunchState,
};
use serde_json::Value;
use std::sync::{Arc, Mutex};

struct Registration {
    port: Arc<dyn LoginLaunchPort>,
    instance_id: String,
    outcome: Arc<Mutex<(u64, Option<String>)>>,
}
impl Core {
    pub fn register_login_launch(
        &mut self,
        port: Arc<dyn LoginLaunchPort>,
        instance_id: String,
    ) -> Result<(), RegistrationError> {
        let registration = Arc::new(Registration {
            port,
            instance_id,
            outcome: Arc::new(Mutex::new((0, None))),
        });
        self.register_query("service.login_launch.get", registration.clone())?;
        self.register_command("service.login_launch.set", registration)
    }
}
impl QueryHandler for Registration {
    fn handle(&self, payload: &Value) -> Result<Value, ApiError> {
        parse_payload::<LoginLaunchQuery>(payload)?;
        let (version, previous_error) = &*self.outcome.lock().unwrap_or_else(|p| p.into_inner());
        let (enabled, error) = match self.port.enabled() {
            Ok(enabled) => (Some(enabled), previous_error.clone()),
            Err(error) => (None, Some(error.message)),
        };
        serde_json::to_value(LoginLaunchState {
            instance_id: self.instance_id.clone(),
            version: *version,
            enabled,
            error,
        })
        .map_err(internal)
    }
}
impl CommandHandler for Registration {
    fn parse(&self, payload: &Value) -> Result<ParsedCommand, ApiError> {
        let request: LoginLaunchSetRequest = parse_payload(payload)?;
        if request.instance_id != self.instance_id {
            return Err(ApiError::invalid_request(
                "registration state belongs to a previous service instance; refresh before changing it",
            ));
        }
        ParsedCommand::lift("service_login_launch", payload)
    }
    fn current_version(&self, _: &ParsedCommand) -> Result<u64, ApiError> {
        Ok(self.outcome.lock().unwrap_or_else(|p| p.into_inner()).0)
    }
    fn apply(
        &self,
        command: &ParsedCommand,
        effects: &dyn CommandEffects,
    ) -> Result<Value, ApiError> {
        let request: LoginLaunchSetRequest = parse_payload(&command.payload)?;
        let port = self.port.clone();
        let outcome = self.outcome.clone();
        effects.after_commit(Box::new(move || {
            let mut state = outcome.lock().unwrap_or_else(|p| p.into_inner());
            state.0 += 1;
            state.1 = port
                .set_enabled(request.enabled)
                .and_then(|_| {
                    if port.enabled()? == request.enabled {
                        Ok(())
                    } else {
                        Err(ApiError::internal(
                            "native registration change was not observed",
                        ))
                    }
                })
                .err()
                .map(|error| error.message);
        }));
        serde_json::to_value(LoginLaunchSetResponse {
            status: LoginLaunchChangeStatus::ChangeRequested,
        })
        .map_err(internal)
    }
}
fn internal(error: impl std::fmt::Display) -> ApiError {
    ApiError::internal(&error.to_string())
}
