//! Honest per-row decisions for the three feed surfaces (runs, blotter,
//! incidents): map an engine kind/status/severity string to a display tone,
//! and decide order cancellability. gpui-free so each decision is unit-tested
//! without rendering. The render layer maps FeedTone -> theme Color/Hsla; this
//! crate never emits a colour and never rewrites the engine's plain text.

/// A row's tone, for the theme to colour at render time. Exactly the tones the
/// feeds can emit — no render `match` ever sees a state this crate can't produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeedTone {
    /// No judgement / default muted (unknown kind, neutral event).
    Neutral,
    /// On its way / informational (orders in flight, info incidents).
    Info,
    /// All good (succeeded / filled).
    Positive,
    /// Worth attention, not broken (warnings, skipped).
    Caution,
    /// A problem the user should act on (failed / rejected / error).
    Negative,
    /// Done-but-inert (cancelled order) — distinct from Neutral so render can
    /// pick the theme's `ignored` colour, matching today's blotter behaviour.
    Ignored,
}

/// Runs-dock event tone from the engine event `kind`. Order of checks is
/// load-bearing and mirrors the current render exactly (failed/error first,
/// then order.*, then warning/skipped, then succeeded/filled), so behaviour is
/// preserved; only the location changes. Unknown kinds are Neutral — never
/// dramatised. Tolerant of surrounding whitespace, case-sensitive on the engine's
/// canonical lowercase kinds.
pub fn event_tone(kind: &str) -> FeedTone {
    // Trim only — the engine's kinds are canonical lowercase, so matching is
    // case-sensitive on purpose; a surrounding space must not change the tone.
    let kind = kind.trim();

    // Predicates and branch order mirror the current `RunsDock::kind_color`
    // exactly (`ends_with`/exact match, not `contains`), so behaviour is
    // byte-for-byte preserved — only the location changes:
    //
    //   ends_with("failed") || == "log.error"      -> error    (Negative)
    //   starts_with("order.")                       -> info     (Info)
    //   == "log.warning" || ends_with("skipped")    -> warning  (Caution)
    //   ends_with("succeeded") || ends_with("filled") -> success (Positive)
    //   else                                        -> muted    (Neutral)
    //
    // Branch order is load-bearing: a "*.failed" wins even when it also starts
    // with "order." (an `order.failed` is Negative). And because `order.*` is
    // checked before the succeeded/filled branch, an `order.filled` is Info, not
    // Positive — the success branch only reaches non-order `*.filled` kinds. This
    // is the render's behaviour today; preserving it is the bar.
    if kind.ends_with("failed") || kind == "log.error" {
        FeedTone::Negative
    } else if kind.starts_with("order.") {
        FeedTone::Info
    } else if kind == "log.warning" || kind.ends_with("skipped") {
        FeedTone::Caution
    } else if kind.ends_with("succeeded") || kind.ends_with("filled") {
        FeedTone::Positive
    } else {
        // Unknown / neutral event: never dramatised — honest muted default.
        FeedTone::Neutral
    }
}

/// Blotter order tone from the engine order `status`. Lowercased before match
/// (engine statuses are canonical but the broker snapshot words vary in case).
/// filled/executed -> Positive; rejected/failed/error -> Negative;
/// cancelled/canceled -> Ignored; everything else (submitted/pending/routed/…)
/// -> Info ("on its way"). Unknown -> Info, matching today's `_ => info`.
pub fn order_tone(status: &str) -> FeedTone {
    match status.trim().to_ascii_lowercase().as_str() {
        "filled" | "executed" => FeedTone::Positive,
        "rejected" | "failed" | "error" => FeedTone::Negative,
        "cancelled" | "canceled" => FeedTone::Ignored,
        // Everything in flight (submitted/pending/routed/…) and any unknown
        // status reads as "on its way" — honest, never guessed as good or broken.
        _ => FeedTone::Info,
    }
}

/// Incident severity tone. error -> Negative; warning -> Caution; anything else
/// (incl. "info" and unknown) -> Info. Matches `severity_color` today.
pub fn severity_tone(severity: &str) -> FeedTone {
    match severity.trim().to_ascii_lowercase().as_str() {
        "error" => FeedTone::Negative,
        "warning" => FeedTone::Caution,
        // "info", "notice", and any unknown severity read as Info — never
        // upgraded to a dramatic tone the engine did not send.
        _ => FeedTone::Info,
    }
}

