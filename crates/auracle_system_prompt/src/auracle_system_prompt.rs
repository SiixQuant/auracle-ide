//! Honest models for the native "Agent rules" settings subpage.
//!
//! The subpage surfaces the *real* sources the agent draws its default
//! system-prompt and reusable rules from — there is no fabricated editor and no
//! invented prompt text. Three sources exist in the IDE today:
//!
//! 1. the user-global `AGENTS.md` rules file (the agent's default standing
//!    instructions),
//! 2. the first worktree's project `AGENTS.md` (per-project overrides), and
//! 3. the reusable rule files held in the prompt store (Zed's rules library).
//!
//! The first two are plain files the render layer can open natively in an editor
//! buffer, so editing round-trips through the same file the agent reads. The
//! third lives in the prompt store's database; the render layer can only *open*
//! it (no fake in-page editor), so its row is informational and reports an
//! honest count plus whether a default is designated.
//!
//! Every decision a row's appearance depends on — its status line, its tone, and
//! whether an "Open" affordance applies — is derived here as a pure function over
//! plain facts, so it is unit-tested without rendering GPUI. The render layer is
//! a thin pass over [`derive_rows`]; it never re-derives status or openability.
//! Nothing here reads a file, touches the store, or invents a path, count, or
//! default — every input is supplied by the caller from the real sources.

/// Tone for a rule row's status, for the theme to colour at render time. Only
/// the tones the reducer can actually emit are modelled, so a reader (and a
/// future render `match`) never sees a state this crate cannot produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleTone {
    /// In effect / present (e.g. a rules file that exists on disk).
    Active,
    /// Reachable but not yet established (e.g. a rules file that would be created
    /// on first open, or a store with no default designated).
    Neutral,
}

/// Which real source a row stands for, so the render layer knows how to act on
/// an "Open" without re-deriving intent from a string. A file source can be
/// opened into an editor buffer; the store source is opened via the store's own
/// native surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleSource {
    /// The user-global `AGENTS.md` rules file.
    GlobalAgentsFile,
    /// The first worktree's project `AGENTS.md` rules file.
    ProjectAgentsFile,
    /// The reusable rule files held in the prompt store.
    PromptStore,
}

/// One row on the agent-rules subpage, fully resolved for rendering.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleRow {
    pub source: RuleSource,
    pub title: String,
    pub status: String,
    pub tone: RuleTone,
    /// Whether the render layer should offer an "Open" affordance for this row.
    /// A row is never hidden when it can't be opened — it stays visible with an
    /// honest status — so the user always sees the source exists.
    pub openable: bool,
}

/// Facts about the prompt store the caller reads once and passes in, rather than
/// this crate touching the store. `loaded` is false when the store is still
/// initialising or failed to open, so the row can say so honestly instead of
/// claiming zero rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PromptStoreFacts {
    /// Whether the store has been opened. When false, counts are not trusted.
    pub loaded: bool,
    /// Number of reusable rule files the store holds (excluding built-ins the
    /// caller chooses not to count). Ignored when `loaded` is false.
    pub rule_count: usize,
    /// Whether any stored rule is marked as the agent's default.
    pub has_default: bool,
}

/// The facts the caller gathers from the real sources before rendering. The
/// global rules file is always reachable (it is created on first open if
/// absent), so only its on-disk presence varies; the project file only exists as
/// a row when a worktree is open.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RulesFacts {
    /// Whether the user-global `AGENTS.md` already exists on disk.
    pub global_file_exists: bool,
    /// Whether a worktree is open (so a project `AGENTS.md` row applies at all).
    pub has_project_worktree: bool,
    /// Whether the project `AGENTS.md` already exists on disk. Only meaningful
    /// when `has_project_worktree` is true.
    pub project_file_exists: bool,
    pub prompt_store: PromptStoreFacts,
}

/// Status line + tone for a rules *file* row: an existing file is `Active`, an
/// absent one is `Neutral` and tells the user it will be created on first open
/// (so a missing file never reads as an error — opening it is the fix).
fn file_status(exists: bool) -> (String, RuleTone) {
    if exists {
        ("In effect".to_string(), RuleTone::Active)
    } else {
        (
            "Not created yet — opening it creates the file".to_string(),
            RuleTone::Neutral,
        )
    }
}

/// Status line + tone for the prompt-store row. An unloaded store reports that
/// honestly (never "0 rules"); a loaded store reports its count, pluralised, and
/// whether a default is designated.
fn store_status(facts: PromptStoreFacts) -> (String, RuleTone) {
    if !facts.loaded {
        return ("Rules library unavailable".to_string(), RuleTone::Neutral);
    }
    let rules = match facts.rule_count {
        0 => "No reusable rules yet".to_string(),
        1 => "1 reusable rule".to_string(),
        n => format!("{n} reusable rules"),
    };
    if facts.has_default {
        (format!("{rules} · default set"), RuleTone::Active)
    } else {
        (rules, RuleTone::Neutral)
    }
}

