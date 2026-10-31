//! One sync: fetch every pilot's published state, rebuild the remote-claims view, publish this
//! machine's claims, push (SPEC section 8).
//!
//! The fetch comes first because publishing needs it. One pilot on two machines shares one
//! claims ref, and its document must carry the claims of both, so each machine republishes what
//! the other last wrote; publishing before fetching would erase those.
//!
//! The view is fresh as soon as the fetch has worked. A failure after that (a pilot with no push
//! access, say) is recorded and reported, but it does not make the claims just fetched stale.

use std::collections::BTreeMap;

use acs_core::{Claim, ClaimsDocument, SpecVersion, Timestamp};
use acs_store::{StateDb, StoreError};
use serde::Serialize;

use crate::GitError;
use crate::refs::{Pilot, PilotDocument, SyncStorage};

/// What a sync needs to know.
#[derive(Debug, Clone, Copy)]
pub struct SyncOptions<'a> {
    /// The pilot running the sync.
    pub pilot: &'a str,
    /// The time of the sync.
    pub now: Timestamp,
}

/// What a sync did.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize)]
pub struct SyncReport {
    /// How many pilots have live claims on other machines.
    pub remote_pilots: usize,
    /// How many live claims other machines hold.
    pub remote_claims: usize,
    /// How many claims documents were rewritten. Zero when nothing changed that others need to
    /// know: see [`needs_publishing`].
    pub documents_published: usize,
    /// How many refs were pushed.
    pub refs_pushed: usize,
    /// Fetched documents that could not be used. Their pilots' last good claims stay in the view
    /// until they expire.
    pub warnings: Vec<String>,
}

