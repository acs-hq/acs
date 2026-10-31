# Benchmarks

Criterion benchmarks for the latency budgets of plan section 4.7, process start included (WP10.1).
They are a target of `acs-cli`, because a virtual workspace root can hold no targets:

```sh
cargo bench -p acs-cli --bench hook_paths
```

`hook_paths.rs` lists the settings. A run fails when a budget is missed or, given a base build, when
a p99 is over 20% worse than that build's.
