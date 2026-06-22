//! Honest decision logic for the native Validation rail.
//!
//! The engine returns a per-strategy verdict: a plain-language summary plus a
//! list of overfit-check signals, each carrying a "tier" word the engine chose
//! (`green`/`red`/`amber`/`warning`) and human text. The rail's only real
//! decision is mapping that tier word to a glance severity so the view can pick
//! a color and a one-word marker — and doing so *honestly*: a tier the engine
//! didn't send (blank, mixed-case, or anything outside the known set) becomes
//! `NotChecked`, never a guessed `Ok`. The engine's plain/fix/summary text is
//! passed through verbatim; the reducer never rewrites or invents it.
//!
//! Kept gpui-free so the severity contract and the empty/blank-summary rules are
//! unit-tested without rendering. The panel wraps the returned `Verdict` in
//! `auracle_view_state::Load` at the call site; this crate stays dependency-free.

/// Glance severity of one overfit check, decided from the engine's tier string.
///
/// The view maps this (and only this) to a `Color`; `NotChecked` is the honest
/// fallback for any tier the engine didn't actually send.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Ok,
    Caution,
    NeedsAttention,
    NotChecked,
}

/// One render-ready signal row: engine text passed through verbatim plus a
/// derived severity. Never invents a tier or rewrites the engine's plain/fix
/// text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignalRow {
    pub name: String,
    pub severity: Severity,
    /// Engine "what this means"; may be empty.
    pub plain: String,
    /// Engine "what usually fixes it"; may be empty (view hides the line then).
    pub fix: String,
}

/// The verdict the rail shows for one strategy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    /// `None` when the engine summary was blank — so the view omits the summary
    /// row rather than rendering an empty label.
    pub summary: Option<String>,
    pub signals: Vec<SignalRow>,
}

/// Map an engine tier string to a severity. Case-insensitive over the known
/// words only (`green`/`red`/`amber`/`warning`); ANY unrecognised or blank tier
/// is `NotChecked` — never silently `Ok`.
///
/// This preserves the live rail's mapping exactly: `green -> Ok`,
/// `red -> NeedsAttention`, `amber`/`warning -> Caution`, anything else -> the
/// honest "not checked" fallback. We lowercase first so the engine sending
/// `GREEN` doesn't fall through to a false-negative `NotChecked`.
pub fn signal_severity(tier: &str) -> Severity {
    match tier.trim().to_lowercase().as_str() {
        "green" => Severity::Ok,
        "red" => Severity::NeedsAttention,
        "amber" | "warning" => Severity::Caution,
        // Unknown / blank / unexpected — couldn't be checked, so say so rather
        // than guess a positive verdict.
        _ => Severity::NotChecked,
    }
}

/// Build a `SignalRow` from raw engine fields. The severity is derived from the
/// tier; `name`/`plain`/`fix` are passed through verbatim (not trimmed) so the
/// engine's exact wording reaches the view.
pub fn signal_row(name: &str, tier: &str, plain: &str, fix: &str) -> SignalRow {
    SignalRow {
        name: name.to_string(),
        severity: signal_severity(tier),
        plain: plain.to_string(),
        fix: fix.to_string(),
    }
}

/// Normalise a raw verdict: a blank or whitespace-only summary becomes `None`
/// (so the view omits the row rather than rendering an empty label). The summary
/// is otherwise passed through verbatim — never fabricated.
pub fn verdict(summary: &str, signals: Vec<SignalRow>) -> Verdict {
    let summary = if summary.trim().is_empty() {
        None
    } else {
        Some(summary.to_string())
    };
    Verdict { summary, signals }
}

/// A verdict is "empty" (→ `ViewState::Empty`) when it has no signals,
/// regardless of summary text. Used as the `into_view` predicate at the call
/// site: a verdict with no checks is genuinely "nothing to show".
pub fn verdict_is_empty(verdict: &Verdict) -> bool {
    verdict.signals.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_tiers_map_to_their_severities() {
        assert_eq!(signal_severity("green"), Severity::Ok);
        assert_eq!(signal_severity("red"), Severity::NeedsAttention);
        assert_eq!(signal_severity("amber"), Severity::Caution);
        assert_eq!(signal_severity("warning"), Severity::Caution);
    }

    #[test]
    fn tier_match_is_case_insensitive() {
        assert_eq!(signal_severity("GREEN"), Severity::Ok);
        assert_eq!(signal_severity("Red"), Severity::NeedsAttention);
        assert_eq!(signal_severity("  Amber  "), Severity::Caution);
    }

    #[test]
    fn unknown_or_blank_tier_is_not_checked_never_ok() {
        assert_eq!(signal_severity(""), Severity::NotChecked);
        assert_eq!(signal_severity("unknown"), Severity::NotChecked);
        assert_eq!(signal_severity("tier_42"), Severity::NotChecked);
    }

    #[test]
    fn blank_summary_with_no_signals_is_empty() {
        let v = verdict("  ", vec![]);
        assert_eq!(v.summary, None);
        assert!(verdict_is_empty(&v));
    }

    #[test]
    fn summary_with_signals_is_not_empty() {
        let v = verdict(
            "looks healthy",
            vec![signal_row("Sharpe", "green", "It is fine.", "")],
        );
        assert_eq!(v.summary, Some("looks healthy".to_string()));
        assert!(!verdict_is_empty(&v));
    }

    #[test]
    fn signal_row_preserves_text_verbatim_and_never_trims() {
        let row = signal_row(
            "Walk-forward",
            "amber",
            "  whitespace kept  ",
            "  fix kept  ",
        );
        assert_eq!(row.name, "Walk-forward");
        assert_eq!(row.severity, Severity::Caution);
        // plain / fix are not trimmed — the engine's exact wording reaches the view.
        assert_eq!(row.plain, "  whitespace kept  ");
        assert_eq!(row.fix, "  fix kept  ");
    }

    #[test]
    fn empty_fix_is_allowed() {
        let row = signal_row("Stability", "green", "Stable.", "");
        assert_eq!(row.fix, "");
    }
}
