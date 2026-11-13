# Preserve the experimental diagnostics flag

Follow-up measured 2026-11-13, based on `db80453` with the preservation change.
Compiler: rustc 1.100.0 (0ef1bc386 2026-11-09), x86_64 Linux, workspace release defaults.

## Change and validation

Keep `disable_experimental` in `DiagnosticsRequest` rather than normalizing it
to false. The uncached computation already filters experimental diagnostics
before expensive lint/macro post-processing, so the wrapper can now simply clone
the stored vector without filtering again. The caller-only `enabled` flag
continues to be normalized.

Requests with the same experimental setting still reuse their cache entries.
Default publication and quick-fix requests use different settings and therefore
compute separate results. This is a deliberate tradeoff to avoid eagerly doing
hidden-diagnostic work during publication.

759 diagnostics tests pass; two tests are ignored (the existing ignored test and
this manual benchmark). The updated experimental-mode regression test fails
against normalization and passes with preservation. It checks separate entries
for hidden/enabled diagnostics, reuse after returning to the hidden mode, and
explicitly disabled lint handling. Formatting and whitespace checks pass.

## Diagnostics-only benchmark

Re-ran the optimized three-mode experiment from [NORMALIZATION.md](NORMALIZATION.md).
`PreserveFlag` now calls the **actual production wrapper**. `NormalizeFlag`
explicitly reproduces the old normalization/late-filtering wrapper; `Uncached`
calls the original diagnostics computation. This keeps the historical comparator
available after changing production.

Each workload has 1,000 functions plus one visible missing-field error.
Three rounds rotate mode order with a fresh database per mode. Each round uses
30 comment edits followed by five unchanged requests per edit. All diagnostics
and fix Debug fingerprints match across implementations and revisions. The fixture
also verifies experimental diagnostics are actually generated when enabled.

First request after an edit, pooled medians in milliseconds (90 samples per cell):

| Workload | Uncached | Old normalization | Production preservation | Preservation vs uncached |
|---|---:|---:|---:|---:|
| clean | 31.161 | 33.604 | 32.348 | +3.8% |
| shorthand | 33.952 | 111.907 | 36.217 | +6.7% |
| allowed_shorthand | 40.719 | 84.112 | 42.204 | +3.6% |

Preserving the flag removes the large normalization penalty. It does not remove
all cache overhead on a miss: allocating/storing/cloning a query result still
costs time. In the shorthand stress case the preserved cache is about 7% slower
than uncached computation on the first request after each edit.

Unchanged requests, pooled medians in microseconds (450 samples per cell):

| Workload | Uncached | Old normalization | Production preservation |
|---|---:|---:|---:|
| clean | 3579.752 | 0.271 | 0.261 |
| shorthand | 5321.218 | 0.931 | 0.271 |
| allowed_shorthand | 5884.341 | 0.271 | 0.261 |

## End-to-end LSP benchmark

Built both servers from the same code/compiler/flags, changing only the production
wrapper. Ran the existing LSP driver with 1,000 functions, one missing-field error,
experimental publication disabled, and 30 iterations per phase. Run order was
normalized A, preserved A, preserved B, normalized B. No builds ran during timing.

Medians in milliseconds, pooling 60 samples per cell:

| Request | Old normalization | Production preservation |
|---|---:|---:|
| unchanged_diagnostics | 0.304 | 0.322 |
| unchanged_code_action | 5.646 | 5.460 |
| edited_diagnostics | 36.949 | 36.491 |
| edited_code_action | 5.905 | 12.600 |
| edited_resolve | 0.456 | 0.521 |

The median combined post-edit diagnostics + code action + resolve sequence is
44.089 ms with normalization versus 49.291 ms with preservation.
This driver deliberately asks for a quick fix after every diagnostic request,
so it exposes the loss of cross-flag sharing. The post-edit code action cost
roughly doubles; repeated diagnostics remain around 0.3 ms. This is not a claim
that preservation improves this mixed workload.

All four LSP runs pass dependency-change, error removal/restoration, diagnostic,
and fix assertions. Their diagnostic/fix response fingerprints match exactly.
The raw JSON records server binary SHA-256 hashes, samples, platform, and settings.

## Reproduction

Diagnostics-only comparison (three implementations in one binary):

```sh
cargo test --release -p ide-diagnostics experimental_normalization_benchmark \
  --lib -- --ignored --nocapture > /tmp/preserve-normalization.log 2>&1
```

The `NORMALIZATION_BENCH=` JSON line contains every raw sample.
For the LSP comparison, retain a server built from `db80453` as
`/tmp/ra-normalized-18997`, then build the changed branch and copy its server to
`/tmp/ra-preserved-18997`. Run each binary twice in the order above:

```sh
PYTHONPATH=/path/to/lsp-bench-main/src python bench/diagnostics/benchmark.py \
  --rust-analyzer /tmp/ra-preserved-18997 --functions 1000 --errors 1 \
  --iterations 30 --output /tmp/lsp-preserved.json
```

Raw results: [diagnostics-only](preserve-normalization-results.json) and
[LSP](preserve-lsp-results.json).

## Limits

Synthetic per-file workloads; no startup or full-workspace claim. The direct
benchmark tests the normalization penalty in deliberately lint-heavy code.
The LSP fixture uses a no-core project with different generated source/configuration;
its absolute diagnostics times should not be compared with the direct fixture.
The runs isolate publication-only benefits and mixed-request costs, rather than
asserting one universal speedup. Memory retention was not measured. Rotated order
reduces systematic order bias, but small changes remain susceptible to timing noise.
