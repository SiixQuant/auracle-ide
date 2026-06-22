//! Honest models for the native AI "Model providers" settings subpage.
//!
//! The subpage shows two engine-sourced things the render must present without
//! embellishment: the engine's *designated default* AI model (does it have a
//! usable key?) and the *list of providers* with that default marked. Both
//! decisions are gpui-free so they are unit-tested without rendering.
//!
//! Both reads resolve the default through the SAME input — the registry id the
//! caller obtained by translating the engine's vault-key name (e.g. a
//! `<provider>_api_key` slot) to the IDE registry id — then look that id up in
//! the one provider list. So the header can only ever describe a default that
//! also appears (and is marked) in the list below it; if the engine's default
//! has no matching visible provider, both reads honestly report "no default"
//! rather than inventing one. Nothing here fabricates a provider, model, or key
//! state. Label length and truncation are deliberately left to the render layer,
//! so this crate never alters provider or model text beyond trimming.

/// Tone for a status row, for the theme to colour at render time. Only the tones
/// the reducers can actually emit are modelled, so a reader (and a future render
/// `match`) never sees a state this crate cannot produce.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// No judgement (e.g. no default designated).
    Neutral,
    /// All good (a default with a usable key).
    Positive,
    /// Worth attention but not broken (a default whose key isn't configured).
    Caution,
}

/// How the engine's designated default AI model reads on the AI subpage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDefaultStatus {
    pub label: String,
    pub detail: Option<String>,
    pub tone: StatusTone,
}

/// A provider as the caller extracts it from the language-model registry
/// (already filtered to the visible providers). Kept dependency-free so the
/// derivation is unit-tested without gpui.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderDescriptor {
    pub id: String,
    pub display: String,
    pub authenticated: bool,
}

/// One row in the provider list on the AI subpage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderRow {
    pub id: String,
    pub display: String,
    pub authenticated: bool,
    pub is_engine_default: bool,
}

/// Whether a provider id is the engine default. Forgiving of surrounding
/// whitespace on either side (so a padded id can't silently fail to match), but
/// case-sensitive: registry ids are canonical, so a case difference names a
/// different provider, not the same one.
fn ids_match(default: &str, candidate: &str) -> bool {
    default.trim() == candidate.trim()
}

/// Index of the first provider whose id matches the engine default, if any. A
/// `None`, empty, or whitespace-only default — or one that matches no provider —
/// yields `None`, so callers never fabricate a default. "First" so a duplicated
/// id can never produce two defaults.
fn default_index(
    providers: &[ProviderDescriptor],
    engine_default_ide_id: Option<&str>,
) -> Option<usize> {
    let default = engine_default_ide_id
        .map(str::trim)
        .filter(|id| !id.is_empty())?;
    providers
        .iter()
        .position(|provider| ids_match(default, &provider.id))
}

/// Build the provider rows, marking the engine default. Only the first provider
/// whose id matches is marked, so the list never shows two defaults even if the
/// registry ever hands us a duplicate id. Order and fields are preserved
/// verbatim; ids are compared tolerantly (see [`ids_match`]) but never rewritten.
pub fn derive_provider_rows(
    providers: &[ProviderDescriptor],
    engine_default_ide_id: Option<&str>,
) -> Vec<AiProviderRow> {
    let marked = default_index(providers, engine_default_ide_id);
    providers
        .iter()
        .enumerate()
        .map(|(index, provider)| AiProviderRow {
            id: provider.id.clone(),
            display: provider.display.clone(),
            authenticated: provider.authenticated,
            is_engine_default: Some(index) == marked,
        })
        .collect()
}

