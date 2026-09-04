use schemars::schema::RootSchema;
use schemars::schema_for;

use crate::error::{ApiError, ErrorCode};
use crate::event::EventEnvelope;
use crate::health::{HealthQuery, HealthResponse};
use crate::mutation::MutationContext;

/// Every DTO schema exported to `packages/contracts`.
pub fn schema_definitions() -> Vec<(&'static str, RootSchema)> {
    vec![
        ("ApiError", schema_for!(ApiError)),
        ("ErrorCode", schema_for!(ErrorCode)),
        ("EventEnvelope", schema_for!(EventEnvelope)),
        ("HealthQuery", schema_for!(HealthQuery)),
        ("HealthResponse", schema_for!(HealthResponse)),
        ("MutationContext", schema_for!(MutationContext)),
    ]
}