/// Why a sync failed.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    /// `git` failed, or the remote could not be reached.
    #[error(transparent)]
    Git(#[from] GitError),
    /// The state database failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// A claims document could not be written.
    #[error("cannot write the claims document: {0}")]
    Encode(#[from] serde_json::Error),
}

/// Runs one sync. A failure is also recorded in `db`, where `acs status` shows it.
///
/// # Errors
///
/// [`SyncError`], with the state database already updated.
pub fn run(
    storage: &dyn SyncStorage,
    db: &StateDb,
    options: &SyncOptions<'_>,
) -> Result<SyncReport, SyncError> {
    let outcome = exchange(storage, db, options);
    if let Err(err) = &outcome {
        // Best effort: the error itself is what the caller needs.
        let _ = db.record_sync_failure(options.now, &err.to_string());
    }
    outcome
}

fn exchange(
    storage: &dyn SyncStorage,
    db: &StateDb,
    options: &SyncOptions<'_>,
) -> Result<SyncReport, SyncError> {
    let mut report = SyncReport::default();
    storage.fetch()?;
    let fetched = rebuild_view(storage, db, options.now, &mut report)?;
    db.record_sync_success(options.now)?;

    let (pilots, published) = publish_claims(storage, db, options, &fetched, &mut report)?;
    for pilot in &pilots {
        storage.reconcile_ledger(pilot)?;
    }
    report.refs_pushed = storage.push(&published, &pilots)?;
    Ok(report)
}

/// A fetched document, if it is one and its pilot is who the ref says. A pusher could otherwise
/// put words in someone else's mouth: agents are told the name of the holder of a claim.
fn parse_document(document: &PilotDocument) -> Result<ClaimsDocument, String> {
    let parsed: ClaimsDocument = serde_json::from_slice(&document.bytes)
        .map_err(|err| format!("not a claims document: {err}"))?;
    let owner = &document.pilot_id;
    if &acs_core::pilot_id(&parsed.pilot) != owner
        || parsed
            .claims
            .iter()
            .any(|claim| &acs_core::pilot_id(&claim.pilot) != owner)
    {
        return Err("it names a pilot other than the owner of its ref".to_owned());
    }
    Ok(parsed)
}

/// Replaces the remote-claims view with the fetched documents, and returns those by pilot id.
///
/// Claims of seats on this machine are left out, whichever pilot they are under: the local table
/// is their truth, and a copy fetched back would outlive a release and block its own holder.
fn rebuild_view(
    storage: &dyn SyncStorage,
    db: &StateDb,
    now: Timestamp,
    report: &mut SyncReport,
) -> Result<BTreeMap<String, ClaimsDocument>, SyncError> {
    let documents = storage.fetched_claims()?;
    let mut fetched = BTreeMap::new();
    for document in &documents {
        let parsed = match parse_document(document) {
            Ok(parsed) => parsed,
            Err(why) => {
                let id = &document.pilot_id;
                report
                    .warnings
                    .push(format!("ignored the claims of pilot {id}: {why}"));
                continue;
            }
        };
        let mut others = Vec::new();
        for claim in &parsed.claims {
            if db.get_seat(&claim.seat_id)?.is_none() {
                others.push(claim.clone());
            }
        }
        db.replace_remote_claims(&document.pilot_id, &others, now)?;
        let live = others.iter().filter(|c| !c.is_expired(now)).count();
        report.remote_claims += live;
        report.remote_pilots += usize::from(live > 0);
        fetched.insert(document.pilot_id.clone(), parsed);
    }
    let ids: Vec<&str> = documents.iter().map(|d| d.pilot_id.as_str()).collect();
    db.retain_remote_pilots(&ids)?;
    Ok(fetched)
}

/// Whether the claims differ from what is published in a way others need to know.
///
/// A heartbeat moves on every tool call, and publishing each one would mean a push every interval
/// for as long as anyone works. One only counts once it has moved a quarter of the claim's TTL:
/// the published claim then always outlives its last publication by three quarters of the TTL,
/// which is many sync intervals, and it still expires soon after its holder dies.
#[must_use]
pub fn needs_publishing(published: Option<&ClaimsDocument>, claims: &[Claim]) -> bool {
    let Some(published) = published else {
        return !claims.is_empty();
    };
    published.claims.len() != claims.len()
        || claims.iter().any(|claim| {
            published
                .claims
                .iter()
                .find(|old| old.claim_id == claim.claim_id)
                .is_none_or(|old| {
                    let moved = claim.heartbeat_at.duration_since(old.heartbeat_at);
                    old.scope != claim.scope
                        || old.target_branch != claim.target_branch
                        || old.ttl_secs != claim.ttl_secs
                        || moved.as_secs().unsigned_abs() >= claim.ttl_secs / 4
                })
        })
}

/// Writes the claims documents that changed. Returns every pilot with claims on this machine,
/// plus the one running the sync, and those of them whose document was written.
fn publish_claims(
    storage: &dyn SyncStorage,
    db: &StateDb,
    options: &SyncOptions<'_>,
    fetched: &BTreeMap<String, ClaimsDocument>,
    report: &mut SyncReport,
) -> Result<(Vec<Pilot>, Vec<Pilot>), SyncError> {
    let own = Pilot::new(options.pilot);
    let mut local: BTreeMap<String, (Pilot, Vec<Claim>)> = BTreeMap::new();
    local.insert(own.id.clone(), (own, Vec::new()));
    for held in db.live_claims(options.now)? {
        let pilot = Pilot::new(&held.claim.pilot);
        let entry = local
            .entry(pilot.id.clone())
            .or_insert_with(|| (pilot, Vec::new()));
        entry.1.push(held.claim);
    }

    let mut pilots = Vec::new();
    let mut published = Vec::new();
    for (id, (pilot, mut claims)) in local {
        let previous = fetched.get(&id);
        // What the same pilot holds on another machine stays in the document.
        for claim in previous.iter().flat_map(|doc| &doc.claims) {
            if !claim.is_expired(options.now) && db.get_seat(&claim.seat_id)?.is_none() {
                claims.push(claim.clone());
            }
        }
        claims.sort_by(|a, b| a.claim_id.as_str().cmp(b.claim_id.as_str()));
        if needs_publishing(previous, &claims) {
            let document = ClaimsDocument {
                spec: SpecVersion,
                pilot: pilot.email.clone(),
                updated_at: options.now,
                claims,
            };
            storage.write_claims(&pilot, &serde_json::to_vec_pretty(&document)?)?;
            report.documents_published += 1;
            published.push(pilot.clone());
        }
        pilots.push(pilot);
    }
    Ok((pilots, published))
}
