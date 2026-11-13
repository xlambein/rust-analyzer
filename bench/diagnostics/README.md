# Native diagnostics reuse (#18997)

This benchmark uses `lsp_bench.Client` from the supplied lsp-bench library.
It creates a deterministic `rust-project.json` workspace without Cargo, external
crates, build scripts, proc macros, or a sysroot. It uses primitive types and
record constructors, so it needs no downloaded Rust source.

## Run

Build the server with the same compiler and flags for both revisions:

```sh
cargo build --release --locked -p rust-analyzer
cp target/release/rust-analyzer /tmp/rust-analyzer-before
# Apply the implementation, then build and copy /tmp/rust-analyzer-after.
```

Point `PYTHONPATH` at the attached library's `src` directory; installation and
NumPy are unnecessary. From the rust-analyzer repository:

```sh
PYTHONPATH=/path/to/lsp-bench-main/src python bench/diagnostics/benchmark.py \
    --rust-analyzer /tmp/rust-analyzer-before \
    --functions 1000 --errors 1 --iterations 30 --output before.json
PYTHONPATH=/path/to/lsp-bench-main/src python bench/diagnostics/benchmark.py \
    --rust-analyzer /tmp/rust-analyzer-after \
    --functions 1000 --errors 1 --iterations 30 --output after.json
```

Run sequentially on an otherwise idle machine. Repeat in reversed order to check
for build/order effects. Add `--experimental` to exercise publication with
experimental diagnostics enabled; omission exercises the default disabled case.
`--errors 1000` gives a deliberately diagnostic-heavy stress case.

## Measurements and assertions

Each iteration requests `textDocument/diagnostic`, then
`textDocument/codeAction` restricted to quick fixes, then `codeAction/resolve`.
The driver waits for project loading, validates and warms the fixture, and only
then starts timing. Cargo checking and cache priming are disabled.

- `unchanged_*`: repeated requests in the same database revision.
- `edited_*`: append a changing comment, then repeat the same requests. This
  measures diagnostics recomputation followed by quick-fix reuse after each edit.
- `*_resolve` is a protocol control: missing-field fixes already contain an edit,
  so resolution returns it unchanged. Lazy rename resolution is covered by Rust
  regression tests, rather than claimed as a speedup from this fixture.

Every response must have the expected **exact** diagnostic count, code, source,
and missing-field message. The driver also checks the target fix and its edit,
adds a required field in a different file without changing the target text,
fixes all errors and checks diagnostics/fixes disappear, then restores the input
and checks they reappear. Failure aborts without reporting successful results.
These checks run outside the samples except for response validation, which is
excluded from each request's timer.

The JSON contains raw samples in milliseconds, medians, extrema, binary SHA-256,
server version, platform, fixture size, assertion status, and normalized response
fingerprints for comparing diagnostic and fix contents across binaries. Compare like-for-like
binary flags and fixture parameters. This is a synthetic per-file workload; it
does not measure startup, references, or total workspace analysis time.

The local adapter subclasses the supplied `Client`: it drains stderr, reads each
LSP frame with `readexactly`, answers progress/registration requests, and propagates
transport failures. The supplied library itself is unchanged.
