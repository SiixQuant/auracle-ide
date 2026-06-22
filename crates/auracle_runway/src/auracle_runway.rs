//! Honest presentation of a runway stage marker, for the native runway rail.
//!
//! The engine reports each stage with a raw `reached` verdict
//! ("yes"|"no"|"unknown") and which stage is `current`. This module turns that
//! into the exact glance the rail draws — and never overstates it: a stage the
//! engine can't yet prove reads "not tracked" (never the banned "soon"), and a
//! stage with no engine truth at all reads Locked (never Todo — absence of data
//! is not a claim the stage isn't done). The decision is gpui-free so it is
//! unit-tested without rendering. Mirrors `auracle_account`'s tone+summary shape.

/// How a runway stage reads at a glance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageMark {
    /// The active rung the engine reports as `current`.
    Now,
    /// Engine proved this stage reached (`reached == "yes"`).
    Done,
    /// Engine cannot yet prove this stage (`reached == "unknown"`).
    /// NOT "soon" — the engine doesn't track it yet.
    NotTracked,
    /// Not reached and engine is explicit it isn't (`reached == "no"`).
    Todo,
    /// No engine truth at all for this stage yet (offline / pre-connect).
    Locked,
}

/// Glance tone for the theme to colour at render time (mirrors LicenseTone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageTone {
    Accent,
    Positive,
    Muted,
    Disabled,
}

/// The full per-stage decision the rail renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageMarker {
    pub mark: StageMark,
    /// One-word glance label, or None when the row needs no marker (Done).
    /// Honest vocabulary only: "now" | "not tracked" | "to do". Never "soon".
    pub label: Option<&'static str>,
    pub mark_tone: StageTone,
    /// Tone for the stage name text.
    pub name_tone: StageTone,
    /// Whether the leading icon is the reached (check) vs locked glyph.
    pub reached_icon: bool,
}

/// Decide a stage's marker from the engine's `reached` verdict and whether this
/// is the `current` rung. `reached` is the raw engine string
/// ("yes"|"no"|"unknown"); anything unrecognised is treated as NotTracked when
/// truth is present (the engine can't prove it) and never dressed up as Done
/// (honesty — cf. auracle_account "Unknown").
///
/// `has_truth` is false when the engine returned no stage entry at all (offline
/// or pre-first-poll) — that is Locked, distinct from an explicit "no" (Todo).
pub fn stage_marker(reached: &str, is_current: bool, has_truth: bool) -> StageMarker {
    // Absence of any engine truth is Locked — never a claim the stage isn't done,
    // and it ignores `reached`/`is_current` entirely (a stale claim must not leak
    // through when we have no fresh evidence).
    if !has_truth {
        return StageMarker {
            mark: StageMark::Locked,
            label: None,
            mark_tone: StageTone::Disabled,
            name_tone: StageTone::Disabled,
            reached_icon: false,
        };
    }

    // The active rung wins over `reached` for icon/tone: it always reads "now".
    // Its icon is the check glyph only when the engine also proved it reached.
    if is_current {
        return StageMarker {
            mark: StageMark::Now,
            label: Some("now"),
            mark_tone: StageTone::Accent,
            name_tone: StageTone::Accent,
            reached_icon: reached == "yes",
        };
    }

    match reached {
        "yes" => StageMarker {
            mark: StageMark::Done,
            label: None,
            mark_tone: StageTone::Muted,
            // why: Done is a proven-good rung; Positive keeps the name legible
            // without a marker, staying inside the listed {Accent,Positive,
            // Muted,Disabled} tone set (no Default variant) — the render maps it.
            name_tone: StageTone::Positive,
            reached_icon: true,
        },
        "no" => StageMarker {
            mark: StageMark::Todo,
            label: Some("to do"),
            mark_tone: StageTone::Muted,
            name_tone: StageTone::Disabled,
            reached_icon: false,
        },
        // "unknown" and anything unrecognised collapse to NotTracked: the engine
        // can't prove it, so we never claim "soon" and never claim Done.
        _ => StageMarker {
            mark: StageMark::NotTracked,
            label: Some("not tracked"),
            mark_tone: StageTone::Muted,
            name_tone: StageTone::Disabled,
            reached_icon: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_done_is_now() {
        let marker = stage_marker("yes", true, true);
        assert_eq!(
            marker,
            StageMarker {
                mark: StageMark::Now,
                label: Some("now"),
                mark_tone: StageTone::Accent,
                name_tone: StageTone::Accent,
                reached_icon: true,
            }
        );
    }

    #[test]
    fn reached_not_current_is_done_no_marker() {
        let marker = stage_marker("yes", false, true);
        assert_eq!(marker.mark, StageMark::Done);
        assert_eq!(marker.label, None);
        assert!(marker.reached_icon);
    }

    #[test]
    fn unknown_is_not_tracked_never_soon() {
        let marker = stage_marker("unknown", false, true);
        assert_eq!(marker.mark, StageMark::NotTracked);
        assert_eq!(marker.label, Some("not tracked"));
        assert_ne!(marker.label, Some("soon"));
    }

    #[test]
    fn explicit_no_is_todo() {
        let marker = stage_marker("no", false, true);
        assert_eq!(marker.mark, StageMark::Todo);
        assert_eq!(marker.label, Some("to do"));
    }

    #[test]
    fn current_overrides_unproven() {
        let marker = stage_marker("no", true, true);
        assert_eq!(marker.mark, StageMark::Now);
        assert_eq!(marker.label, Some("now"));
    }

    #[test]
    fn no_truth_is_locked() {
        let marker = stage_marker("", false, false);
        assert_eq!(marker.mark, StageMark::Locked);
        assert_eq!(marker.label, None);
        assert_eq!(marker.mark_tone, StageTone::Disabled);
        assert_eq!(marker.name_tone, StageTone::Disabled);
    }

    #[test]
    fn unrecognised_reached_is_not_tracked_not_done() {
        // Honesty: an unrecognised verdict can't be proven, so NotTracked, never Done.
        let marker = stage_marker("weird", false, true);
        assert_eq!(marker.mark, StageMark::NotTracked);
        assert_ne!(marker.mark, StageMark::Done);
    }

    #[test]
    fn locked_ignores_reached_and_current() {
        // Absence of fresh truth beats a stale "yes"/current claim.
        let marker = stage_marker("yes", true, false);
        assert_eq!(marker.mark, StageMark::Locked);
    }
}
