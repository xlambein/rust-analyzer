//! Share native diagnostics between publication and quick-fix requests.

use std::{
    hash::{Hash, Hasher},
    mem::discriminant,
};

use ide_db::{
    FileId, RootDatabase, assists::AssistResolveStrategy, imports::insert_use::InsertUseConfig,
};

use crate::{Diagnostic, DiagnosticsConfig};

// Private Salsa view for diagnostic handlers, which currently use RootDatabase.
#[salsa::db]
trait DiagnosticsDatabase: salsa::Database {
    fn root_database(&self) -> &RootDatabase;
}

#[salsa::db]
impl DiagnosticsDatabase for RootDatabase {
    fn root_database(&self) -> &RootDatabase {
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CacheConfig(DiagnosticsConfig);

impl Hash for CacheConfig {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Hash sets have no stable iteration order. Destructure exhaustively so adding a
        // configuration field requires considering it here as well as in equality.
        let DiagnosticsConfig {
            enabled,
            proc_macros_enabled,
            proc_attr_macros_enabled,
            disable_experimental,
            disabled,
            expr_fill_default,
            style_lints,
            snippet_cap,
            insert_use,
            prefer_no_std,
            prefer_prelude,
            prefer_absolute,
            term_search_fuel,
            show_rename_conflicts,
        } = &self.0;
        (
            enabled,
            proc_macros_enabled,
            proc_attr_macros_enabled,
            disable_experimental,
            style_lints,
            prefer_no_std,
            prefer_prelude,
            prefer_absolute,
            term_search_fuel,
            show_rename_conflicts,
        )
            .hash(state);
        discriminant(expr_fill_default).hash(state);
        snippet_cap.is_some().hash(state);
        let InsertUseConfig {
            granularity,
            enforce_granularity,
            prefix_kind,
            group,
            skip_glob_imports,
        } = insert_use;
        (
            discriminant(granularity),
            enforce_granularity,
            discriminant(prefix_kind),
            group,
            skip_glob_imports,
        )
            .hash(state);
        let mut disabled: Vec<_> = disabled.iter().collect();
        disabled.sort_unstable();
        disabled.hash(state);
    }
}

#[salsa::interned(debug)]
struct DiagnosticsRequest {
    #[returns(copy)]
    file_id: FileId,
    #[returns(ref)]
    config: CacheConfig,
    #[returns(ref)]
    resolve: AssistResolveStrategy,
}

// Salsa evicts results above this entry target when the next revision starts.
// Messages and source edits do not implement structural equality, so changed dependencies
// conservatively invalidate users of this query.
#[salsa::tracked(returns(ref), no_eq, lru = 128)]
fn semantic_diagnostics_query<'db>(
    db: &'db dyn DiagnosticsDatabase,
    request: DiagnosticsRequest<'db>,
) -> Vec<Diagnostic> {
    crate::semantic_diagnostics_uncached(
        db.root_database(),
        &request.config(db).0,
        request.resolve(db),
        request.file_id(db),
    )
}

pub(super) fn semantic_diagnostics(
    db: &RootDatabase,
    config: &DiagnosticsConfig,
    resolve: &AssistResolveStrategy,
    file_id: FileId,
) -> Vec<Diagnostic> {
    let mut cached_config = config.clone();
    // Enablement is handled by callers. Cache experimental diagnostics too: publication hides
    // them, whereas quick fixes include them even when publication is disabled.
    cached_config.enabled = true;
    cached_config.disable_experimental = false;
    let config_key = CacheConfig(cached_config);
    let request = DiagnosticsRequest::new(db, file_id, &config_key, resolve);
    semantic_diagnostics_query(db, request)
        .iter()
        .filter(|diagnostic| !config.disable_experimental || !diagnostic.experimental)
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_ignores_disabled_lint_order() {
        let db = RootDatabase::default();
        let mut first = DiagnosticsConfig::test_sample();
        first.disabled.extend(["one".to_owned(), "two".to_owned()]);
        let mut second = DiagnosticsConfig::test_sample();
        second.disabled.extend(["two".to_owned(), "one".to_owned()]);
        let a = DiagnosticsRequest::new(
            &db,
            FileId::from_raw(0),
            CacheConfig(first),
            AssistResolveStrategy::None,
        );
        let b = DiagnosticsRequest::new(
            &db,
            FileId::from_raw(0),
            CacheConfig(second),
            AssistResolveStrategy::None,
        );
        assert_eq!(a, b);
    }
}