/// Summarise the engine's designated default model honestly, reading the SAME
/// providers + id the list uses so the header can never claim a default the list
/// doesn't show. When the default matches no visible provider (absent, blank, or
/// a provider the IDE doesn't model), the row reads "No engine default set" with
/// no detail and no invented model or key state. When it does match, the display
/// name comes from that provider; a blank `model_id` is omitted from the label;
/// and the engine's `key_configured` is the only thing that decides `Positive`
/// vs `Caution`.
pub fn engine_default_summary(
    providers: &[ProviderDescriptor],
    engine_default_ide_id: Option<&str>,
    model_id: Option<&str>,
    key_configured: bool,
) -> EngineDefaultStatus {
    let provider =
        default_index(providers, engine_default_ide_id).and_then(|index| providers.get(index));
    let Some(provider) = provider else {
        return EngineDefaultStatus {
            label: "No engine default set".into(),
            detail: None,
            tone: StatusTone::Neutral,
        };
    };

    let provider_display = provider.display.trim();
    let model = model_id.map(str::trim).filter(|model| !model.is_empty());
    let label = match model {
        Some(model) => format!("{provider_display} · {model}"),
        None => provider_display.to_string(),
    };
    let (detail, tone) = if key_configured {
        (Some("Key configured".into()), StatusTone::Positive)
    } else {
        (Some("No key configured".into()), StatusTone::Caution)
    };
    EngineDefaultStatus {
        label,
        detail,
        tone,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str, display: &str, authenticated: bool) -> ProviderDescriptor {
        ProviderDescriptor {
            id: id.to_string(),
            display: display.to_string(),
            authenticated,
        }
    }

    fn three() -> Vec<ProviderDescriptor> {
        vec![
            provider("anthropic", "Anthropic", true),
            provider("openai", "OpenAI", false),
            provider("ollama", "Ollama", true),
        ]
    }

    // ── derive_provider_rows ──────────────────────────────────────────

    #[test]
    fn marks_only_the_matching_provider() {
        let rows = derive_provider_rows(&three(), Some("openai"));
        let marked: Vec<&str> = rows
            .iter()
            .filter(|row| row.is_engine_default)
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(marked, vec!["openai"]);
    }

    #[test]
    fn duplicate_ids_mark_only_the_first() {
        let providers = vec![
            provider("anthropic", "Anthropic", true),
            provider("openai", "OpenAI", true),
            provider("openai", "OpenAI (dupe)", false),
        ];
        let rows = derive_provider_rows(&providers, Some("openai"));
        assert!(!rows[0].is_engine_default);
        assert!(rows[1].is_engine_default);
        assert!(
            !rows[2].is_engine_default,
            "a duplicate id must not yield a second default"
        );
    }

    #[test]
    fn rows_preserve_order_and_fields() {
        let providers = vec![
            provider("anthropic", "Anthropic", true),
            provider("ollama", "Ollama", false),
        ];
        let rows = derive_provider_rows(&providers, None);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "anthropic");
        assert_eq!(rows[0].display, "Anthropic");
        assert!(rows[0].authenticated);
        assert_eq!(rows[1].id, "ollama");
        assert!(!rows[1].authenticated);
    }

    #[test]
    fn unknown_default_marks_nothing() {
        let rows = derive_provider_rows(&three(), Some("does-not-exist"));
        assert!(rows.iter().all(|row| !row.is_engine_default));
    }

    #[test]
    fn no_default_marks_nothing() {
        let rows = derive_provider_rows(&three(), None);
        assert!(rows.iter().all(|row| !row.is_engine_default));
    }

    #[test]
    fn empty_string_default_marks_nothing() {
        let rows = derive_provider_rows(&three(), Some(""));
        assert!(rows.iter().all(|row| !row.is_engine_default));
    }

    #[test]
    fn whitespace_only_default_marks_nothing() {
        let rows = derive_provider_rows(&three(), Some("   "));
        assert!(rows.iter().all(|row| !row.is_engine_default));
    }

    #[test]
    fn padded_default_matches_clean_id() {
        let rows = derive_provider_rows(&three(), Some("  openai  "));
        assert!(
            rows.iter()
                .any(|row| row.id == "openai" && row.is_engine_default)
        );
    }

    #[test]
    fn padded_provider_id_matches_clean_default() {
        let providers = vec![provider("  openai  ", "OpenAI", false)];
        let rows = derive_provider_rows(&providers, Some("openai"));
        assert!(
            rows[0].is_engine_default,
            "a padded descriptor id must still match a clean default"
        );
    }

    #[test]
    fn id_match_is_case_sensitive() {
        let rows = derive_provider_rows(&three(), Some("OpenAI"));
        assert!(
            rows.iter().all(|row| !row.is_engine_default),
            "a case-only difference names a different provider and must not mark"
        );
    }

    #[test]
    fn empty_providers_yield_no_rows() {
        let rows = derive_provider_rows(&[], Some("anthropic"));
        assert!(rows.is_empty());
    }

    // ── engine_default_summary ────────────────────────────────────────

    #[test]
    fn no_matching_provider_means_no_default() {
        let status = engine_default_summary(&three(), None, Some("claude-sonnet-4-6"), true);
        assert_eq!(status.label, "No engine default set");
        assert_eq!(status.detail, None);
        assert_eq!(status.tone, StatusTone::Neutral);
    }

    #[test]
    fn default_absent_from_visible_list_invents_nothing() {
        // The engine names a default the visible list doesn't contain: the header
        // must not assert a model or key state the list can't corroborate.
        let providers = vec![provider("anthropic", "Anthropic", true)];
        let status = engine_default_summary(&providers, Some("openai"), Some("gpt-x"), true);
        assert_eq!(status.label, "No engine default set");
        assert_eq!(status.detail, None);
        assert_eq!(status.tone, StatusTone::Neutral);
    }

    #[test]
    fn configured_default_is_positive_with_provider_and_model() {
        let status =
            engine_default_summary(&three(), Some("anthropic"), Some("claude-sonnet-4-6"), true);
        assert_eq!(status.label, "Anthropic · claude-sonnet-4-6");
        assert_eq!(status.detail.as_deref(), Some("Key configured"));
        assert_eq!(status.tone, StatusTone::Positive);
    }

    #[test]
    fn default_without_key_is_caution() {
        let status = engine_default_summary(
            &three(),
            Some("anthropic"),
            Some("claude-sonnet-4-6"),
            false,
        );
        assert_eq!(status.detail.as_deref(), Some("No key configured"));
        assert_eq!(status.tone, StatusTone::Caution);
    }

    #[test]
    fn missing_model_shows_provider_only() {
        let status = engine_default_summary(&three(), Some("ollama"), None, true);
        assert_eq!(status.label, "Ollama");
        assert_eq!(status.tone, StatusTone::Positive);
    }

    #[test]
    fn blank_model_is_omitted_from_label() {
        let status = engine_default_summary(&three(), Some("ollama"), Some("  "), true);
        assert_eq!(status.label, "Ollama");
    }

    #[test]
    fn padded_provider_display_and_model_are_trimmed_in_label() {
        let providers = vec![provider("anthropic", "  Anthropic  ", true)];
        let status =
            engine_default_summary(&providers, Some("anthropic"), Some("  claude-x  "), true);
        assert_eq!(status.label, "Anthropic · claude-x");
    }
}
