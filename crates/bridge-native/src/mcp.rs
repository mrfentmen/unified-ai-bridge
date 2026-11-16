use std::sync::Arc;

use anyhow::Result;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
    ToolAnnotations,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServiceExt};
use serde_json::Value;

use bridge_core::provider::Agent;

use crate::app::App;
use crate::tools;

pub struct BridgeMcp {
    app: Arc<App>,
    agent: Agent,
}

impl BridgeMcp {
    pub fn new(app: Arc<App>, agent: Agent) -> Self {
        Self { app, agent }
    }

    fn tool_definitions(&self) -> Vec<Tool> {
        self.app
            .workspace
            .profile
            .tools()
            .map(|spec| {
                let schema = serde_json::from_str(spec.schema)
                    .unwrap_or_else(|_| Value::Object(Default::default()));
                let schema = schema.as_object().cloned().unwrap_or_default();
                let mut tool = Tool::default();
                tool.name = spec.name.into();
                tool.title = Some(spec.name.replace('_', " "));
                tool.description = Some(spec.description.into());
                tool.input_schema = Arc::new(schema);
                tool.annotations = Some(
                    ToolAnnotations::new()
                        .read_only(!spec.writes)
                        .destructive(spec.writes)
                        .open_world(false),
                );
                tool
            })
            .collect()
    }
}

impl ServerHandler for BridgeMcp {
    async fn initialize(
        &self,
        _request: rmcp::model::InitializeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        Ok(self.get_info())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(ListToolsResult::with_all_items(self.tool_definitions()))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let arguments = request
            .arguments
            .map(Value::Object)
            .unwrap_or(Value::Object(Default::default()));
        let result = tools::dispatch(&self.app, self.agent, "mcp", &request.name, arguments).await;
        let text = if result.status == "ok" {
            let summary = result.summary.unwrap_or_default();
            let output = result.output.unwrap_or_default();
            format!("{summary}\n\n{output}")
        } else {
            format!(
                "{}: {}",
                result.code.unwrap_or_else(|| "error".into()),
                result.error.unwrap_or_else(|| "unknown error".into())
            )
        };
        let response: CallToolResponse = if result.status == "ok" {
            CallToolResult::success(vec![ContentBlock::text(text)]).into()
        } else {
            CallToolResult::error(vec![ContentBlock::text(text)]).into()
        };
        Ok(response)
    }

    fn get_info(&self) -> InitializeResult {
        let mut info = InitializeResult::default();
        info.server_info = Implementation::new("unified-ai-bridge", env!("CARGO_PKG_VERSION"));
        info.capabilities = ServerCapabilities::builder().enable_tools().build();
        info.instructions = Some(format!("Unified AI Bridge MCP server. Fixed identity {}. Profile {}. Root {}. Read-only {}. Protected operations fail closed because this stdio process has no approval UI. Use only the tools advertised for this profile.",self.agent.dir(),self.app.workspace.profile.name(),self.app.workspace.root.display(),self.app.workspace.read_only));
        info
    }
}

pub async fn run(app: Arc<App>, agent: Agent) -> Result<()> {
    let service = BridgeMcp::new(app, agent)
        .serve(rmcp::transport::stdio())
        .await?;
    service.waiting().await?;
    Ok(())
}
