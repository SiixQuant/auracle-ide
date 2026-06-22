//! The four states every Auracle surface renders, behind one gpui-free seam.
//!
//! Every surface must design its loading, empty, error, and success states —
//! never a blank panel or a raw spinner. To make that checkable without
//! rendering GPUI (which can't compile without the platform graphics toolchain),
//! the mapping from a fetch outcome to a rendered state lives here, as a pure
//! function over plain data. A surface's `render` becomes a thin `match` over
//! [`ViewState`]; this module is the single place that decides the loading,
//! empty, ready, and retryable-error states, so no surface can silently forget
//! one. A permanent (non-retryable) error is the one state a surface states
//! explicitly, via [`ViewState::permanent_error`].
//!
//! The quality bar every surface is held to is written in `RUBRIC.md` next to
//! this crate.

/// What a surface renders right now.
#[derive(Debug, Clone, PartialEq)]
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
    /// An error the surface should offer to retry — a transient failure such as
    /// the engine being briefly unreachable. This is what [`Load::into_view`]
    /// produces for any failed fetch.
    pub fn retryable_error(message: impl Into<String>) -> Self {
        ViewState::Error {
            message: message.into(),
            retryable: true,
        }
    }

    /// An error retrying cannot fix, such as an unsupported platform. Surfaces
    /// construct this explicitly; the fetch mapping never produces it.
    pub fn permanent_error(message: impl Into<String>) -> Self {
        ViewState::Error {
            message: message.into(),
            retryable: false,
        }
    }

    /// Whether the surface should offer a Retry affordance — true only for a
    /// retryable error.
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
#[derive(Debug, Clone, PartialEq)]
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
    /// decides whether a successful payload is "empty"; `empty_hint` is shown in
    /// that case. A failed fetch always maps to a *retryable* error; a surface
    /// with a permanent error uses [`ViewState::permanent_error`] directly.
    pub fn into_view(
        self,
        is_empty: impl FnOnce(&T) -> bool,
        empty_hint: impl Into<String>,
    ) -> ViewState<T> {
        match self {
            Load::Pending => ViewState::Loading,
            Load::Failed(message) => ViewState::retryable_error(message),
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
        assert_eq!(state, ViewState::Loading);
    }

    #[test]
    fn failed_load_is_a_retryable_error_carrying_the_message() {
        let state: ViewState<Vec<u8>> =
            Load::Failed("engine unreachable".into()).into_view(Vec::is_empty, "no rows yet");
        assert_eq!(state, ViewState::retryable_error("engine unreachable"));
    }

    #[test]
    fn done_load_with_a_non_empty_payload_is_ready_with_the_value() {
        let state = Load::Done(vec![1, 2, 3]).into_view(Vec::is_empty, "no rows yet");
        assert_eq!(state, ViewState::Ready(vec![1, 2, 3]));
    }

    #[test]
    fn done_load_with_an_empty_payload_is_empty_with_the_hint() {
        let state: ViewState<Vec<u8>> = Load::Done(vec![]).into_view(Vec::is_empty, "no rows yet");
        assert_eq!(
            state,
            ViewState::Empty {
                hint: "no rows yet".into()
            }
        );
    }

    #[test]
    fn emptiness_follows_the_supplied_predicate_not_the_payload_shape() {
        // The predicate decides Empty vs Ready — not the payload's intrinsic
        // shape. This is the whole reason `into_view` takes a predicate rather
        // than hard-coding a length check, and it's the contract the list and
        // non-list surfaces both rely on. Drive it so the predicate DISAGREES
        // with literal emptiness, in both directions, including a scalar.
        let scalar_empty = Load::Done(0u32).into_view(|n| *n == 0, "zero");
        assert_eq!(
            scalar_empty,
            ViewState::Empty {
                hint: "zero".into()
            }
        );

        let forced_empty = Load::Done(vec![1, 2, 3]).into_view(|_| true, "forced");
        assert_eq!(
            forced_empty,
            ViewState::Empty {
                hint: "forced".into()
            }
        );

        let forced_ready = Load::Done(Vec::<i32>::new()).into_view(|_| false, "unused");
        assert_eq!(forced_ready, ViewState::Ready(vec![]));
    }

    #[test]
    fn into_list_view_uses_vec_emptiness() {
        let empty: ViewState<Vec<i32>> = Load::Done(vec![]).into_list_view("no runs yet");
        assert_eq!(
            empty,
            ViewState::Empty {
                hint: "no runs yet".into()
            }
        );

        let filled: ViewState<Vec<i32>> = Load::Done(vec![7]).into_list_view("no runs yet");
        assert_eq!(filled, ViewState::Ready(vec![7]));
    }

    #[test]
    fn only_a_retryable_error_should_show_retry() {
        assert!(ViewState::<()>::retryable_error("timed out").should_retry());
        assert!(!ViewState::<()>::permanent_error("unsupported platform").should_retry());
        assert!(!ViewState::<()>::Loading.should_retry());
        assert!(!ViewState::<()>::Empty { hint: "x".into() }.should_retry());
        assert!(!ViewState::Ready(()).should_retry());
    }
}
