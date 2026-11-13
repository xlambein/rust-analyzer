use ide_db::{RootDatabase, assists::AssistResolveStrategy};
use test_fixture::WithFixture;

use crate::{DiagnosticsConfig, semantic_diagnostics};

#[test]
fn reuses_diagnostics_between_requests() {
    let (db, file) = RootDatabase::with_single_file(
        r#"
struct S { field: i32 }
fn f() { let _ = S {}; }
"#,
    );
    let file = file.file_id(&db);
    let mut config = DiagnosticsConfig::test_sample();
    config.disable_experimental = true;

    cov_mark::check_count!(semantic_diagnostics_computed, 1);
    let diagnostics = hir::attach_db(&db, || {
        semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
    });
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "E0063");

    // LSP requests use different snapshots, and quick fixes include experimental diagnostics.
    let snapshot = db.clone();
    config.disable_experimental = false;
    let fixes = hir::attach_db(&snapshot, || {
        semantic_diagnostics(&snapshot, &config, &AssistResolveStrategy::None, file)
    });
    assert_eq!(fixes.len(), 1);
    assert_eq!(fixes[0].fixes.as_ref().unwrap()[0].id.0, "fill_missing_fields");
}

#[test]
fn diagnostics_cache_invalidates_after_dependency_and_target_edits() {
    use ide_db::base_db::SourceDatabase;

    let (mut db, files) = RootDatabase::with_many_files(
        r#"
//- /lib.rs
mod model;
fn f() { let _ = model::S {}; }
//- /model.rs
pub struct S { pub old_field: i32 }
"#,
    );
    let file = files[0].file_id(&db);
    let model = files[1].file_id(&db);
    let config = DiagnosticsConfig::test_sample();
    let diagnostics = hir::attach_db(&db, || {
        semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
    });
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("old_field"));

    db.set_file_text(model, "pub struct S { pub new_field: i32 }");
    let diagnostics = hir::attach_db(&db, || {
        semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
    });
    assert_eq!(diagnostics.len(), 1);
    assert!(diagnostics[0].message.contains("new_field"));
    assert!(!diagnostics[0].message.contains("old_field"));
    let fix = &diagnostics[0].fixes.as_ref().unwrap()[0];
    let mut fixed = db.file_text(file).text(&db).to_string();
    fix.source_change.as_ref().unwrap().source_file_edits[&file].0.apply(&mut fixed);
    assert!(fixed.contains("new_field:"));
    assert!(!fixed.contains("old_field"));

    db.set_file_text(file, "mod model; fn f() { let _ = model::S { new_field: 0 }; }");
    assert!(
        hir::attach_db(&db, || {
            semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
        })
        .is_empty()
    );
}

#[test]
fn diagnostics_cache_preserves_experimental_quick_fixes() {
    let (db, file) = RootDatabase::with_single_file(
        r#"
struct S { field: i32 }
fn f(field: i32) { let _ = S { field: field }; }
"#,
    );
    let file = file.file_id(&db);
    let mut config = DiagnosticsConfig::test_sample();
    config.disable_experimental = true;
    assert!(
        hir::attach_db(&db, || {
            semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
        })
        .is_empty()
    );

    config.disable_experimental = false;
    let diagnostics = hir::attach_db(&db, || {
        semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
    });
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].code.as_str(), "redundant_field_names");
    assert!(diagnostics[0].fixes.as_ref().unwrap()[0].source_change.is_some());

    config.disabled.insert("redundant_field_names".to_owned());
    assert!(
        hir::attach_db(&db, || {
            semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
        })
        .is_empty()
    );
}

#[test]
fn diagnostics_cache_keeps_lazy_fix_resolution_separate() {
    use ide_db::assists::{AssistKind, SingleResolve};

    let (db, file) = RootDatabase::with_single_file(
        r#"
struct bad_name;
fn f(_: bad_name) {}
"#,
    );
    let file = file.file_id(&db);
    let config = DiagnosticsConfig::test_sample();
    for (strategy, resolved) in [
        (AssistResolveStrategy::None, false),
        (AssistResolveStrategy::All, true),
        (
            AssistResolveStrategy::Single(SingleResolve {
                assist_id: "change_case".to_owned(),
                assist_kind: AssistKind::QuickFix,
                assist_subtype: None,
            }),
            true,
        ),
        (
            AssistResolveStrategy::Single(SingleResolve {
                assist_id: "different_fix".to_owned(),
                assist_kind: AssistKind::QuickFix,
                assist_subtype: None,
            }),
            false,
        ),
        (AssistResolveStrategy::None, false),
    ] {
        let diagnostics =
            hir::attach_db(&db, || semantic_diagnostics(&db, &config, &strategy, file));
        assert_eq!(diagnostics.len(), 1);
        let fix = &diagnostics[0].fixes.as_ref().unwrap()[0];
        assert_eq!(fix.id.0, "change_case");
        assert_eq!(fix.source_change.is_some(), resolved);
    }
    let mut config = config;
    config.disabled.insert("non_camel_case_types".to_owned());
    assert!(
        hir::attach_db(&db, || {
            semantic_diagnostics(&db, &config, &AssistResolveStrategy::None, file)
        })
        .is_empty()
    );
}