/// Build the rows for the agent-rules subpage from the gathered facts. Order is
/// fixed (global file, then project file when a worktree is open, then the
/// prompt store) so the surface reads the same every render. File rows are always
/// openable; the store row is openable only when the store loaded, because there
/// is nothing honest to open otherwise.
pub fn derive_rows(facts: RulesFacts) -> Vec<RuleRow> {
    let mut rows = Vec::new();

    let (status, tone) = file_status(facts.global_file_exists);
    rows.push(RuleRow {
        source: RuleSource::GlobalAgentsFile,
        title: "Global agent rules".to_string(),
        status,
        tone,
        openable: true,
    });

    if facts.has_project_worktree {
        let (status, tone) = file_status(facts.project_file_exists);
        rows.push(RuleRow {
            source: RuleSource::ProjectAgentsFile,
            title: "Project agent rules".to_string(),
            status,
            tone,
            openable: true,
        });
    }

    let (status, tone) = store_status(facts.prompt_store);
    rows.push(RuleRow {
        source: RuleSource::PromptStore,
        title: "Reusable rules library".to_string(),
        status,
        tone,
        openable: facts.prompt_store.loaded,
    });

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(loaded: bool, rule_count: usize, has_default: bool) -> PromptStoreFacts {
        PromptStoreFacts {
            loaded,
            rule_count,
            has_default,
        }
    }

    fn facts() -> RulesFacts {
        RulesFacts {
            global_file_exists: true,
            has_project_worktree: false,
            project_file_exists: false,
            prompt_store: store(true, 0, false),
        }
    }

    fn row(rows: &[RuleRow], source: RuleSource) -> &RuleRow {
        rows.iter()
            .find(|row| row.source == source)
            .expect("expected a row for the source")
    }

    #[test]
    fn no_worktree_omits_the_project_row_but_keeps_the_others() {
        let rows = derive_rows(facts());
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].source, RuleSource::GlobalAgentsFile);
        assert_eq!(rows[1].source, RuleSource::PromptStore);
        assert!(
            rows.iter()
                .all(|row| row.source != RuleSource::ProjectAgentsFile)
        );
    }

    #[test]
    fn worktree_inserts_the_project_row_between_global_and_store() {
        let rows = derive_rows(RulesFacts {
            has_project_worktree: true,
            ..facts()
        });
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].source, RuleSource::GlobalAgentsFile);
        assert_eq!(rows[1].source, RuleSource::ProjectAgentsFile);
        assert_eq!(rows[2].source, RuleSource::PromptStore);
    }

    #[test]
    fn an_existing_file_reads_as_active() {
        let rows = derive_rows(RulesFacts {
            global_file_exists: true,
            ..facts()
        });
        let global = row(&rows, RuleSource::GlobalAgentsFile);
        assert_eq!(global.status, "In effect");
        assert_eq!(global.tone, RuleTone::Active);
    }

    #[test]
    fn a_missing_file_says_it_will_be_created_and_is_still_openable() {
        let rows = derive_rows(RulesFacts {
            global_file_exists: false,
            ..facts()
        });
        let global = row(&rows, RuleSource::GlobalAgentsFile);
        assert_eq!(
            global.status,
            "Not created yet — opening it creates the file"
        );
        assert_eq!(global.tone, RuleTone::Neutral);
        assert!(
            global.openable,
            "a missing rules file must still be openable so it can be created"
        );
    }

    #[test]
    fn file_rows_are_always_openable() {
        let rows = derive_rows(RulesFacts {
            has_project_worktree: true,
            global_file_exists: false,
            project_file_exists: false,
            ..facts()
        });
        assert!(row(&rows, RuleSource::GlobalAgentsFile).openable);
        assert!(row(&rows, RuleSource::ProjectAgentsFile).openable);
    }

    #[test]
    fn an_unloaded_store_is_honest_and_not_openable() {
        let rows = derive_rows(RulesFacts {
            prompt_store: store(false, 0, false),
            ..facts()
        });
        let store_row = row(&rows, RuleSource::PromptStore);
        assert_eq!(store_row.status, "Rules library unavailable");
        assert_eq!(store_row.tone, RuleTone::Neutral);
        assert!(
            !store_row.openable,
            "an unloaded store has nothing honest to open"
        );
    }

    #[test]
    fn an_unloaded_store_never_claims_a_count_even_if_one_is_supplied() {
        // A stale count must not leak through when the store hasn't loaded.
        let rows = derive_rows(RulesFacts {
            prompt_store: store(false, 9, true),
            ..facts()
        });
        let store_row = row(&rows, RuleSource::PromptStore);
        assert_eq!(store_row.status, "Rules library unavailable");
        assert!(!store_row.openable);
    }

    #[test]
    fn a_loaded_empty_store_says_so_and_is_openable() {
        let rows = derive_rows(RulesFacts {
            prompt_store: store(true, 0, false),
            ..facts()
        });
        let store_row = row(&rows, RuleSource::PromptStore);
        assert_eq!(store_row.status, "No reusable rules yet");
        assert_eq!(store_row.tone, RuleTone::Neutral);
        assert!(store_row.openable);
    }

    #[test]
    fn a_single_rule_is_not_pluralised() {
        let rows = derive_rows(RulesFacts {
            prompt_store: store(true, 1, false),
            ..facts()
        });
        assert_eq!(
            row(&rows, RuleSource::PromptStore).status,
            "1 reusable rule"
        );
    }

    #[test]
    fn many_rules_are_pluralised() {
        let rows = derive_rows(RulesFacts {
            prompt_store: store(true, 4, false),
            ..facts()
        });
        assert_eq!(
            row(&rows, RuleSource::PromptStore).status,
            "4 reusable rules"
        );
    }

    #[test]
    fn a_designated_default_is_appended_and_marks_the_row_active() {
        let rows = derive_rows(RulesFacts {
            prompt_store: store(true, 3, true),
            ..facts()
        });
        let store_row = row(&rows, RuleSource::PromptStore);
        assert_eq!(store_row.status, "3 reusable rules · default set");
        assert_eq!(store_row.tone, RuleTone::Active);
    }

    #[test]
    fn an_empty_store_with_a_default_still_notes_the_default() {
        let rows = derive_rows(RulesFacts {
            prompt_store: store(true, 0, true),
            ..facts()
        });
        assert_eq!(
            row(&rows, RuleSource::PromptStore).status,
            "No reusable rules yet · default set"
        );
    }
}
