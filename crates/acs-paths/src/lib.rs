//! ACS paths: canonicalization, symlinks, junctions and repo-escape detection.
//!
//! A [`PathResolver`] is built for one repository. It turns the raw path an agent wrote (absolute
//! or relative, with `..`, links, junctions, mixed separators, the wrong case) into the
//! [`ResolvedPath`](acs_core::gate::ResolvedPath) the decision engine consumes: in the repository
//! as a canonical [`RepoPath`](acs_core::RepoPath), outside it, or unresolvable.
//!
//! - [`PathResolver::resolve`] follows every link, like a write or read would.
//!   [`PathResolver::resolve_entry`] leaves a link in the last component alone, like `rm` and
//!   `mv` see it.
//! - [`PathResolver::resolve_recursive`] looks inside a directory a recursive operation would
//!   enter and reports the repository paths its links reach (a scratch copy holding a junction to
//!   the live tree).
//! - [`PathResolver::expand_recursive`] and [`PathResolver::expand_glob`] list the repository
//!   files a recursive delete or a glob reaches, capped.
//!
//! Resolution walks the path one component at a time so that `a/link/../b` follows the link
//! before the `..` is applied, and it starts below the (already canonical) repository root, so
//! an ordinary in-repository path costs one `lstat` per component.

mod glob;
mod names;
mod reach;
mod resolver;

pub use glob::has_glob_meta;
pub use reach::RecursiveResolution;
pub use resolver::{PathResolver, PathsError};
