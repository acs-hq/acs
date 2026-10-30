//! ACS MCP: the MCP server over stdio.
//!
//! The server is backend-agnostic. It defines the five tools (`acs_status`, `acs_claim`,
//! `acs_release`, `acs_intent`, `acs_why`) and speaks the protocol; what they do is behind the
//! [`Backend`] trait, which `acs-cli` implements with the shared runtime operations (decision
//! 0011 section 1) and the tests implement with a fake.
//!
//! - [`backend`]: the trait and its typed requests and replies.
//! - [`bind`]: what a tool call tells us about the harness session behind it (Claude Code's
//!   environment, Antigravity's per-call `_meta`).
//! - [`tools`]: tool definitions, dispatch and the text of results, independent of the protocol
//!   library.
//! - [`server`]: the `rmcp` handler.
//!
//! [`serve_stdio`] is the only entry point that starts an async runtime; the `acs hook` path
//! never calls it.

pub mod backend;
pub mod bind;
pub mod server;
pub mod tools;

use rmcp::ServiceExt;

pub use backend::{
    Backend, BackendError, Caller, ClaimInfo, ClaimReply, ClaimRequest, DecisionInfo, IntentReply,
    IntentRequest, ReleaseReply, ReleaseRequest, SeatInfo, StatusReply, WhyReply,
};
pub use bind::BindingSettings;
pub use server::{AcsServer, ServerOptions};

/// Why [`serve_stdio`] ended abnormally.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The async runtime could not be created.
    #[error("cannot start the async runtime: {0}")]
    Runtime(#[from] std::io::Error),
    /// The MCP handshake failed (the client closed the pipe, or sent something that is not MCP).
    #[error("MCP initialization failed: {0}")]
    Init(String),
    /// The session ended with an error.
    #[error("MCP session failed: {0}")]
    Session(String),
}

/// Serves MCP over stdin and stdout until the client closes the pipe.
///
/// Nothing else may write to stdout while this runs: it carries the protocol. Blocks the calling
/// thread; builds a single-threaded runtime of its own.
///
/// # Errors
///
/// [`ServeError`] when the runtime cannot start or the session fails.
pub fn serve_stdio<B: Backend>(backend: B, options: ServerOptions) -> Result<(), ServeError> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let server = AcsServer::new(backend, options);
        let running = server
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|err| ServeError::Init(err.to_string()))?;
        running
            .waiting()
            .await
            .map(|_| ())
            .map_err(|err| ServeError::Session(err.to_string()))
    })
}
