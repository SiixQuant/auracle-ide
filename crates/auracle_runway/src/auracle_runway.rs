//! Honest glance markers for the Quant Runway rail's six stages.
//!
//! Each stage shows a one-word marker so its state reads without a hover. The
//! engine reports whether a stage is reached ("yes" / "no" / "unknown"); the
//! rail also knows which rung is current. This module maps that to the marker —
//! gpui-free, so it is unit-tested without rendering. Crucially, a stage the
//! engine *can't prove* reads "not tracked", never "soon": "soon" promises
//! progress we can't back, which the quality rubric forbids (item 1).

/// How a runway stage reads at a glance. Drives the icon and colour at render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StageMark {
    /// The rung you're on now.
    Current,
    /// Reached, with engine evidence.
    Done,
    /// Not reached yet — the engine can prove it isn't done.
    Pending,
    /// The engine can't tell yet — honestly "not tracked", never "soon".
    Untracked,
}

/// A stage's one-word glance marker plus how it reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageMarker {
    pub label: Option<String>,
    pub mark: StageMark,
}

/// Map a stage's engine `reached` state ("yes" / "no" / "unknown") and whether
/// it is the current rung to its honest glance marker. The current rung wins; an
/// engine-unprovable stage reads "not tracked", never "soon".
pub fn stage_marker(reached: &str, is_current: bool) -> StageMarker {
    if is_current {
        return StageMarker {
            label: Some("now".into()),
            mark: StageMark::Current,
        };
    }
    match reached {
        "yes" => StageMarker {
            label: None,
            mark: StageMark::Done,
        },
        "no" => StageMarker {
            label: Some("to do".into()),
            mark: StageMark::Pending,
        },
        // "unknown" — or any value the engine doesn't promise — reads honestly
        // as not-tracked, never "soon".
        _ => StageMarker {
            label: Some("not tracked".into()),
            mark: StageMark::Untracked,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_engine_unprovable_stage_is_not_tracked_never_soon() {
        let marker = stage_marker("unknown", false);
        assert_eq!(
            marker,
            StageMarker {
                label: Some("not tracked".into()),
                mark: StageMark::Untracked,
            }
        );
    }

    #[test]
    fn the_current_rung_reads_now() {
        let marker = stage_marker("no", true);
        assert_eq!(
            marker,
            StageMarker {
                label: Some("now".into()),
                mark: StageMark::Current,
            }
        );
    }

    #[test]
    fn a_reached_stage_is_done_with_no_marker() {
        let marker = stage_marker("yes", false);
        assert_eq!(
            marker,
            StageMarker {
                label: None,
                mark: StageMark::Done,
            }
        );
    }

    #[test]
    fn a_not_reached_stage_reads_to_do() {
        let marker = stage_marker("no", false);
        assert_eq!(
            marker,
            StageMarker {
                label: Some("to do".into()),
                mark: StageMark::Pending,
            }
        );
    }

    #[test]
    fn the_current_rung_wins_even_when_reached() {
        // Precedence: the active rung reads "now" regardless of reached state.
        assert_eq!(stage_marker("yes", true).mark, StageMark::Current);
    }

    #[test]
    fn an_unexpected_reached_value_falls_back_to_not_tracked() {
        // Honesty: never dress an unrecognised engine value as done or to-do.
        let marker = stage_marker("garbled", false);
        assert_eq!(marker.mark, StageMark::Untracked);
    }
}
