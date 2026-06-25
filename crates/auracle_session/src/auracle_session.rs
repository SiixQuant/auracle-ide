//! The shared session aggregate for the "one core, three shells" architecture.
//!
//! Auracle presents the same working session through three co-equal shells —
//! **Desk** (the IDE today), **Copilot** (a conversation-first front door), and
//! **Flow** (a node canvas, shipping in v2). This crate owns the gpui-free state
//! they share, so switching shells is just a change of presentation, never a
//! change of context:
//!
//! * **D1 — equal switcher.** No shell is privileged; [`Session::new`] opens on
//!   [`Shell::Desk`] only as the continuity entry point, not as a default mode.
//! * **D2 — the working context travels.** The session owns the shared context
//!   (today: the [`ArtifactStore`]); [`Session::switch_shell`] never touches it,
//!   so an artifact opened in Copilot is the same instance Desk renders.
//! * **D4 — all three shells ship.** [`Shell::Flow`] (the node canvas) now ships
//!   alongside Desk and Copilot; [`Shell::available`] still gates the switch so a
//!   future shell can be staged, but no current shell is blocked.
//!
//! This crate holds no rendering — each shell renders this state itself.

use auracle_artifact::ArtifactStore;

/// One of the three co-equal presentations of a session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shell {
    Desk,
    Copilot,
    Flow,
}

impl Shell {
    /// Whether this shell is shippable. All three now ship — Flow (the node
    /// canvas) landed alongside Desk and Copilot. The gate is kept (rather than
    /// hard-coding `true`) so a future shell can be staged behind it without
    /// re-plumbing [`selectable`] or [`Session::switch_shell`].
    pub fn available(self) -> bool {
        match self {
            Shell::Desk | Shell::Copilot | Shell::Flow => true,
        }
    }

    /// The shells a switcher should currently offer, in display order.
    pub fn selectable() -> Vec<Shell> {
        [Shell::Desk, Shell::Copilot, Shell::Flow]
            .into_iter()
            .filter(|shell| shell.available())
            .collect()
    }
}

/// Outcome of a [`Session::switch_shell`] request, so the switcher chrome can
/// react honestly instead of silently no-opping.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SwitchOutcome {
    /// The active shell changed to the requested one.
    Switched,
    /// The requested shell was already active; nothing changed.
    AlreadyActive,
    /// The shell can't be entered yet (Flow in v1); the active shell is unchanged.
    Blocked { reason: String },
}

/// The shared session: which shell is active, plus the working context every
/// shell reads from. New context aggregates (active strategy, selected run,
/// connection snapshot) join the [`ArtifactStore`] here as later slices land —
/// the invariant is that [`Session::switch_shell`] leaves all of it untouched.
pub struct Session {
    active_shell: Shell,
    artifacts: ArtifactStore,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Session {
    /// Open a session on [`Shell::Desk`] — the continuity entry point, not a
    /// privileged default (decision D1).
    pub fn new() -> Self {
        Self {
            active_shell: Shell::Desk,
            artifacts: ArtifactStore::new(),
        }
    }

    pub fn active_shell(&self) -> Shell {
        self.active_shell
    }

    /// Switch the active shell. Switching to an unavailable shell (Flow in v1) is
    /// rejected and leaves the session unchanged; switching to the already-active
    /// shell is a no-op. The shared working context is never reset — that's the
    /// whole point of the switch (decision D2).
    pub fn switch_shell(&mut self, to: Shell) -> SwitchOutcome {
        if to == self.active_shell {
            return SwitchOutcome::AlreadyActive;
        }
        if !to.available() {
            return SwitchOutcome::Blocked {
                reason: format!("{to:?} isn't available yet — it ships in a later version."),
            };
        }
        self.active_shell = to;
        SwitchOutcome::Switched
    }

    /// The shared artifact registry — read by whichever shell is active.
    pub fn artifacts(&self) -> &ArtifactStore {
        &self.artifacts
    }

    /// Mutable access for the verbs that produce artifacts (backtests, drafts).
    pub fn artifacts_mut(&mut self) -> &mut ArtifactStore {
        &mut self.artifacts
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use auracle_artifact::ArtifactKind;

    #[test]
    fn session_opens_on_desk() {
        let session = Session::new();
        assert_eq!(session.active_shell(), Shell::Desk);
    }

    #[test]
    fn switching_between_available_shells_works() {
        let mut session = Session::new();
        assert_eq!(
            session.switch_shell(Shell::Copilot),
            SwitchOutcome::Switched
        );
        assert_eq!(session.active_shell(), Shell::Copilot);
        assert_eq!(session.switch_shell(Shell::Desk), SwitchOutcome::Switched);
        assert_eq!(session.active_shell(), Shell::Desk);
    }

    #[test]
    fn switching_to_the_active_shell_is_a_no_op() {
        let mut session = Session::new();
        assert_eq!(
            session.switch_shell(Shell::Desk),
            SwitchOutcome::AlreadyActive
        );
        assert_eq!(session.active_shell(), Shell::Desk);
    }

    #[test]
    fn switching_to_flow_now_works() {
        let mut session = Session::new();
        assert_eq!(session.switch_shell(Shell::Flow), SwitchOutcome::Switched);
        assert_eq!(session.active_shell(), Shell::Flow);
    }

    #[test]
    fn selectable_includes_all_three_shells() {
        assert_eq!(
            Shell::selectable(),
            vec![Shell::Desk, Shell::Copilot, Shell::Flow]
        );
        assert!(Shell::Desk.available());
        assert!(Shell::Copilot.available());
        assert!(Shell::Flow.available());
    }

    #[test]
    fn the_working_context_survives_a_shell_switch() {
        let mut session = Session::new();
        let id = session.artifacts_mut().begin(
            "backtest:momentum",
            ArtifactKind::BacktestResult,
            "Momentum",
        );
        session.artifacts_mut().succeed(id);

        // Switch shells — the artifact opened in one shell is the same instance
        // the next shell sees (decision D2).
        session.switch_shell(Shell::Copilot);

        let artifact = session
            .artifacts()
            .get(id)
            .expect("the artifact must survive the shell switch");
        assert_eq!(artifact.title, "Momentum");
        assert_eq!(session.artifacts().len(), 1);
    }
}
