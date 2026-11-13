//! Run explicitly in release mode; timings are observations, not test assertions.

use std::{hint::black_box, time::Instant};

use ide_db::base_db::SourceDatabase;
use serde_json::json;
use test_fixture::WithFixture;

use super::*;

#[derive(Clone, Copy, Debug)]
enum Mode {
    Uncached,
    PreserveFlag,
    NormalizeFlag,
}

fn request(
    db: &RootDatabase,
    config: &DiagnosticsConfig,
    file: FileId,
    mode: Mode,
) -> Vec<Diagnostic> {
    hir::attach_db(db, || match mode {
        Mode::Uncached => {
            crate::semantic_diagnostics_uncached(db, config, &AssistResolveStrategy::None, file)
        }
        Mode::PreserveFlag => semantic_diagnostics(db, config, &AssistResolveStrategy::None, file),
        Mode::NormalizeFlag => {
            let mut config = config.clone();
            config.enabled = true;
            config.disable_experimental = false;
            let key = DiagnosticsRequest::new(
                db,
                file,
                &CacheConfig(config),
                &AssistResolveStrategy::None,
            );
            // Reproduce the previous production wrapper's normalization and late filtering.
            semantic_diagnostics_query(db, key)
                .iter()
                .filter(|d| !d.experimental)
                .cloned()
                .collect()
        }
    })
}

fn timed(f: impl FnOnce() -> Vec<Diagnostic>) -> (f64, Vec<Diagnostic>) {
    let start = Instant::now();
    let result = black_box(f());
    (start.elapsed().as_secs_f64() * 1000.0, result)
}

fn fingerprint(diagnostics: &[Diagnostic]) -> String {
    // Includes diagnostic metadata and fix contents; performed outside the timed region.
    format!("{diagnostics:?}")
}

#[test]
#[ignore = "manual diagnostics normalization benchmark; run with --release --nocapture"]
fn experimental_normalization_benchmark() {
    let functions: usize = std::env::var("DIAGNOSTICS_BENCH_FUNCTIONS")
        .unwrap_or_else(|_| "1000".into())
        .parse()
        .unwrap();
    let iterations: usize = std::env::var("DIAGNOSTICS_BENCH_ITERATIONS")
        .unwrap_or_else(|_| "30".into())
        .parse()
        .unwrap();
    let rounds: usize =
        std::env::var("DIAGNOSTICS_BENCH_ROUNDS").unwrap_or_else(|_| "3".into()).parse().unwrap();
    let modes = [Mode::Uncached, Mode::PreserveFlag, Mode::NormalizeFlag];
    let mut records = Vec::new();
    for workload in ["clean", "shorthand", "allowed_shorthand"] {
        let mut source = String::from("struct S { field: i32 }\nfn error() { let _ = S {}; }\n");
        for i in 0..functions {
            if workload == "allowed_shorthand" {
                source.push_str("#[allow(clippy::redundant_field_names)]\n");
            }
            let field = if workload == "clean" { "field" } else { "field: field" };
            source.push_str(&format!("fn f{i}(field: i32) {{ let _ = S {{ {field} }}; }}\n"));
        }
        let mut expected = None;
        for round in 0..rounds {
            // Rotate order to reduce systematic first/last-run bias.
            for offset in 0..modes.len() {
                let mode = modes[(round + offset) % modes.len()];
                let (mut db, file) = RootDatabase::with_single_file(&source);
                let file = file.file_id(&db);
                let mut config = DiagnosticsConfig::test_sample();
                config.disable_experimental = true;
                let (cold_ms, diagnostics) = timed(|| request(&db, &config, file, mode));
                assert_eq!(diagnostics.len(), 1);
                assert_eq!(diagnostics[0].code.as_str(), "E0063");
                let actual = fingerprint(&diagnostics);
                if let Some(expected) = &expected {
                    assert_eq!(&actual, expected);
                } else {
                    expected = Some(actual.clone());
                }
                let mut edited_ms = Vec::new();
                let mut repeated_ms = Vec::new();
                for edit in 0..iterations {
                    db.set_file_text(file, &format!("{source}// revision {edit}\n"));
                    let (elapsed, diagnostics) = timed(|| request(&db, &config, file, mode));
                    edited_ms.push(elapsed);
                    assert_eq!(fingerprint(&diagnostics), actual);
                    for _ in 0..5 {
                        let (elapsed, diagnostics) = timed(|| request(&db, &config, file, mode));
                        repeated_ms.push(elapsed);
                        assert_eq!(fingerprint(&diagnostics), actual);
                    }
                }
                // Validate the stress fixture actually produces experimental diagnostics.
                config.disable_experimental = false;
                let all = hir::attach_db(&db, || {
                    crate::semantic_diagnostics_uncached(
                        &db,
                        &config,
                        &AssistResolveStrategy::None,
                        file,
                    )
                });
                let experimental = all.iter().filter(|d| d.experimental).count();
                assert_eq!(experimental, if workload == "shorthand" { functions } else { 0 });
                records.push(json!({
                    "workload": workload, "mode": format!("{mode:?}"), "round": round,
                    "cold_ms": cold_ms, "edited_ms": edited_ms, "repeated_ms": repeated_ms,
                    "experimental_when_enabled": experimental, "visible": diagnostics.len(),
                }));
            }
        }
    }
    println!(
        "NORMALIZATION_BENCH={}",
        json!({
            "functions": functions, "iterations": iterations, "rounds": rounds,
            "outputs_match": true, "records": records,
        })
    );
}
