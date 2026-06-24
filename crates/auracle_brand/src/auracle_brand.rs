//! Single source of truth for Auracle's brand identifiers and the legacy Zed
//! values they replace.
//!
//! Auracle is a rebranded fork of Zed. Completing the rename means renaming
//! user-facing functional identifiers (the project-local settings folder, the
//! deep-link URL scheme, env/task variables, documentation links) to Auracle
//! names *without breaking existing installs* — so each canonical name is paired
//! with the legacy name the IDE must still read as a fallback ("dual-read").
//!
//! Everything brand-related lives here so a future audit never has to hunt for a
//! stray literal: define it once, reference it everywhere.

/// The product's display name.
pub const DISPLAY_NAME: &str = "Auracle";

/// Canonical project-local settings folder (`<project>/.auracle/…`).
pub const LOCAL_SETTINGS_FOLDER: &str = ".auracle";
/// Legacy project-local settings folder, still read as a fallback.
pub const LEGACY_LOCAL_SETTINGS_FOLDER: &str = ".zed";

/// Canonical deep-link URL scheme (`auracle://…`).
pub const URL_SCHEME: &str = "auracle";
/// Legacy deep-link URL scheme, still resolved as a fallback.
pub const LEGACY_URL_SCHEME: &str = "zed";

/// Canonical environment / task variable prefix.
pub const ENV_PREFIX: &str = "AURACLE_";
/// Legacy environment / task variable prefix, still read as a fallback.
pub const LEGACY_ENV_PREFIX: &str = "ZED_";

/// Base URL for Auracle documentation. Every "learn more" / docs link is built
/// from this so the docs domain is a one-line change.
pub const DOCS_BASE: &str = "https://docs.aurapointcapital.com";

/// The legacy documentation host that links are migrated away from.
pub const LEGACY_DOCS_HOST: &str = "zed.dev";

/// Build an Auracle documentation URL for a doc `path`.
///
/// Accepts the path with or without a leading slash; the result always has
/// exactly one separating slash. `docs_url("/docs/debugger")` and
/// `docs_url("docs/debugger")` both yield
/// `https://docs.aurapointcapital.com/docs/debugger`.
pub fn docs_url(path: &str) -> String {
    let trimmed = path.trim_start_matches('/');
    format!("{DOCS_BASE}/{trimmed}")
}

/// Rewrite a legacy `https://zed.dev/...` documentation URL to the Auracle docs
/// base, preserving the path. Returns `None` when `url` is not a `zed.dev` URL,
/// so callers can leave non-docs links untouched.
pub fn rewrite_legacy_docs_url(url: &str) -> Option<String> {
    let rest = url
        .strip_prefix("https://zed.dev/")
        .or_else(|| url.strip_prefix("http://zed.dev/"))
        .or_else(|| url.strip_prefix("https://www.zed.dev/"))?;
    Some(docs_url(rest))
}

/// The canonical and legacy names for an env/task variable `suffix`, in the
/// order they should be read (canonical first). E.g. `env_var_names("FILE")`
/// → `["AURACLE_FILE", "ZED_FILE"]`. Use the first form when *setting* a
/// variable for new consumers, and read both (canonical wins) for back-compat.
pub fn env_var_names(suffix: &str) -> [String; 2] {
    [
        format!("{ENV_PREFIX}{suffix}"),
        format!("{LEGACY_ENV_PREFIX}{suffix}"),
    ]
}

/// The project-local settings folder names to probe, canonical first. Loaders
/// should read the first that exists; writers should always target the first.
pub const fn local_settings_folders() -> [&'static str; 2] {
    [LOCAL_SETTINGS_FOLDER, LEGACY_LOCAL_SETTINGS_FOLDER]
}

/// Whether `scheme` (without the `://`) is one the IDE should resolve — either
/// the canonical Auracle scheme or the legacy Zed scheme.
pub fn is_known_url_scheme(scheme: &str) -> bool {
    scheme == URL_SCHEME || scheme == LEGACY_URL_SCHEME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn docs_url_normalizes_leading_slash() {
        let expected = "https://docs.aurapointcapital.com/docs/debugger";
        assert_eq!(docs_url("/docs/debugger"), expected);
        assert_eq!(docs_url("docs/debugger"), expected);
    }

    #[test]
    fn docs_url_handles_empty_and_anchors() {
        assert_eq!(docs_url(""), "https://docs.aurapointcapital.com/");
        assert_eq!(
            docs_url("docs/completions#edit-predictions"),
            "https://docs.aurapointcapital.com/docs/completions#edit-predictions"
        );
    }

    #[test]
    fn rewrite_maps_zed_docs_to_auracle() {
        assert_eq!(
            rewrite_legacy_docs_url("https://zed.dev/docs/debugger").as_deref(),
            Some("https://docs.aurapointcapital.com/docs/debugger")
        );
        assert_eq!(
            rewrite_legacy_docs_url("http://zed.dev/docs/git").as_deref(),
            Some("https://docs.aurapointcapital.com/docs/git")
        );
    }

    #[test]
    fn rewrite_ignores_non_zed_urls() {
        assert_eq!(rewrite_legacy_docs_url("https://github.com/x"), None);
        // The cloud/api hosts are not docs and must not be rewritten here.
        assert_eq!(rewrite_legacy_docs_url("https://api.zed.dev/x"), None);
    }

    #[test]
    fn env_var_names_canonical_first() {
        assert_eq!(
            env_var_names("FILE"),
            ["AURACLE_FILE".to_string(), "ZED_FILE".to_string()]
        );
    }

    #[test]
    fn local_settings_folders_canonical_first() {
        assert_eq!(local_settings_folders(), [".auracle", ".zed"]);
    }

    #[test]
    fn known_schemes() {
        assert!(is_known_url_scheme("auracle"));
        assert!(is_known_url_scheme("zed"));
        assert!(!is_known_url_scheme("http"));
    }
}
