//! rmcp server surface: the MCP service wiring the charger client, the
//! bounded session log, tools, resources, and prompts (Python `server.py`).
//!
//! The service construction takes a dynamically dispatched
//! [`Transport`] backend, so the real BLE stack
//! and the deterministic fake backend are chosen at the binary boundary while
//! everything above stays statically dispatched.

pub mod prompts;
pub mod resources;
pub mod tools;

use std::future::{Future, ready};
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsResult,
    ListResourcesResult, PaginatedRequestParams, ReadResourceRequestParams, ReadResourceResponse,
    ReadResourceResult, ResourceContents, ServerCapabilities, ServerConfig,
};
use rmcp::service::MaybeSendFuture;
use rmcp::service::RequestContext;
use rmcp::service::RoleServer;
use rmcp::{ServerHandler, tool_handler};
use serde_json::Value;

use crate::client::ChargerClient;
use crate::session::SessionLog;
use crate::transport::Transport;

/// Server name reported in `initialize` (`serverInfo.name`).
pub const SERVER_NAME: &str = "mc3000-mcp";

/// Error message for an unknown resource URI (Python `ValueError` text).
const UNKNOWN_RESOURCE: &str = "unknown resource: ";

/// Dynamically dispatched transport backend shared behind the MCP service.
#[derive(Clone)]
pub struct SharedTransport(Arc<dyn Transport>);

impl SharedTransport {
    /// Wrap a backend handle.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self(transport)
    }
}

#[async_trait]
impl Transport for SharedTransport {
    async fn scan(
        &self,
        timeout: std::time::Duration,
    ) -> Result<Vec<crate::transport::ScanDevice>, crate::transport::TransportError> {
        self.0.scan(timeout).await
    }

    async fn connect(&self, address: &str) -> Result<(), crate::transport::TransportError> {
        self.0.connect(address).await
    }

    async fn disconnect(&self) -> Result<(), crate::transport::TransportError> {
        self.0.disconnect().await
    }

    async fn is_connected(&self) -> bool {
        self.0.is_connected().await
    }

    async fn write(&self, data: &[u8]) -> Result<(), crate::transport::TransportError> {
        self.0.write(data).await
    }

    async fn next_notification(&self) -> Option<Vec<u8>> {
        self.0.next_notification().await
    }

    fn clear_pending(&self) {
        self.0.clear_pending();
    }

    fn notification_dropped_count(&self) -> u64 {
        self.0.notification_dropped_count()
    }
}

impl std::fmt::Debug for SharedTransport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SharedTransport")
            .finish_non_exhaustive()
    }
}

/// The MCP service: all charger tools over one serialized client, plus the
/// bounded audit log, resources, and prompts.
///
/// Clone freely; the client, log, and tool router are shared behind `Arc`s
/// (the `ChargerClient` already serializes concurrent operations internally).
#[derive(Debug, Clone)]
pub struct McpService {
    client: Arc<ChargerClient<SharedTransport>>,
    session: Arc<SessionLog>,
    tool_router: ToolRouter<Self>,
}

impl McpService {
    /// Build the service over a transport backend with a default session log.
    #[must_use]
    pub fn new(transport: Arc<dyn Transport>) -> Self {
        Self::with_session(transport, Arc::new(SessionLog::default()))
    }

    /// Build the service over a transport backend and an existing session log.
    #[must_use]
    pub fn with_session(transport: Arc<dyn Transport>, session: Arc<SessionLog>) -> Self {
        Self {
            client: Arc::new(ChargerClient::new(SharedTransport::new(transport))),
            session,
            tool_router: Self::tool_router(),
        }
    }

    /// The shared session log (audit export and the `session-log/latest`
    /// resource).
    #[must_use]
    pub fn session(&self) -> &Arc<SessionLog> {
        &self.session
    }

    /// The serialized charger client (diagnostics and scripted backends).
    #[must_use]
    pub fn client(&self) -> &ChargerClient<SharedTransport> {
        &self.client
    }
}

/// Route `tools/list` and `tools/call` through the generated tool router.
// rmcp 3.7 generates an immediately-ready async trait method; newer Clippy
// flags that generated code until the SDK changes its macro output.
#[allow(unknown_lints)]
#[allow(clippy::unused_async_trait_impl)]
#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpService {
    /// `initialize` response: name, `CARGO_PKG_VERSION`, and the
    /// tools/resources/prompts capabilities.
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(
            ServerCapabilities::builder()
                .enable_prompts()
                .enable_resources()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new(SERVER_NAME, env!("CARGO_PKG_VERSION")))
    }

    /// List the five `charger://` resources.
    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        ready(Ok(ListResourcesResult::with_all_items(resources::list())))
    }

    /// Read one resource payload as pretty JSON.
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResponse, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        ready(self.read_resource_payload(request))
    }

    /// List the five safe-charging prompts.
    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        ready(Ok(ListPromptsResult::with_all_items(prompts::list())))
    }

    /// Return one prompt as a single user text message.
    fn get_prompt(
        &self,
        request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResponse, rmcp::ErrorData>> + MaybeSendFuture + '_
    {
        ready(Self::prompt_payload(&request))
    }
}

impl McpService {
    /// Synchronously build a resource response for [`ServerHandler::read_resource`].
    fn read_resource_payload(
        &self,
        request: ReadResourceRequestParams,
    ) -> Result<ReadResourceResponse, rmcp::ErrorData> {
        let Some(payload) = resources::read(&request.uri, &self.session) else {
            return Err(rmcp::ErrorData::internal_error(
                format!("{UNKNOWN_RESOURCE}{}", request.uri),
                None,
            ));
        };
        let text = payload_text(&payload)?;
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, request.uri).with_mime_type("application/json"),
        ])
        .into())
    }

    /// Synchronously build a prompt response for [`ServerHandler::get_prompt`].
    fn prompt_payload(
        request: &GetPromptRequestParams,
    ) -> Result<GetPromptResponse, rmcp::ErrorData> {
        match prompts::get(&request.name) {
            Some(result) => Ok(result.into()),
            None => Err(rmcp::ErrorData::internal_error(
                format!("unknown prompt: {}", request.name),
                None,
            )),
        }
    }
}

/// Pretty-print a resource payload (`json.dumps(..., indent=2)` shape).
fn payload_text(payload: &Value) -> Result<String, rmcp::ErrorData> {
    serde_json::to_string_pretty(payload)
        .map_err(|error| rmcp::ErrorData::internal_error(error.to_string(), None))
}
