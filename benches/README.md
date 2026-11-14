# Benchmarks

Criterion benchmarks for the latency budgets of plan section 4.7, process start included (WP10.1).
They are a target of `acs-cli`, because a virtual workspace root can hold no targets:

```sh
cargo bench -p acs-cli --bench hook_paths
```

`hook_paths.rs` lists the settings. It also measures, without budgets, the tool calls that end
early because no switched-on feature needs them (decision 0035). A run fails when a budget is missed or, given a base build, when
a p99 is over 20% worse than that build's and the difference is over 15% of the path's p99 budget.
