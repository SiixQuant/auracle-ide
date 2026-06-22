//! The four states every Auracle surface renders, behind one gpui-free seam.
//!
//! The Zed-parity rubric requires every surface to design its loading, empty,
//! error, and success states — never a blank panel or a raw spinner. To make
//! that checkable without rendering GPUI (which can't compile without the Metal
//! toolchain), the mapping from a fetch outcome to a rendered state lives here,
//! as a pure function over plain data. A surface's `render` becomes a thin
//! `match` over [`ViewState`]; this module is the single place that decides the
//! state, so no surface can silently forget one.
//!
//! The quality bar every surface is held to is written in `RUBRIC.md` next to
//! this crate.

/// What a surface renders right now.
pub enum ViewState<T> {
    /// The engine fetch is in flight — render a skeleton.
    Loading,
    /// The fetch succeeded but there is nothing to show — render a designed
    /// empty state carrying a hint about what would appear here.
    Empty { hint: String },
    /// The fetch failed — render a designed error state; `retryable` says
    /// whether a Retry affordance applies.
    Error { message: String, retryable: bool },
    /// The fetch succeeded and there is something to show.
    Ready(T),
}

impl<T> ViewState<T> {
    /// Whether the surface should offer a Retry affordance — true only for an
    /// error the caller marked retryable.
    pub fn should_retry(&self) -> bool {
        matches!(
            self,
            ViewState::Error {
                retryable: true,
                ..
            }
        )
    }
}

/// The outcome of an engine fetch, before it is mapped to a [`ViewState`].
pub enum Load<T> {
    /// The fetch is still running.
    Pending,
    /// The fetch errored, carrying the message to show.
    Failed(String),
    /// The fetch returned a value (which may still be "empty").
    Done(T),
}

impl<T> Load<T> {
    /// Map a fetch outcome to the state a surface should render. `is_empty`
    /// decides whether a successful payload is "empty"; `empty_hint` is shown
    /// in that case.
    pub fn into_view(
        self,
        is_empty: impl FnOnce(&T) -> bool,
        empty_hint: impl Into<String>,
    ) -> ViewState<T> {
        match self {
            Load::Pending => ViewState::Loading,
            Load::Failed(message) => ViewState::Error {
                message,
                retryable: true,
            },
            Load::Done(value) if is_empty(&value) => ViewState::Empty {
                hint: empty_hint.into(),
            },
            Load::Done(value) => ViewState::Ready(value),
        }
    }
}

impl<T> Load<Vec<T>> {
    /// Convenience for the common list/feed surfaces (runs, blotter, incidents,
    /// validation, strategies, schedules): an empty `Vec` is the empty state.
    pub fn into_list_view(self, empty_hint: impl Into<String>) -> ViewState<Vec<T>> {
        self.into_view(Vec::is_empty, empty_hint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_load_is_loading() {
        let state: ViewState<Vec<u8>> = Load::Pending.into_view(Vec::is_empty, "no rows yet");
        assert!(matches!(state, ViewState::Loading));
    }

    #[test]
    fn failed_load_is_a_retryable_error_carrying_the_message() {
        let state: ViewState<Vec<u8>> =
            Load::Failed("engine unreachable".into()).into_view(Vec::is_empty, "no rows yet");
        match state {
            ViewState::Error { message, retryable } => {
                assert_eq!(message, "engine unreachable");
                assert!(retryable);
            }
            other => panic!("expected Error, got {:?}", DebugState(&other)),
        }
    }

    #[test]
    fn done_load_with_a_non_empty_payload_is_ready_with_the_value() {
        let state = Load::Done(vec![1, 2, 3]).into_view(Vec::is_empty, "no rows yet");
        match state {
            ViewState::Ready(value) => assert_eq!(value, vec![1, 2, 3]),
            other => panic!("expected Ready, got {:?}", DebugState(&other)),
        }
    }

    #[test]
    fn done_load_with_an_empty_payload_is_empty_with_the_hint() {
        let state: ViewState<Vec<u8>> = Load::Done(vec![]).into_view(Vec::is_empty, "no rows yet");
        match state {
            ViewState::Empty { hint } => assert_eq!(hint, "no rows yet"),
            other => panic!("expected Empty, got {:?}", DebugState(&other)),
        }
    }

    #[test]
    fn into_list_view_uses_vec_emptiness() {
        let empty: ViewState<Vec<i32>> = Load::Done(vec![]).into_list_view("no runs yet");
        assert!(matches!(empty, ViewState::Empty { hint } if hint == "no runs yet"));

        let filled: ViewState<Vec<i32>> = Load::Done(vec![7]).into_list_view("no runs yet");
        assert!(matches!(filled, ViewState::Ready(rows) if rows == vec![7]));
    }

    #[test]
    fn only_a_retryable_error_should_show_retry() {
        let retryable: ViewState<()> = ViewState::Error {
            message: "timed out".into(),
            retryable: true,
        };
        let permanent: ViewState<()> = ViewState::Error {
            message: "unsupported platform".into(),
            retryable: false,
        };
        assert!(retryable.should_retry());
        assert!(!permanent.should_retry());
        assert!(!ViewState::<()>::Loading.should_retry());
        assert!(!ViewState::<()>::Empty { hint: "x".into() }.should_retry());
        assert!(!ViewState::Ready(()).should_retry());
    }
}

/// Test-only helper so panics on the wrong variant read clearly without
/// requiring `T: Debug`.
#[cfg(test)]
struct DebugState<'a, T>(&'a ViewState<T>);

#[cfg(test)]
impl<T> std::fmt::Debug for DebugState<'_, T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.0 {
            ViewState::Loading => "Loading",
            ViewState::Empty { .. } => "Empty",
            ViewState::Error { .. } => "Error",
            ViewState::Ready(_) => "Ready",
        };
        f.write_str(name)
    }
}
