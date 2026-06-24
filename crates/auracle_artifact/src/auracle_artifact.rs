//! Session-scoped registry of the canonical "artifacts" the three Auracle shells
//! share — a strategy draft, a backtest result, a QuantConnect divergence
//! comparison, a connection snapshot, a validation report.
//!
//! Each artifact has exactly one canonical instance addressed by an
//! [`ArtifactId`] (decision D7: session-scoped, one instance). The Copilot
//! thread stores ids and resolves them here so its inline cards and the
//! Desk/Flow views render the *same* instance, never a divergent copy. Re-running
//! the work behind an artifact (a new backtest of the same strategy, say) updates
//! that instance in place — same id, bumped [`Artifact::revision`] — rather than
//! piling up duplicates.
//!
//! This crate is gpui-free and holds no rendering: only identity, kind,
//! lifecycle, and ordering. Shells read from it as the single source of truth and
//! must honour [`ArtifactStatus`] rather than assuming an artifact is ready
//! (decision D5: client mirrors fail closed).

use std::collections::HashMap;

/// Stable, session-scoped identity handed out by [`ArtifactStore`]. Never reused
/// within a session. [`ArtifactStore::get`] returns an `Option` so a shell that
/// holds an id across a store rebuild fails closed rather than panicking.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct ArtifactId(u64);

/// What kind of work product an artifact represents. Each maps to an existing
/// Auracle domain output so a shell knows which renderer to reach for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtifactKind {
    StrategyDraft,
    BacktestResult,
    Comparison,
    Connection,
    Validation,
}

/// Honest lifecycle of an artifact. Shells must render the status, never assume
/// `Ready`: a card for a still-building backtest shows progress, a `Stale` card
/// shows an out-of-date hint, and `Failed` shows the reason.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ArtifactStatus {
    Building,
    Ready,
    Stale,
    Failed { reason: String },
}

/// One canonical work product. `seq` is private: ordering is the store's concern,
/// exposed through [`ArtifactStore::timeline`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Artifact {
    pub id: ArtifactId,
    pub kind: ArtifactKind,
    pub title: String,
    pub status: ArtifactStatus,
    /// Bumped every time the work behind the artifact is re-run, so a shell can
    /// tell "same artifact, fresh result" from a brand-new one.
    pub revision: u32,
    seq: u64,
}

impl Artifact {
    /// Whether the artifact carries content a shell can act on. `Building` has
    /// nothing yet and `Failed` has no result, so only `Ready`/`Stale` qualify.
    pub fn is_actionable(&self) -> bool {
        matches!(self.status, ArtifactStatus::Ready | ArtifactStatus::Stale)
    }
}

/// The registry. One canonical artifact per logical key; ids and an update
/// sequence are issued internally so callers never have to mint either.
#[derive(Default)]
pub struct ArtifactStore {
    artifacts: Vec<Artifact>,
    by_key: HashMap<String, ArtifactId>,
    next_id: u64,
    next_seq: u64,
}

