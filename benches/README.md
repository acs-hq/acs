# Benchmarks

Criterion benchmarks for the latency budgets of plan section 4.7, process start included (WP10.1).
They are a target of `acs-cli`, because a virtual workspace root can hold no targets:

```sh
cargo bench -p acs-cli --bench hook_paths
```

`hook_paths.rs` lists the settings. It also measures, without budgets, the tool calls that end
early because no switched-on feature needs them (decision 0035). A run fails when a budget is missed or, given a base build, when
a p99 is over 20% worse than that build's and the difference is over 15% of the path's p99 budget.

The pre-tool hook is also measured in a repository with 50 lore entries (decision 0049): an edit and
a read whose entries were already shown and an edit outside every scope, all three under the
pre-tool budget, and, without budgets, the first read of a new session in a scope (with the
staleness cache warm, and right after a commit) and the end of a turn (`turn_end`).
`ACS_PERF_LORE_ENTRIES` changes the number of entries.

A full run takes about 20 minutes on Windows, nearly all of it `git add` of the 100,000-file
repository. `ACS_PERF_FILES=2000` is for trying a change, not for reported numbers. Report numbers
from this command, which builds in release mode; debug-build timings are not comparable.
