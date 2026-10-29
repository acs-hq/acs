//! Turns the event's targets into the flat list of repository paths the guards, claims and
//! snapshot steps work on.

use std::collections::HashMap;
use std::collections::HashSet;
use std::str::FromStr;

use crate::gate::{
    GateInput, PathLocation, PolicyView, ResolvedPath, SnapshotRequest, Target, TargetOp,
};
use crate::policy_types::ZoneKind;
use crate::values::RepoPath;

/// Why a path is sealed.
#[derive(Debug, Clone, Copy)]
pub(super) struct Sealed<'p> {
    /// The glob or zone pattern that matched.
    pub pattern: &'p str,
    /// Whether it is a `sealed` zone of `territory.toml` rather than a `guards.toml` glob.
    pub zone: bool,
}

/// One repository path the event writes or deletes.
#[derive(Debug)]
pub(super) struct Touch<'a, 'p> {
    pub path: &'a RepoPath,
    /// Whether the event deletes it (as opposed to writing it).
    pub delete: bool,
    /// A directory that a recursive operation reaches into: only checked against sealed globs,
    /// because the files inside it are listed separately (or cannot be listed at all).
    pub dir: bool,
    /// Set when the path is sealed.
    pub sealed: Option<Sealed<'p>>,
    /// Whether the path lies in an `open` zone (no claim, never "outside claims").
    pub open: bool,
}

/// The event's targets, flattened and checked against the policy's zones.
#[derive(Debug, Default)]
pub(super) struct Plan<'a, 'p> {
    /// Repository paths written or deleted, each once, in order of appearance.
    pub touches: Vec<Touch<'a, 'p>>,
    /// Write and delete targets outside the repository.
    pub escapes: Vec<&'a ResolvedPath>,
    /// Pre-images to capture.
    pub snapshots: Vec<SnapshotRequest>,
    /// A recursive delete inside the repository whose files the pipeline did not list.
    pub unlisted_delete: bool,
    /// A delete whose file listing stopped early.
    pub truncated_delete: bool,
    /// Whether any target deletes something, anywhere.
    pub any_delete: bool,
}

impl<'a, 'p> Plan<'a, 'p> {
    pub(super) fn build(input: &GateInput<'a>, policy: &'p dyn PolicyView) -> Self {
        let mut builder = Builder {
            plan: Self::default(),
            policy,
            index: HashMap::new(),
            snapshotted: HashSet::new(),
        };
        for target in input.targets {
            if target.op != TargetOp::Read {
                builder.target(target);
            }
        }
        builder.plan
    }
}

struct Builder<'a, 'p> {
    plan: Plan<'a, 'p>,
    policy: &'p dyn PolicyView,
    /// `(path, dir)` to the index in `plan.touches`.
    index: HashMap<(&'a RepoPath, bool), usize>,
    snapshotted: HashSet<(&'a RepoPath, bool)>,
}

impl<'a, 'p> Builder<'a, 'p> {
    fn target(&mut self, target: &'a Target) {
        let delete = target.op == TargetOp::Delete;
        self.plan.any_delete |= delete;
        match &target.path.location {
            PathLocation::InRepo(path) => match &target.expansion {
                Some(expansion) => {
                    if target.recursive {
                        self.touch(path, delete, true);
                        self.snapshot(path, true);
                    }
                    for file in &expansion.files {
                        self.touch(file, delete, false);
                        if !target.recursive {
                            self.snapshot(file, false);
                        }
                    }
                    self.plan.truncated_delete |= delete && expansion.truncated;
                }
                None if target.recursive && delete => {
                    self.touch(path, true, true);
                    self.snapshot(path, true);
                    self.plan.unlisted_delete = true;
                }
                None => {
                    self.touch(path, delete, false);
                    self.snapshot(path, target.recursive);
                }
            },
            PathLocation::Outside(_) | PathLocation::Unresolvable(_) => {
                self.plan.escapes.push(&target.path);
            }
        }
        for linked in &target.path.linked_into_repo {
            self.touch(linked, delete, false);
            self.snapshot(linked, target.recursive);
        }
    }

    fn touch(&mut self, path: &'a RepoPath, delete: bool, dir: bool) {
        if let Some(&at) = self.index.get(&(path, dir)) {
            self.plan.touches[at].delete |= delete;
            return;
        }
        let zone = self.policy.zone(path);
        let mut sealed = self.sealed(path, zone.map(|z| (z.kind, z.pattern)));
        if sealed.is_none() && dir {
            // A recursive operation on `.git` reaches `.git/**` even if the glob engine does not
            // treat the directory itself as matching `.git/**`.
            if let Ok(probe) = RepoPath::from_str(&format!("{path}/x")) {
                sealed = self.sealed(
                    &probe,
                    self.policy.zone(&probe).map(|z| (z.kind, z.pattern)),
                );
            }
        }
        self.index.insert((path, dir), self.plan.touches.len());
        self.plan.touches.push(Touch {
            path,
            delete,
            dir,
            sealed,
            open: zone.is_some_and(|z| z.kind == ZoneKind::Open),
        });
    }

    fn sealed(&self, path: &RepoPath, zone: Option<(ZoneKind, &'p str)>) -> Option<Sealed<'p>> {
        if let Some(pattern) = self.policy.sealed_match(path) {
            return Some(Sealed {
                pattern,
                zone: false,
            });
        }
        match zone {
            Some((ZoneKind::Sealed, pattern)) => Some(Sealed {
                pattern,
                zone: true,
            }),
            _ => None,
        }
    }

    fn snapshot(&mut self, path: &'a RepoPath, recursive: bool) {
        if self.snapshotted.insert((path, recursive)) {
            self.plan.snapshots.push(SnapshotRequest {
                path: path.clone(),
                recursive,
            });
        }
    }
}