impl ArtifactStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Start (or restart) the work behind the artifact identified by
    /// `logical_key`. The first call for a key creates the artifact at revision
    /// 1; later calls reuse the same [`ArtifactId`], bump the revision, and reset
    /// it to [`ArtifactStatus::Building`] — modelling a re-run of the same work.
    /// Returns the canonical id either way.
    pub fn begin(
        &mut self,
        logical_key: &str,
        kind: ArtifactKind,
        title: impl Into<String>,
    ) -> ArtifactId {
        let seq = self.bump_seq();
        if let Some(&id) = self.by_key.get(logical_key) {
            if let Some(artifact) = self.artifacts.iter_mut().find(|a| a.id == id) {
                artifact.kind = kind;
                artifact.title = title.into();
                artifact.status = ArtifactStatus::Building;
                artifact.revision = artifact.revision.saturating_add(1);
                artifact.seq = seq;
                return id;
            }
        }
        let id = ArtifactId(self.next_id);
        self.next_id += 1;
        self.by_key.insert(logical_key.to_string(), id);
        self.artifacts.push(Artifact {
            id,
            kind,
            title: title.into(),
            status: ArtifactStatus::Building,
            revision: 1,
            seq,
        });
        id
    }

    /// Mark a `Building` artifact's result as available.
    pub fn succeed(&mut self, id: ArtifactId) {
        self.set_status(id, ArtifactStatus::Ready);
    }

    /// Mark the work behind an artifact as failed, with a user-facing reason.
    pub fn fail(&mut self, id: ArtifactId, reason: impl Into<String>) {
        self.set_status(
            id,
            ArtifactStatus::Failed {
                reason: reason.into(),
            },
        );
    }

    /// Flag a `Ready` artifact as out of date (its inputs changed). Only `Ready`
    /// artifacts can go stale — a `Building` run is already producing fresh
    /// content and a `Failed` one has nothing to stale.
    pub fn mark_stale(&mut self, id: ArtifactId) {
        if let Some(artifact) = self.artifacts.iter_mut().find(|a| a.id == id)
            && artifact.status == ArtifactStatus::Ready
        {
            artifact.status = ArtifactStatus::Stale;
            artifact.seq = self.next_seq;
            self.next_seq += 1;
        }
    }

    /// Resolve a (possibly stale) id to its current artifact, or `None` if this
    /// store never issued it — an honest miss the shell must render as "no longer
    /// available" rather than guessing.
    pub fn get(&self, id: ArtifactId) -> Option<&Artifact> {
        self.artifacts.iter().find(|a| a.id == id)
    }

    /// All artifacts, most-recently-touched first — the session timeline a shell
    /// renders as its conversation or activity feed.
    pub fn timeline(&self) -> Vec<&Artifact> {
        let mut ordered: Vec<&Artifact> = self.artifacts.iter().collect();
        ordered.sort_by_key(|artifact| std::cmp::Reverse(artifact.seq));
        ordered
    }

    pub fn len(&self) -> usize {
        self.artifacts.len()
    }

    pub fn is_empty(&self) -> bool {
        self.artifacts.is_empty()
    }

    fn set_status(&mut self, id: ArtifactId, status: ArtifactStatus) {
        let seq = self.bump_seq();
        if let Some(artifact) = self.artifacts.iter_mut().find(|a| a.id == id) {
            artifact.status = status;
            artifact.seq = seq;
        }
    }

    fn bump_seq(&mut self) -> u64 {
        let seq = self.next_seq;
        self.next_seq += 1;
        seq
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn begin_creates_a_building_artifact_at_revision_one() {
        let mut store = ArtifactStore::new();
        let id = store.begin(
            "backtest:momentum",
            ArtifactKind::BacktestResult,
            "Momentum",
        );
        let artifact = store.get(id).expect("just created");
        assert_eq!(artifact.kind, ArtifactKind::BacktestResult);
        assert_eq!(artifact.title, "Momentum");
        assert_eq!(artifact.status, ArtifactStatus::Building);
        assert_eq!(artifact.revision, 1);
        assert!(!artifact.is_actionable());
    }

    #[test]
    fn rerunning_the_same_key_reuses_the_id_and_bumps_the_revision() {
        let mut store = ArtifactStore::new();
        let first = store.begin(
            "backtest:momentum",
            ArtifactKind::BacktestResult,
            "Momentum",
        );
        store.succeed(first);

        let second = store.begin(
            "backtest:momentum",
            ArtifactKind::BacktestResult,
            "Momentum v2",
        );
        assert_eq!(
            first, second,
            "same logical key keeps one canonical instance"
        );
        assert_eq!(store.len(), 1);

        let artifact = store.get(second).expect("still present");
        assert_eq!(artifact.revision, 2);
        assert_eq!(artifact.title, "Momentum v2");
        assert_eq!(
            artifact.status,
            ArtifactStatus::Building,
            "a re-run is building again"
        );
    }

    #[test]
    fn distinct_keys_get_distinct_ids() {
        let mut store = ArtifactStore::new();
        let a = store.begin("backtest:a", ArtifactKind::BacktestResult, "A");
        let b = store.begin("backtest:b", ArtifactKind::BacktestResult, "B");
        assert_ne!(a, b);
        assert_eq!(store.len(), 2);
    }

    #[test]
    fn succeed_and_fail_set_terminal_status() {
        let mut store = ArtifactStore::new();
        let ok = store.begin("c:ok", ArtifactKind::Comparison, "OK");
        store.succeed(ok);
        assert_eq!(store.get(ok).unwrap().status, ArtifactStatus::Ready);
        assert!(store.get(ok).unwrap().is_actionable());

        let bad = store.begin("c:bad", ArtifactKind::Comparison, "Bad");
        store.fail(bad, "engine unreachable");
        assert_eq!(
            store.get(bad).unwrap().status,
            ArtifactStatus::Failed {
                reason: "engine unreachable".into()
            }
        );
        assert!(!store.get(bad).unwrap().is_actionable());
    }

    #[test]
    fn only_ready_artifacts_go_stale() {
        let mut store = ArtifactStore::new();

        let ready = store.begin("v:ready", ArtifactKind::Validation, "Ready");
        store.succeed(ready);
        store.mark_stale(ready);
        assert_eq!(store.get(ready).unwrap().status, ArtifactStatus::Stale);
        assert!(store.get(ready).unwrap().is_actionable());

        let building = store.begin("v:building", ArtifactKind::Validation, "Building");
        store.mark_stale(building);
        assert_eq!(
            store.get(building).unwrap().status,
            ArtifactStatus::Building,
            "a building artifact is already producing fresh content"
        );

        let failed = store.begin("v:failed", ArtifactKind::Validation, "Failed");
        store.fail(failed, "boom");
        store.mark_stale(failed);
        assert!(matches!(
            store.get(failed).unwrap().status,
            ArtifactStatus::Failed { .. }
        ));
    }

    #[test]
    fn timeline_is_most_recently_touched_first() {
        let mut store = ArtifactStore::new();
        let a = store.begin("k:a", ArtifactKind::StrategyDraft, "A");
        let b = store.begin("k:b", ArtifactKind::StrategyDraft, "B");
        // Touch A again so it becomes the most recent.
        store.succeed(a);

        let order: Vec<_> = store
            .timeline()
            .iter()
            .map(|artifact| artifact.id)
            .collect();
        assert_eq!(order, vec![a, b]);

        // Touching B last moves it to the front.
        store.succeed(b);
        let order: Vec<_> = store
            .timeline()
            .iter()
            .map(|artifact| artifact.id)
            .collect();
        assert_eq!(order, vec![b, a]);
    }

    #[test]
    fn get_resolves_each_id_to_its_own_artifact() {
        let mut store = ArtifactStore::new();
        let a = store.begin("k:a", ArtifactKind::Connection, "Connection A");
        let b = store.begin("k:b", ArtifactKind::StrategyDraft, "Draft B");

        assert_eq!(store.get(a).unwrap().title, "Connection A");
        assert_eq!(store.get(a).unwrap().kind, ArtifactKind::Connection);
        assert_eq!(store.get(b).unwrap().title, "Draft B");
        assert_eq!(store.get(b).unwrap().kind, ArtifactKind::StrategyDraft);
        assert_ne!(a, b);
    }
}
