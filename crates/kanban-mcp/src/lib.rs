//! Generated MCP tools; domain decisions remain in the application Core.
use kanban_app::{Core, catalog::exposed_operations};
use kanban_domain::CapabilityId;
use rmcp::{
    ErrorData as McpError, RoleServer, ServerHandler,
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
    },
    service::RequestContext,
};
use serde_json::Value;
use std::sync::Arc;

#[derive(Clone)]
pub struct Adapter {
    backend: Arc<Backend>,
}
enum Backend {
    Direct(kanban_app::agent_authorization::AgentSession),
    Channel(kanban_transport::agent::AgentClient),
}
impl Backend {
    fn operations(&self) -> Result<Vec<String>, kanban_dto::ApiError> {
        match self {
            Self::Direct(session) => session.operations(),
            Self::Channel(client) => client.operations(),
        }
    }
    fn call(&self, name: &str, payload: &Value) -> Result<Value, kanban_dto::ApiError> {
        match self {
            Self::Direct(session) => session.call(name, payload),
            Self::Channel(client) => client.call(name, payload),
        }
    }
}
impl Adapter {
    /// Only the trusted service chooses this binding, never MCP arguments.
    pub fn new(core: Arc<Core>, capability: CapabilityId) -> Self {
        Self {
            backend: Arc::new(Backend::Direct(
                kanban_app::agent_authorization::AgentSession::new(core, capability)
                    .expect("service selected a live run"),
            )),
        }
    }
    pub fn from_channel(channel: std::os::unix::net::UnixStream) -> Self {
        Self {
            backend: Arc::new(Backend::Channel(kanban_transport::agent::AgentClient::new(
                channel,
            ))),
        }
    }
    fn tools(&self) -> Result<Vec<Tool>, McpError> {
        let permitted = self
            .backend
            .operations()
            .map_err(|_| McpError::invalid_request("run access is unavailable", None))?;
        let generated: Vec<Tool> = serde_json::from_str(include_str!(
            "../../../packages/contracts/src/mcp-tools.json"
        ))
        .map_err(|_| McpError::internal_error("generated tools are invalid", None))?;
        Ok(generated
            .into_iter()
            .filter(|tool| {
                exposed_operations().iter().any(|op| {
                    op.mcp_tool_name == tool.name && permitted.iter().any(|name| name == op.name)
                })
            })
            .collect())
    }
}
impl ServerHandler for Adapter {
    async fn on_custom_request(
        &self,
        _request: rmcp::model::CustomRequest,
        _context: RequestContext<RoleServer>,
    ) -> Result<rmcp::model::CustomResult, McpError> {
        Err(McpError::new(
            rmcp::model::ErrorCode::METHOD_NOT_FOUND,
            "unsupported MCP method",
            None,
        ))
    }

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
    }
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        if request
            .as_ref()
            .is_some_and(|request| request.cursor.is_some())
        {
            return Err(McpError::invalid_params(
                "tool pagination is not supported",
                None,
            ));
        }
        Ok(ListToolsResult::with_all_items(self.tools()?).with_ttl_ms(0))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let Some(operation) = exposed_operations()
            .iter()
            .find(|op| op.mcp_tool_name == request.name)
        else {
            return Ok(CallToolResult::structured_error(
                serde_json::json!({"code":"not_found","message":"tool is unavailable"}),
            )
            .into());
        };
        let payload = Value::Object(request.arguments.unwrap_or_default());
        let outcome = self.backend.call(operation.name, &payload);
        // Do not reflect attacker-controlled field names or values into errors.
        Ok(match outcome {
            Ok(value)=>CallToolResult::structured(value),
            Err(error)=>CallToolResult::structured_error(serde_json::json!({"code":error.code,"message":"request refused by Core","current_version":error.current_version})),
        }.into())
    }
}
