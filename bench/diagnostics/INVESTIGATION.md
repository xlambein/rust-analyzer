# #18997: investigation and first implementation

Current base: `rust-lang/rust-analyzer` `master` at
`114dcb8d6f50aca1ef14d67b831a7560362f8f69` (2026-11-13).
The two commits rebased without conflicts and retain the same code patch.

The measurements below were collected before that rebase, against
`xlambein/rust-analyzer` at `03fcb77246f2568adb0e9b2fa60d19c6cc1686f4`
(2026-09-27). They are historical results, not new measurements of upstream master.

## Plan and implementation

1. Build a deterministic LSP benchmark using the supplied `lsp_bench.Client`.
   Disable Cargo checking, proc macros, sysroot loading, and cache priming.
   Check exact diagnostic counts and fix contents before trusting timings.
2. Reproduce duplicate native computation with a regression test across cloned
   database snapshots. Before the change it reports **two computations, expected
   one**; after the change it passes.
3. Cache rendered semantic diagnostics in a Salsa tracked query owned by
   `ide-diagnostics`. Key it by file, configuration, and fix-resolution strategy.
   Existing callers and LSP behavior use the same entry point.
4. Normalize diagnostic enablement and experimental visibility in the key, then
   filter experimental diagnostics after lookup. Quick-fix requests intentionally
   include experimental diagnostics hidden by publication.
5. Validate configuration changes, edits in a dependency, edits in the target,
   disappearance of errors, lazy rename resolution, and before/after output parity.

The result is `cache.rs`, a private database view, and an LRU target of 128 query
results. Salsa reclaims entries above that target when a revision advances;
this is an entry target rather than a strict byte limit. Existing Salsa input
tracking provides invalidation and concurrent request coordination.

Configuration equality covers all fields. Disabled-lint names are sorted for
hashing so equal sets share an entry regardless of insertion order. Resolution
mode is part of the key because `incorrect_case` deliberately leaves rename
edits unresolved. Most other native diagnostic fixes are currently eager.
The only dependency addition is a direct edge to the already-used Salsa version.

## Release measurements

Rust 1.100.0; same `cargo build --release --locked -p rust-analyzer` flags for both
binaries. Thirty samples per phase and binary. Each process loads a fresh
workspace, validates and warms it, then measures requests. Compilation was stopped
during the final runs. Sparse runs were repeated in reversed binary order.

1,000 functions, **one** missing-field diagnostic, experimental publication off:

| Phase | Before: run A / B (ms) | After: run A / B (ms) |
|---|---:|---:|
| Repeated diagnostics, unchanged revision | 5.05 / 5.64 | 0.25 / 0.27 |
| Repeated quick-fix listing, unchanged revision | 9.79 / 10.36 | 5.16 / 8.92 |
| First diagnostics after each edit | 36.21 / 35.79 | 36.47 / 38.42 |
| Quick-fix listing after those diagnostics | 12.00 / 12.44 | 6.29 / 5.86 |

Each cell is a median. Full samples and binary hashes are in the original investigation export
(`diagnostics-18997.zip`, under `results`). Experimental-on runs also pass. A deliberately dense
300-function/300-error stress case reduces post-edit quick-fix listing from
62.36 ms to 2.49 ms; that workload is intentionally artificial.

Normalized diagnostic and edit fingerprints match between binaries in all
four comparisons: sparse A, sparse B, experimental-on, and dense. These include
initial, dependency-changed, clean, and restored states. The missing-field
`codeAction/resolve` is a no-op control because its edit is already present.

## Initial validation before rebase

- Diagnostics suite: **743 passed, one pre-existing ignored test**.
- ide-db suite: **196 passed**.
- Targeted regression demonstrably fails before the cache and passes afterward.
- Clippy for both affected crates, all targets: completed successfully. One
  warning in unchanged `hir-ty/src/lower.rs:1241` (`needless_bool`).
- Formatting and whitespace checks pass.
- LSP correctness checks pass for all final comparison runs.

## Validation after rebase onto upstream master

- Diagnostics suite: **759 passed, one existing ignored test**.
- ide-db suite: **196 passed** (955 passing tests across the two crates).
- Clippy for both affected crates, all targets: clean, with no warnings.
- Formatting, whitespace checks, and the release server build pass.
- The LSP benchmark smoke run passes with 100 functions, one error, and three
  iterations per phase, including dependency changes and error removal/restoration.
- The earlier timing table remains a pre-rebase measurement; the smoke run is a
  correctness check rather than a new performance comparison.

## Limits and next experiments

This removes repeated work; it does not accelerate the initial diagnostic
computation. The initial post-edit request was slightly slower in these samples,
consistent with retaining and cloning cached diagnostics. This is a synthetic
per-file benchmark, with no claim about startup or references performance.

Different fix-resolution strategies still compute separate entries. A next
iteration could separate diagnostic collection from fix materialization, sharing
the common work across first-time resolution and clients requesting eager fixes.
Before expanding scope, profile a representative real workspace and measure cache
retention and cold-request overhead. Finer-grained per-body queries would be a
larger follow-up for reuse across edits within a file.