/// Whether an order is still working at the broker and therefore cancellable.
/// Positive allowlist (lowercased): pending | sent | partially_filled | working
/// | open | submitted | routed. dry_run is NOT cancellable (preview, never
/// reached a broker); terminal statuses (executed/filled/cancelled/rejected/
/// failed) are done. Identical to the current `BlotterPanel::is_cancellable`.
pub fn is_cancellable(status: &str) -> bool {
    // Closed allowlist: any string not listed is `false`, so a new or unknown
    // engine status never shows a Cancel button the route would reject. dry_run
    // is deliberately absent — a preview never reached a broker to cancel.
    matches!(
        status.trim().to_ascii_lowercase().as_str(),
        "pending" | "sent" | "partially_filled" | "working" | "open" | "submitted" | "routed"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    // event_tone

    #[test]
    fn event_tone_errors_and_failures_are_negative() {
        assert_eq!(event_tone("log.error"), FeedTone::Negative);
        assert_eq!(event_tone("backtest.failed"), FeedTone::Negative);
    }

    #[test]
    fn event_tone_orders_in_flight_are_info() {
        assert_eq!(event_tone("order.submitted"), FeedTone::Info);
    }

    #[test]
    fn event_tone_order_failed_is_negative_not_info() {
        // Decision order matters: a failed order is Negative even though it also
        // starts with "order." — the failed branch wins, matching render today.
        assert_eq!(event_tone("order.failed"), FeedTone::Negative);
    }

    #[test]
    fn event_tone_warnings_and_skips_are_caution() {
        assert_eq!(event_tone("log.warning"), FeedTone::Caution);
        assert_eq!(event_tone("job.skipped"), FeedTone::Caution);
    }

    #[test]
    fn event_tone_succeeded_and_filled_are_positive() {
        assert_eq!(event_tone("backtest.succeeded"), FeedTone::Positive);
        // A non-order `*.filled` reaches the success branch -> Positive.
        assert_eq!(event_tone("position.filled"), FeedTone::Positive);
    }

    #[test]
    fn event_tone_order_filled_is_info_not_positive() {
        // Behaviour-preservation: `RunsDock::kind_color` checks `starts_with(
        // "order.")` BEFORE the succeeded/filled branch, so an `order.filled`
        // renders Info today, never Positive. The spec's prose decision order
        // (order.* before succeeded/filled) and the live render agree on Info;
        // the spec's case-5 literal `order.filled -> Positive` contradicts both,
        // so it is treated as a spec error and the honest, behaviour-preserving
        // value is pinned here instead.
        assert_eq!(event_tone("order.filled"), FeedTone::Info);
    }

    #[test]
    fn event_tone_unmatched_kind_is_neutral() {
        assert_eq!(event_tone("strategy.started"), FeedTone::Neutral);
    }

    #[test]
    fn event_tone_tolerates_surrounding_whitespace() {
        assert_eq!(event_tone("  log.error  "), FeedTone::Negative);
    }

    // order_tone

    #[test]
    fn order_tone_filled_and_executed_are_positive_case_insensitive() {
        assert_eq!(order_tone("filled"), FeedTone::Positive);
        assert_eq!(order_tone("EXECUTED"), FeedTone::Positive);
    }

    #[test]
    fn order_tone_rejected_failed_error_are_negative() {
        assert_eq!(order_tone("rejected"), FeedTone::Negative);
        assert_eq!(order_tone("failed"), FeedTone::Negative);
        assert_eq!(order_tone("error"), FeedTone::Negative);
    }

    #[test]
    fn order_tone_cancelled_either_spelling_is_ignored() {
        assert_eq!(order_tone("Cancelled"), FeedTone::Ignored);
        assert_eq!(order_tone("canceled"), FeedTone::Ignored);
    }

    #[test]
    fn order_tone_in_flight_and_unknown_are_info() {
        assert_eq!(order_tone("submitted"), FeedTone::Info);
        assert_eq!(order_tone("pending"), FeedTone::Info);
        assert_eq!(order_tone("routed"), FeedTone::Info);
        assert_eq!(order_tone(""), FeedTone::Info);
    }

    // severity_tone

    #[test]
    fn severity_tone_maps_error_warning_and_falls_back_to_info() {
        assert_eq!(severity_tone("error"), FeedTone::Negative);
        assert_eq!(severity_tone("warning"), FeedTone::Caution);
        assert_eq!(severity_tone("info"), FeedTone::Info);
        assert_eq!(severity_tone("notice"), FeedTone::Info);
    }

    // is_cancellable

    #[test]
    fn is_cancellable_allows_every_working_status_case_insensitive() {
        for status in [
            "pending",
            "sent",
            "partially_filled",
            "working",
            "open",
            "submitted",
            "routed",
        ] {
            assert!(is_cancellable(status), "{status} should be cancellable");
        }
        assert!(is_cancellable("WORKING"));
    }

    #[test]
    fn is_cancellable_excludes_dry_run() {
        // Honesty guard: a preview never reached a broker, so it is not cancellable.
        assert!(!is_cancellable("dry_run"));
    }

    #[test]
    fn is_cancellable_excludes_terminal_statuses() {
        for status in ["executed", "filled", "cancelled", "rejected", "failed"] {
            assert!(
                !is_cancellable(status),
                "{status} should not be cancellable"
            );
        }
    }

    #[test]
    fn is_cancellable_is_a_closed_allowlist() {
        assert!(!is_cancellable(""));
        assert!(!is_cancellable("some_new_status"));
    }
}
