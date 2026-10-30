//! The MCP server: `rmcp`'s [`ServerHandler`] over a [`Backend`].
//!
//! Tool listing and calls go through [`crate::tools`]; this module only translates between the
//! protocol types and those plain values, and derives the [`Caller`] of each call from the
//! request (see [`crate::bind`]).

use std::sync::{Arc, Mutex};

use rmcp::ErrorData as McpError;
use rmcp::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeRequestParams, InitializeResult, ListToolsResult, MetaObject, PaginatedRequestParams,
    ServerCapabilities, ServerConfig, Tool, ToolAnnotations,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::{Map, Value};

use crate::backend::Backend;
use crate::bind::BindingSettings;
use crate::tools::{self, ToolDef, ToolOutput};

/// Antigravity loads MCP tools on demand unless the tool says otherwise (v1.3.3 and later).
const ANTIGRAVITY_ALWAYS_LOAD: &str = "antigravity/alwaysLoad";

/// What the server tells a model about itself at `initialize`. Short: it is spent context.
const INSTRUCTIONS: &str = "ACS coordinates the coding agents working in this repository. \
Claim files with acs_claim before you edit or delete them, say what you are doing with \
acs_intent, release files when done, and call acs_why when something is denied.";

/// How the server behaves.
#[derive(Debug, Clone, Default)]
pub struct ServerOptions {
    /// How calls are bound to a harness session.
    pub binding: BindingSettings,
    /// Also return each result as `structuredContent`. Off by default: Claude Code (2.1.314)
    /// shows the model the structured JSON *instead of* the text content when both are present,
    /// and the text is what is written for the agent. Turn it on for clients that read the
    /// structure.
    pub structured_content: bool,
}

/// The MCP server for one harness session.
pub struct AcsServer<B> {
    backend: Arc<B>,
    options: ServerOptions,
    client_name: Mutex<Option<String>>,
}

impl<B: Backend> AcsServer<B> {
    /// A server answering with `backend`.
    #[must_use]
    pub fn new(backend: B, options: ServerOptions) -> Self {
        Self {
            backend: Arc::new(backend),
            options,
            client_name: Mutex::new(None),
        }
    }

    fn client_name(&self) -> Option<String> {
        self.client_name.lock().ok().and_then(|name| name.clone())
    }

    fn remember_client(&self, name: &str) {
        if let Ok(mut slot) = self.client_name.lock() {
            *slot = Some(name.to_owned());
        }
    }
}

fn tool_of(def: &ToolDef) -> Tool {
    let mut annotations = ToolAnnotations::new()
        .read_only(def.read_only)
        .idempotent(def.idempotent)
        .open_world(false);
    if !def.read_only {
        annotations = annotations.destructive(false);
    }
    let mut meta = MetaObject::new();
    meta.0
        .insert(ANTIGRAVITY_ALWAYS_LOAD.to_owned(), Value::Bool(true));
    Tool::new(def.name, def.description, def.input_schema.clone())
        .with_annotations(annotations)
        .with_meta(meta)
}

/// The tool list as `tools/list` returns it.
#[must_use]
pub fn list_tools_result() -> ListToolsResult {
    ListToolsResult::with_all_items(tools::definitions().iter().map(tool_of).collect())
}

fn result_of(output: ToolOutput, structured: bool) -> CallToolResult {
    let content = vec![ContentBlock::text(output.text)];
    let mut result = if output.is_error {
        CallToolResult::error(content)
    } else {
        CallToolResult::success(content)
    };
    if structured {
        result.structured_content = output.structured;
    }
    result
}

/// The `_meta` of a request: the call's own, else the one the transport extracted.
fn request_meta(
    request: &CallToolRequestParams,
    context: &RequestContext<RoleServer>,
) -> Map<String, Value> {
    let mut meta = context.meta.0.0.clone();
    if let Some(own) = &request.meta {
        for (key, value) in &own.0.0 {
            meta.insert(key.clone(), value.clone());
        }
    }
    meta
}

impl<B: Backend> ServerHandler for AcsServer<B> {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("acs", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS)
    }

    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, McpError> {
        self.remember_client(&request.client_info.name);
        context.peer.set_peer_info(request.clone());
        self.negotiate_initialize(&request)
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        Ok(list_tools_result())
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let meta = request_meta(&request, &context);
        let caller = self
            .options
            .binding
            .caller(self.client_name().as_deref(), &meta);
        // The backend does short blocking work (SQLite, a few file reads); the server handles
        // one request at a time on a single thread, so there is nothing to starve.
        let output = tools::call(&*self.backend, &caller, &request.name, request.arguments);
        Ok(result_of(output, self.options.structured_content).into())
    }
}
