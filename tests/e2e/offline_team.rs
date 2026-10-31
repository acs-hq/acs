//! A team whose remote goes away and comes back: local claims never stop working, claims from the
//! other machine are not held against anyone once they cannot be trusted, and everything joins up
//! again after the next sync.

use std::fs;

use acs_core::{Outcome, ReasonCode};

use crate::common::team::{Team, wait_for};

#[test]
fn a_team_keeps_working_through_an_outage_and_joins_up_afterwards() {
    let team = Team::new();
    let sara = team.clone("sara", "sara@example.com");
    let omar = team.clone("omar", "omar@example.com");

    // Online: a claim on one machine blocks the other after a sync.
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    assert!(sara.acs(&["sync"]).status.success());
    assert!(omar.acs(&["sync"]).status.success());
    assert!(omar.sees_remote_claim("src/lib.rs"));
    let d = omar.edit("o1", "src/lib.rs");
    assert_eq!(
        (d.outcome, d.reason_code),
        (Outcome::Deny, ReasonCode::ClaimConflict)
    );

    // The remote disappears.
    let parked = team.base.join("remote-offline.git");
    fs::rename(&team.remote, &parked).unwrap();
    assert!(!omar.acs(&["sync"]).status.success());
    assert!(!sara.acs(&["sync"]).status.success());

    // Each machine still enforces its own seats, and says the shared view cannot be trusted.
    wait_for("the view to go stale, after three sync intervals", || {
        omar.acs(&["sync"]);
        omar.status()["remote"]["stale"] == true
    });
    assert_eq!(omar.edit("o1", "src/util.rs").outcome, Outcome::Allow);
    assert_eq!(omar.edit("o2", "src/util.rs").outcome, Outcome::Deny);

    // The stale view fails open: Omar may work on the file, with a warning.
    let d = omar.edit("o2", "src/lib.rs");
    assert_eq!(d.outcome, Outcome::Allow, "{d:?}");
    assert!(
        d.warnings.iter().any(|w| w.contains("out of date")),
        "{d:?}"
    );

    // The remote returns: the next sync of each machine publishes what happened meanwhile, and the
    // earlier of the two claims on `src/lib.rs` keeps the file (SPEC section 6).
    fs::rename(&parked, &team.remote).unwrap();
    assert!(sara.acs(&["sync"]).status.success());
    assert!(omar.acs(&["sync"]).status.success());
    assert!(sara.acs(&["sync"]).status.success());
    wait_for("Sara sees Omar's claim", || {
        sara.sees_remote_claim("src/util.rs")
    });
    let view = omar.status();
    assert_eq!(view["remote"]["stale"], false, "{view}");
    assert_eq!(sara.edit("s1", "src/lib.rs").outcome, Outcome::Allow);
    assert_eq!(omar.edit("o2", "src/lib.rs").outcome, Outcome::Deny);
}
