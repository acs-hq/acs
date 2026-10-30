//! Payloads no adapter could parse (SPEC section 11): allow, keep the raw payload, warn once per
//! harness session.

use std::path::Path;

use acs_adapters::{Adapter, AdapterError};
use acs_core::{Decision, Outcome, ReasonCode, Timestamp};

use super::markers::save_unknown_payload;
use crate::runtime::{OpenError, RepoContext};

/// Handles a payload `adapter` could not parse. `home` is `ACS_HOME`'s resolution.
pub fn handle(
    adapter: &dyn Adapter,
    error: &AdapterError,
    payload: &[u8],
    cwd: &Path,
    home: Result<acs_store::AcsHome, acs_store::HomeError>,
    now: Timestamp,
) -> Decision {
    let mut decision = Decision::new(Outcome::Allow, ReasonCode::UnknownPayload, "");
    let warning = format!(
        "ACS could not read this {} hook payload ({}), so the call was not checked. The payload \
         was saved for `acs doctor`.",
        adapter.name(),
        error.message
    );
    let ctx = match RepoContext::open_with(cwd, home) {
        Ok(ctx) => ctx,
        Err(OpenError::NotARepo(_)) => return decision,
        Err(_) => {
            decision.warnings.push(warning);
            return decision;
        }
    };
    save_unknown_payload(&ctx, now, payload);
    let first = match (&error.harness_session_id, ctx.db()) {
        // Once per harness session: a flag records that this session has been told.
        (Some(session), Ok(db)) => {
            let key = format!("unknown_warned:{}:{session}", adapter.name());
            match db.flag(&key) {
                Ok(None) => {
                    let _ = db.set_flag(&key, "1", now);
                    true
                }
                Ok(Some(_)) => false,
                Err(_) => true,
            }
        }
        _ => true,
    };
    if first {
        decision.warnings.push(warning);
    }
    decision
}
