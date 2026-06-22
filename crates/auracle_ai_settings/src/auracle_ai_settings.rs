//! Honest models for the native AI "Model providers" settings subpage.
//!
//! The subpage shows two engine-sourced things the render must present without
//! embellishment: the engine's *designated default* AI model (does it have a
//! usable key?) and the *list of providers* with the default marked. Both
//! decisions are gpui-free so they are unit-tested without rendering.
//!
//! The engine names a provider by its vault-key (e.g. `deepseek_api_key`); the
//! IDE registry names it differently (e.g. `auracle-agent`). That translation
//! lives in `auracle_connections::engine_provider_to_ide` and is done by the
//! caller before it reaches this crate, so the reducers stay dependency-free.
//! See `RUBRIC.md` in the `auracle_view_state` crate (item 5, honesty): an
//! absent default marks nothing and invents no provider, model, or key state.

/// Tone for a status row, for the theme to colour at render time. Mirrors the
/// Account page's `LicenseTone` so the settings surfaces colour consistently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatusTone {
    /// No judgement (e.g. no default designated).
    Neutral,
    /// All good (a default with a usable key).
    Positive,
    /// Worth attention but not broken (a default whose key isn't configured).
    Caution,
    /// A problem the user should act on.
    Negative,
}

/// How the engine's designated default AI model reads on the AI subpage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineDefaultStatus {
    pub label: String,
    pub detail: Option<String>,
    pub tone: StatusTone,
}

/// Summarise the engine's designated default model honestly.
///
/// `provider_display` is the friendly provider name the caller resolved from the
/// registry. An absent or blank provider means the engine has no default — we
/// say exactly that and invent no model or key state. When a provider is
/// present, the key being configured is the difference between a `Positive` and
/// a `Caution` row; a blank `model_id` simply omits the model from the label.
pub fn engine_default_summary(
    provider_display: Option<&str>,
    model_id: Option<&str>,
    key_configured: bool,
) -> EngineDefaultStatus {
    let provider = provider_display
        .map(str::trim)
        .filter(|provider| !provider.is_empty());
    let Some(provider) = provider else {
        return EngineDefaultStatus {
            label: "No engine default set".into(),
            detail: None,
            tone: StatusTone::Neutral,
        };
    };

    let model = model_id.map(str::trim).filter(|model| !model.is_empty());
    let label = match model {
        Some(model) => format!("{provider} · {model}"),
        None => provider.to_string(),
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

/// Build the provider rows, marking the engine's default.
///
/// `engine_default_ide_id` is the registry id the caller already translated from
/// the engine vault-key name (via `auracle_connections::engine_provider_to_ide`).
/// `None`, a blank id, or an id that matches no provider marks nothing — we
/// never fabricate a default. Order and fields are preserved verbatim.
pub fn derive_provider_rows(
    providers: &[ProviderDescriptor],
    engine_default_ide_id: Option<&str>,
) -> Vec<AiProviderRow> {
    let default = engine_default_ide_id
        .map(str::trim)
        .filter(|id| !id.is_empty());
    providers
        .iter()
        .map(|provider| AiProviderRow {
            id: provider.id.clone(),
            display: provider.display.clone(),
            authenticated: provider.authenticated,
            is_engine_default: default == Some(provider.id.as_str()),
        })
        .collect()
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

    #[test]
    fn no_provider_means_no_default_and_no_fabrication() {
        let status = engine_default_summary(None, Some("claude-sonnet-4-6"), true);
        assert_eq!(status.label, "No engine default set");
        assert_eq!(status.detail, None);
        assert_eq!(status.tone, StatusTone::Neutral);
    }

    #[test]
    fn blank_provider_is_treated_as_no_default() {
        let status = engine_default_summary(Some("   "), Some("x"), true);
        assert_eq!(status.label, "No engine default set");
        assert_eq!(status.tone, StatusTone::Neutral);
    }

    #[test]
    fn configured_default_is_positive_with_provider_and_model() {
        let status = engine_default_summary(Some("Anthropic"), Some("claude-sonnet-4-6"), true);
        assert_eq!(status.label, "Anthropic · claude-sonnet-4-6");
        assert_eq!(status.detail.as_deref(), Some("Key configured"));
        assert_eq!(status.tone, StatusTone::Positive);
    }

    #[test]
    fn default_without_key_is_caution() {
        let status = engine_default_summary(Some("Anthropic"), Some("claude-sonnet-4-6"), false);
        assert_eq!(status.detail.as_deref(), Some("No key configured"));
        assert_eq!(status.tone, StatusTone::Caution);
    }

    #[test]
    fn missing_model_shows_provider_only() {
        let status = engine_default_summary(Some("Ollama"), None, true);
        assert_eq!(status.label, "Ollama");
        assert_eq!(status.tone, StatusTone::Positive);
    }

    #[test]
    fn blank_model_is_omitted_from_label() {
        let status = engine_default_summary(Some("Ollama"), Some("  "), true);
        assert_eq!(status.label, "Ollama");
    }

    #[test]
    fn rows_mark_the_engine_default_exactly_once() {
        let providers = vec![
            provider("anthropic", "Anthropic", true),
            provider("openai", "OpenAI", false),
            provider("ollama", "Ollama", true),
        ];
        let rows = derive_provider_rows(&providers, Some("openai"));
        let marked: Vec<&str> = rows
            .iter()
            .filter(|row| row.is_engine_default)
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(marked, vec!["openai"]);
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
        let providers = vec![provider("anthropic", "Anthropic", true)];
        let rows = derive_provider_rows(&providers, Some("does-not-exist"));
        assert!(rows.iter().all(|row| !row.is_engine_default));
    }

    #[test]
    fn no_default_marks_nothing() {
        let providers = vec![provider("anthropic", "Anthropic", true)];
        let rows = derive_provider_rows(&providers, None);
        assert!(rows.iter().all(|row| !row.is_engine_default));
    }

    #[test]
    fn empty_providers_yield_no_rows() {
        let rows = derive_provider_rows(&[], Some("anthropic"));
        assert!(rows.is_empty());
    }
}
