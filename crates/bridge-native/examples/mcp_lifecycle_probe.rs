//! Test-only RMCP server for lifecycle/fault tests. Not a provider integration
//! or installed product binary. Markers identify actual starts and child PIDs.
use rmcp::{
    handler::server::ServerHandler, model::*, service::RequestContext, ErrorData, RoleServer,
    ServiceExt,
};
use std::{path::PathBuf, sync::Arc};
struct Probe {
    root: PathBuf,
    mode: String,
}
impl ServerHandler for Probe {
    async fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        if self.mode == "discovery" {
            std::fs::write(self.root.join("discovering"), "started").unwrap();
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        }
        let mut tool = Tool::default();
        tool.name = "wait".into();
        tool.input_schema = Arc::new(
            serde_json::json!({"type":"object","properties":{},"additionalProperties":false})
                .as_object()
                .unwrap()
                .clone(),
        );
        Ok(ListToolsResult::with_all_items(vec![tool]))
    }
    async fn call_tool(
        &self,
        _: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        std::fs::write(self.root.join("called"), "started").unwrap();
        // Inherits the server's process group, as ordinary child processes do.
        let child = std::process::Command::new("/bin/sleep")
            .arg("30")
            .spawn()
            .unwrap();
        std::fs::write(self.root.join("child.pid"), child.id().to_string()).unwrap();
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        std::fs::write(self.root.join("completed"), "finished").unwrap();
        Ok(CallToolResult::success(vec![ContentBlock::text("finished")]).into())
    }
    fn get_info(&self) -> InitializeResult {
        let mut i = InitializeResult::default();
        i.server_info = Implementation::new("lifecycle-test-fixture", "0.0.0");
        i.capabilities = ServerCapabilities::builder().enable_tools().build();
        i
    }
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().expect("marker directory"));
    let mode = args.next().unwrap_or_default();
    std::fs::write(root.join("server.pid"), std::process::id().to_string())?;
    if mode == "initialize" {
        std::fs::write(root.join("initializing"), "started")?;
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    }
    Probe { root, mode }
        .serve(rmcp::transport::stdio())
        .await?
        .waiting()
        .await?;
    Ok(())
}
