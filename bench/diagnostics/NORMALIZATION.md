# Experimental diagnostics normalization experiment

Follow-up: [preserve the experimental flag](PRESERVE_EXPERIMENTAL.md), with new
diagnostics-only and LSP measurements. Production now preserves the flag; the
results below describe the earlier normalization implementation.

Measured 2026-11-13 on the `investigate-18997` implementation at `18af8eb`.
Compiler: rustc 1.100.0 (0ef1bc386 2026-11-09), x86_64 Linux, workspace release defaults.

## Finding

Normalizing `disable_experimental` to false causes a measurable diagnostics-only
regression. With 1,000 shorthand diagnostics, the median first request after an
edit is roughly three times as slow as keeping the flag in the cache key.
Suppressing those diagnostics using `#[allow]` still incurs additional processing.
Clean code shows no consistent normalization cost at this scale.

Recommendation: preserve `disable_experimental` in the key for now. That gives up
cross-flag reuse but retains reuse between requests with identical configurations.
This experiment does **not** change production behavior; the alternative lives
only in the benchmark. It does not measure the tradeoff for diagnostics followed
by quick fixes.

## Method

An explicitly ignored Rust test compares three paths in one optimized binary:

- `Uncached`: the original computation with experimental diagnostics disabled.
- `PreserveFlag`: the same Salsa cache, retaining `disable_experimental = true`.
- `NormalizeFlag`: the current production cache, enabling experimental diagnostics
  internally and filtering them at the end.

Each workload has one stable missing-field error, plus 100 or 1,000 functions:
`clean` uses shorthand initialization already; `shorthand` uses `field: field`;
`allowed_shorthand` uses the latter with a per-function
`#[allow(clippy::redundant_field_names)]`.

For each size and workload, run three rounds with rotated implementation order
and a fresh database per implementation. Record the initial cold request, then
30 comment edits. After each edit, time one request followed by five unchanged
requests. Setup, edits, output validation, and fixture validation are outside
the timed regions. Returned vectors are dropped outside timing in every mode.
No compilation runs concurrently with the timed samples.

All paths return exactly the same diagnostic and fix Debug representations,
including across edits. Enabled diagnostics are separately checked to contain
exactly N experimental diagnostics for `shorthand`, and zero for the other cases.
All nine implementations/workload combinations pass for both sizes.

## First request after each edit

Pooled medians in milliseconds (90 samples per cell). The last column isolates
normalization overhead relative to an otherwise equivalent cache.

| Functions | Workload | Uncached | Preserve flag | Normalize flag | Normalization overhead |
|---:|---|---:|---:|---:|---:|
| 100 | clean | 2.512 | 2.415 | 2.429 | +0.6% |
| 100 | shorthand | 2.671 | 2.622 | 3.738 | +42.6% |
| 100 | allowed_shorthand | 2.866 | 2.950 | 3.603 | +22.1% |
| 1000 | clean | 32.879 | 33.446 | 33.307 | -0.4% |
| 1000 | shorthand | 32.950 | 34.400 | 104.213 | +202.9% |
| 1000 | allowed_shorthand | 39.283 | 41.844 | 82.297 | +96.7% |

For the 1,000-function shorthand workload, each round independently shows the
regression: preserve-flag medians 35.984, 32.613, 35.003 ms versus normalize-flag
medians 108.320, 104.847, 102.572 ms. Suppressed shorthand likewise regresses in
all rounds. Cold samples are retained in the raw data but are too few for a
reliable cold-start conclusion.

## Unchanged requests

Pooled medians in microseconds (450 samples per cell), at 1,000 functions.

| Workload | Uncached | Preserve flag | Normalize flag |
|---|---:|---:|---:|
| clean | 4447.490 | 0.261 | 0.261 |
| shorthand | 4921.304 | 0.270 | 0.886 |
| allowed_shorthand | 5634.455 | 0.270 | 0.280 |

Caching remains highly effective for repeated requests. Normalization retains
hidden diagnostics in the shorthand case and scans them again on each hit;
that produces a small absolute increase in hit cost. Retained memory was not
measured.

## Interpretation and limits

Before this patch, the experimental flag filtered diagnostics after construction,
but before lint attribute/severity processing and macro-origin handling.
Normalization moves that filtering after those stages. This experiment isolates
the resulting cost, including cache insertion and result filtering. It does not
attribute time to individual functions using a profiler.

These are synthetic per-file stress cases, not a representative workspace or an
end-to-end LSP benchmark. Do not extrapolate the percentages to total server CPU,
startup, or typical editing. Three rotated rounds reduce order bias but are not
confidence intervals. The substantial regression is consistent across rounds;
small differences in the clean control should be treated as noise.

## Reproduce

From the repository root:

```sh
cargo test --release -p ide-diagnostics experimental_normalization_benchmark \
  --lib -- --ignored --nocapture > /tmp/normalization.log 2>&1
DIAGNOSTICS_BENCH_FUNCTIONS=100 cargo test --release -p ide-diagnostics \
  experimental_normalization_benchmark --lib -- --ignored --nocapture \
  > /tmp/normalization-100.log 2>&1
```

The test emits one `NORMALIZATION_BENCH=` JSON line with every raw sample.
`DIAGNOSTICS_BENCH_FUNCTIONS`, `DIAGNOSTICS_BENCH_ITERATIONS`, and
`DIAGNOSTICS_BENCH_ROUNDS` default to 1000, 30, and 3. Parse that line and pool
`edited_ms` or `repeated_ms` by workload and mode to reproduce the tables.
The checked-in JSON files contain both raw runs and compiler/source metadata.
