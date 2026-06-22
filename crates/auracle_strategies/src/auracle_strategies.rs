//! Honest row-building for the native Strategies navigator (and the validation
//! rail's strategy picker, which reads the same engine route).
//!
//! The engine lists strategies as `(path, doc, bundled)` entries off
//! `/ui/api/backtest/strategies`. The navigator's only decision logic is purely
//! mechanical: derive a display `name` (the last dotted segment), pull the first
//! non-empty line of the docstring, drop entries with no path, and order
//! user-written strategies before bundled examples then by name. None of that
//! fabricates a label — an entry with no doc shows no doc, an entry with no path
//! is dropped rather than rendered blank.
//!
//! `module_to_relpath` is moved here verbatim from the panel so the "open this
//! strategy file" resolution is testable: when it returns `None` the view shows
//! an honest note instead of a silent no-op.
//!
//! Kept gpui-free so parse/derive/sort/resolve are unit-tested without
//! rendering. The panel wraps the returned rows in `auracle_view_state::Load` at
//! the call site; this crate stays dependency-free.

/// One strategy navigator row, derived from the engine's strategy entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StrategyListItem {
    /// Last dotted segment of `path` — the class or function name.
    pub name: String,
    /// Full dotted module path, verbatim (e.g. `strategies.example_ma.MACrossover`).
    pub path: String,
    /// First non-empty line of the docstring, or "".
    pub doc: String,
    pub bundled: bool,
}

/// Derive the display name (last `.`-segment) from a dotted path. The whole path
/// if it has no dot; "" for an empty path. Mechanical only — never fabricates.
pub fn display_name(path: &str) -> String {
    if path.is_empty() {
        return String::new();
    }
    // `rsplit` always yields at least one element, so `path` with no dot returns
    // the whole path — matching the live `path.rsplit('.').next().unwrap_or(path)`.
    path.rsplit('.').next().unwrap_or(path).to_string()
}

/// First non-empty line of a docstring, trimmed; "" if none. Leading blank lines
/// are skipped so a docstring that opens with a newline still shows real text
/// rather than an empty first line. Never invents text.
pub fn first_doc_line(doc: &str) -> String {
    doc.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("")
        .to_string()
}

/// Build + sort the navigator rows from raw `(path, doc, bundled)` entries.
///
/// - Entries with an empty path are dropped (never a blank row).
/// - `name` and `doc` are derived mechanically.
/// - Order: user-written (`bundled == false`) first, then bundled examples,
///   then by name (case-insensitive). The sort is stable, so entries with an
///   equal key keep the engine's order.
///
/// Pure → unit-tested. Mirrors the live fetch's
/// `sort_by(|a, b| a.bundled.cmp(&b.bundled).then(a.name.cmp(&b.name)))`, with
/// the name comparison made case-insensitive per the slice spec.
pub fn strategy_rows<'a>(
    entries: impl IntoIterator<Item = (&'a str, &'a str, bool)>,
) -> Vec<StrategyListItem> {
    let mut rows: Vec<StrategyListItem> = entries
        .into_iter()
        .filter(|(path, _doc, _bundled)| !path.is_empty())
        .map(|(path, doc, bundled)| StrategyListItem {
            name: display_name(path),
            path: path.to_string(),
            doc: first_doc_line(doc),
            bundled,
        })
        .collect();
    // Stable sort keeps engine order for equal keys (honesty: no reordering we
    // can't justify). bundled=false sorts before bundled=true.
    rows.sort_by(|a, b| {
        a.bundled
            .cmp(&b.bundled)
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    rows
}

/// `strategies.example_ma.MACrossover` → `Some("strategies/example_ma.py")`.
///
/// `None` when the path has fewer than 2 non-empty segments (we can't form a
/// module path without dropping a class/function tail). Moved verbatim from
/// `strategies_panel::module_to_relpath` so resolution becomes testable and the
/// view can show an honest note when it fails instead of a dead click.
pub fn module_to_relpath(module_path: &str) -> Option<String> {
    let mut parts: Vec<&str> = module_path.split('.').filter(|p| !p.is_empty()).collect();
    if parts.len() < 2 {
        return None;
    }
    parts.pop(); // drop the class / function name, leaving the module
    Some(format!("{}.py", parts.join("/")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_name_takes_last_segment() {
        assert_eq!(display_name("a.b.C"), "C");
        assert_eq!(display_name("solo"), "solo");
        assert_eq!(display_name(""), "");
    }

    #[test]
    fn first_doc_line_takes_first_non_empty_trimmed() {
        assert_eq!(first_doc_line("first\nsecond"), "first");
        // Leading blank lines are skipped.
        assert_eq!(first_doc_line("\n\n  real text  \nmore"), "real text");
        assert_eq!(first_doc_line(""), "");
        assert_eq!(first_doc_line("   \n\t"), "");
    }

    #[test]
    fn user_rows_sort_before_bundled_even_when_bundled_sorts_earlier() {
        // "alpha" (bundled) would sort before "zeta" (user) alphabetically, but
        // user-written must come first.
        let rows = strategy_rows([
            ("strategies.alpha.Alpha", "", true),
            ("strategies.zeta.Zeta", "", false),
        ]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name, "Zeta");
        assert!(!rows[0].bundled);
        assert_eq!(rows[1].name, "Alpha");
        assert!(rows[1].bundled);
    }

    #[test]
    fn empty_path_entries_are_dropped() {
        let rows = strategy_rows([
            ("", "ignored doc", false),
            ("strategies.real.Real", "doc", false),
        ]);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "Real");
    }

    #[test]
    fn name_order_is_case_insensitive() {
        // "alpha" should come before "Beta" despite uppercase 'B' < lowercase 'a'.
        let rows = strategy_rows([
            ("strategies.b.Beta", "", false),
            ("strategies.a.alpha", "", false),
        ]);
        assert_eq!(rows[0].name, "alpha");
        assert_eq!(rows[1].name, "Beta");
    }

    #[test]
    fn rows_derive_first_doc_line() {
        let rows = strategy_rows([("strategies.x.X", "\nSummary line\ndetail", false)]);
        assert_eq!(rows[0].doc, "Summary line");
    }

    #[test]
    fn module_to_relpath_requires_two_segments() {
        assert_eq!(
            module_to_relpath("strategies.example_ma.MACrossover"),
            Some("strategies/example_ma.py".to_string())
        );
        assert_eq!(
            module_to_relpath("pkg.sub.mod.Class"),
            Some("pkg/sub/mod.py".to_string())
        );
        // Fewer than 2 non-empty segments → None.
        assert_eq!(module_to_relpath("solo"), None);
        assert_eq!(module_to_relpath(""), None);
        assert_eq!(module_to_relpath("."), None);
    }

    #[test]
    fn module_to_relpath_filters_empty_segments() {
        // Trailing / doubled dots produce empty segments that are filtered out
        // before the length check and the join.
        assert_eq!(
            module_to_relpath("strategies..example.Class"),
            Some("strategies/example.py".to_string())
        );
        assert_eq!(module_to_relpath("a.b."), Some("a.py".to_string()));
    }
}
