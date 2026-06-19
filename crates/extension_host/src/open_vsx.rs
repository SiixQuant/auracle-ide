//! Protocol adapter between the IDE's extension store and the [open VSX]
//! registry.
//!
//! The extension store speaks the Zed extension protocol: a JSON `/extensions`
//! API plus `.tar.gz` archives that already contain a Zed `extension.toml`,
//! Zed-format themes, tree-sitter grammars, and so on. open VSX is a VS Code
//! marketplace: it speaks a REST API and serves `.vsix` (zip) archives whose
//! contents are VS Code artifacts.
//!
//! A VS Code extension is not a Zed extension, so the two are not generally
//! interchangeable. The one class that converts cleanly is **color themes**:
//! VS Code workbench/token colors map onto Zed's theme schema (this is exactly
//! what the `theme_importer` tool already does). This adapter therefore scopes
//! the marketplace to the `Themes` category — search only surfaces themes, and
//! install downloads a `.vsix`, converts each contributed VS Code color theme
//! into a Zed theme family, and writes a synthesized Zed extension directory so
//! the rest of the install/index/load pipeline is unchanged.
//!
//! Language, grammar, language-server, and Wasm extensions are intentionally
//! out of scope: their VS Code artifacts (TextMate grammars, Node-hosted
//! servers, VS Code-API Wasm) have no Zed equivalent.
//!
//! [open VSX]: https://open-vsx.org

use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context as _, Result, bail};
use chrono::{DateTime, Utc};
use cloud_api_types::{ExtensionApiManifest, ExtensionMetadata, ExtensionProvides};
use collections::{BTreeMap, BTreeSet};
use futures::AsyncReadExt as _;
use http_client::{AsyncBody, HttpClient as _, HttpClientWithUrl};
use serde::{Deserialize, Serialize};
use theme_importer::{
    ThemeAppearanceJson, ThemeFamilyContent, ZED_THEME_SCHEMA_URL, convert_vscode_theme,
};
use url::Url;

/// The default open VSX registry root. Overridable with `ZED_EXTENSION_API_URL`
/// (the same env var the rest of the extension store honors).
const DEFAULT_OPEN_VSX_URL: &str = "https://open-vsx.org";

/// The schema version stamped onto synthesized manifests. open VSX themes are
/// converted into v1 Zed extensions; this must stay `<=` the extension store's
/// `CURRENT_SCHEMA_VERSION` so [`crate::is_version_compatible`] accepts them.
const SYNTHESIZED_SCHEMA_VERSION: i32 = 1;

/// How many results to request from a single open VSX search.
const SEARCH_PAGE_SIZE: usize = 50;

/// Returns the registry root (no trailing slash), honoring `ZED_EXTENSION_API_URL`.
fn registry_root() -> String {
    std::env::var("ZED_EXTENSION_API_URL")
        .unwrap_or_else(|_| DEFAULT_OPEN_VSX_URL.to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Builds a registry URL from a path under the root and a set of query params.
fn registry_url(path: &str, query: &[(&str, String)]) -> Result<Url> {
    let url = format!("{}{}", registry_root(), path);
    Ok(Url::parse_with_params(&url, query)?)
}

/// The human-facing extension page on open VSX, used as the "repository" link.
fn extension_page_url(namespace: &str, name: &str) -> String {
    format!("{}/extension/{namespace}/{name}", registry_root())
}

/// Splits an adapter extension id (`namespace.name`) into its parts.
fn split_extension_id(extension_id: &str) -> Result<(&str, &str)> {
    extension_id
        .split_once('.')
        .filter(|(namespace, name)| !namespace.is_empty() && !name.is_empty())
        .with_context(|| {
            format!("{extension_id:?} is not an open VSX extension id (namespace.name)")
        })
}

/// Parses an RFC 3339 timestamp, falling back to the Unix epoch.
fn parse_timestamp(timestamp: Option<&str>) -> DateTime<Utc> {
    timestamp
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())
        .map(|timestamp| timestamp.with_timezone(&Utc))
        .unwrap_or_else(|| {
            DateTime::<Utc>::from_timestamp(0, 0).expect("the Unix epoch is a valid timestamp")
        })
}

/// Issues a GET against the registry and returns the response body, surfacing
/// HTTP error statuses as errors.
async fn registry_get(http_client: &Arc<HttpClientWithUrl>, url: &Url) -> Result<Vec<u8>> {
    let mut response = http_client
        .get(url.as_str(), AsyncBody::empty(), true)
        .await
        .with_context(|| format!("requesting {url}"))?;

    let mut body = Vec::new();
    response
        .body_mut()
        .read_to_end(&mut body)
        .await
        .with_context(|| format!("reading response from {url}"))?;

    let status = response.status();
    if status.is_client_error() || status.is_server_error() {
        let text = String::from_utf8_lossy(&body);
        bail!(
            "open VSX request to {url} failed: {} {text:?}",
            status.as_u16()
        );
    }

    Ok(body)
}

// --- open VSX REST response shapes -----------------------------------------

#[derive(Debug, Deserialize)]
struct SearchResponse {
    #[serde(default)]
    extensions: Vec<SearchEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SearchEntry {
    name: String,
    namespace: String,
    version: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    download_count: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ExtensionResponse {
    name: String,
    namespace: String,
    version: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    download_count: u64,
    #[serde(default)]
    files: RegistryFiles,
    /// Map of version string (and aliases like `latest`) to its metadata URL.
    #[serde(default)]
    all_versions: BTreeMap<String, String>,
}

#[derive(Debug, Default, Deserialize)]
struct RegistryFiles {
    #[serde(default)]
    download: Option<String>,
}

impl SearchEntry {
    fn into_metadata(self) -> ExtensionMetadata {
        let id: Arc<str> = format!("{}.{}", self.namespace, self.name).into();
        ExtensionMetadata {
            id,
            manifest: ExtensionApiManifest {
                name: self.display_name.unwrap_or_else(|| self.name.clone()),
                version: self.version.into(),
                description: self.description,
                authors: vec![self.namespace.clone()],
                repository: extension_page_url(&self.namespace, &self.name),
                schema_version: Some(SYNTHESIZED_SCHEMA_VERSION),
                wasm_api_version: None,
                provides: BTreeSet::from_iter([ExtensionProvides::Themes]),
            },
            published_at: parse_timestamp(self.timestamp.as_deref()),
            download_count: self.download_count,
        }
    }
}

impl ExtensionResponse {
    fn to_metadata(&self) -> ExtensionMetadata {
        let id: Arc<str> = format!("{}.{}", self.namespace, self.name).into();
        ExtensionMetadata {
            id,
            manifest: ExtensionApiManifest {
                name: self
                    .display_name
                    .clone()
                    .unwrap_or_else(|| self.name.clone()),
                version: self.version.clone().into(),
                description: self.description.clone(),
                authors: vec![self.namespace.clone()],
                repository: extension_page_url(&self.namespace, &self.name),
                schema_version: Some(SYNTHESIZED_SCHEMA_VERSION),
                wasm_api_version: None,
                provides: BTreeSet::from_iter([ExtensionProvides::Themes]),
            },
            published_at: parse_timestamp(self.timestamp.as_deref()),
            download_count: self.download_count,
        }
    }
}

// --- search & metadata -----------------------------------------------------

/// Searches open VSX for compatible (color-theme) extensions.
///
/// Because only themes convert into Zed extensions, the query is always scoped
/// to the `Themes` category. If the caller's `provides_filter` excludes
/// `Themes`, there is nothing compatible to return.
pub async fn search_extensions(
    http_client: Arc<HttpClientWithUrl>,
    query: Option<String>,
    provides_filter: Option<BTreeSet<ExtensionProvides>>,
) -> Result<Vec<ExtensionMetadata>> {
    if let Some(provides_filter) = &provides_filter
        && !provides_filter.contains(&ExtensionProvides::Themes)
    {
        return Ok(Vec::new());
    }

    let mut params = vec![
        ("category", "Themes".to_string()),
        ("size", SEARCH_PAGE_SIZE.to_string()),
        ("includeAllVersions", "false".to_string()),
    ];
    if let Some(query) = query
        .as_deref()
        .map(str::trim)
        .filter(|query| !query.is_empty())
    {
        params.push(("query", query.to_string()));
    }

    let url = registry_url("/api/-/search", &params)?;
    let body = registry_get(&http_client, &url).await?;
    let response: SearchResponse =
        serde_json::from_slice(&body).context("parsing open VSX search response")?;

    Ok(response
        .extensions
        .into_iter()
        .map(SearchEntry::into_metadata)
        .collect())
}

/// Fetches metadata for every published version of an extension.
///
/// open VSX exposes per-version metadata URLs but not per-version manifests in
/// a single call, so each entry reuses the latest version's display metadata
/// with its own version string — enough to drive the version picker and to
/// install a specific version.
pub async fn fetch_extension_versions(
    http_client: Arc<HttpClientWithUrl>,
    extension_id: Arc<str>,
) -> Result<Vec<ExtensionMetadata>> {
    let (namespace, name) = split_extension_id(&extension_id)?;
    let url = registry_url(&format!("/api/{namespace}/{name}"), &[])?;
    let body = registry_get(&http_client, &url).await?;
    let response: ExtensionResponse =
        serde_json::from_slice(&body).context("parsing open VSX extension metadata")?;

    let latest = response.to_metadata();
    let mut versions = Vec::new();
    for version in response.all_versions.keys() {
        // open VSX keys this map by version *and* by aliases like `latest` /
        // `pre-release`; skip the non-version aliases.
        if version == "latest" || version == "pre-release" {
            continue;
        }
        let mut metadata = latest.clone();
        metadata.manifest.version = version.clone().into();
        versions.push(metadata);
    }

    if versions.is_empty() {
        versions.push(latest);
    }

    Ok(versions)
}

/// Fetches the latest metadata for a single extension (used for update checks).
pub async fn fetch_latest_metadata(
    http_client: Arc<HttpClientWithUrl>,
    extension_id: Arc<str>,
) -> Result<ExtensionMetadata> {
    let (namespace, name) = split_extension_id(&extension_id)?;
    let url = registry_url(&format!("/api/{namespace}/{name}"), &[])?;
    let body = registry_get(&http_client, &url).await?;
    let response: ExtensionResponse =
        serde_json::from_slice(&body).context("parsing open VSX extension metadata")?;
    Ok(response.to_metadata())
}

/// Resolves the `.vsix` download URL for a specific version (or the latest when
/// `version` is `None`).
pub async fn resolve_download_url(
    http_client: Arc<HttpClientWithUrl>,
    extension_id: Arc<str>,
    version: Option<Arc<str>>,
) -> Result<String> {
    let (namespace, name) = split_extension_id(&extension_id)?;
    let path = match version.as_deref() {
        Some(version) => format!("/api/{namespace}/{name}/{version}"),
        None => format!("/api/{namespace}/{name}"),
    };
    let url = registry_url(&path, &[])?;
    let body = registry_get(&http_client, &url).await?;
    let response: ExtensionResponse =
        serde_json::from_slice(&body).context("parsing open VSX extension metadata")?;

    response
        .files
        .download
        .with_context(|| format!("open VSX returned no .vsix download for {extension_id}"))
}

// --- .vsix -> Zed extension conversion -------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsixPackageJson {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    publisher: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    repository: Option<RepositoryField>,
    #[serde(default)]
    contributes: VsixContributes,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum RepositoryField {
    Url(String),
    Object { url: String },
}

impl RepositoryField {
    fn url(self) -> String {
        match self {
            RepositoryField::Url(url) => url,
            RepositoryField::Object { url } => url,
        }
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VsixContributes {
    #[serde(default)]
    themes: Vec<VsixThemeContribution>,
    /// Kept only to tell color themes apart from icon themes — both live under
    /// open VSX's `Themes` category, but only color themes are convertible.
    #[serde(default)]
    icon_themes: Vec<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct VsixThemeContribution {
    #[serde(default)]
    label: Option<String>,
    #[serde(default, rename = "uiTheme")]
    ui_theme: Option<String>,
    path: String,
}

/// The Zed `extension.toml` synthesized for a converted theme extension.
#[derive(Debug, Serialize)]
struct SynthesizedManifest {
    id: String,
    name: String,
    version: String,
    schema_version: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    authors: Vec<String>,
    repository: String,
}

/// Joins `relative` onto `root`, rejecting anything that escapes `root`.
fn resolve_within(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = relative.trim_start_matches("./");
    let relative = Path::new(relative);
    let escapes = relative.components().any(|component| {
        matches!(
            component,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    });
    if escapes {
        bail!("theme path {relative:?} escapes the extension directory");
    }
    Ok(root.join(relative))
}

/// Maps a VS Code `uiTheme` value onto a Zed theme appearance.
fn appearance_for_ui_theme(ui_theme: Option<&str>) -> ThemeAppearanceJson {
    match ui_theme {
        Some("vs") | Some("hc-light") => ThemeAppearanceJson::Light,
        // `vs-dark`, `hc-black`, or unspecified.
        _ => ThemeAppearanceJson::Dark,
    }
}

/// Turns an extension id into a filesystem-safe stem for the theme file.
fn theme_file_stem(extension_id: &str) -> String {
    extension_id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

/// Downloads-then-converts is split: callers fetch the `.vsix` bytes (so the
/// existing download/size-check path is reused) and hand them here. This
/// extracts the archive, converts every contributed VS Code color theme into a
/// Zed theme family, and writes a synthesized Zed extension into `dest_dir`
/// (`extension.toml` + `themes/<id>.json`).
///
/// Returns an error (leaving `dest_dir` partially populated, to be discarded by
/// the caller) when the `.vsix` contributes no convertible color themes — i.e.
/// when it is a non-theme VS Code extension that Zed cannot run.
pub async fn write_extension_from_vsix(
    vsix_bytes: &[u8],
    extension_id: &str,
    dest_dir: &Path,
) -> Result<()> {
    let scratch = tempfile::tempdir().context("creating scratch dir for .vsix extraction")?;
    util::archive::extract_zip(scratch.path(), futures::io::Cursor::new(vsix_bytes))
        .await
        .context("extracting .vsix archive")?;

    // VSIX archives nest their payload under `extension/`.
    let nested = scratch.path().join("extension");
    let package_root = if nested.join("package.json").exists() {
        nested
    } else {
        scratch.path().to_path_buf()
    };

    let package_json = std::fs::read(package_root.join("package.json"))
        .context("reading package.json from .vsix")?;
    let package: VsixPackageJson =
        serde_json_lenient::from_slice(&package_json).context("parsing package.json from .vsix")?;

    if package.contributes.themes.is_empty() {
        if !package.contributes.icon_themes.is_empty() {
            bail!(
                "{extension_id} is a VS Code icon theme; only color themes are supported \
                 on open VSX (icon themes are not yet convertible to Zed)"
            );
        }
        bail!(
            "{extension_id} is not a color-theme extension; only open VSX color themes are \
             supported (a VS Code extension is not a Zed extension)"
        );
    }

    let mut themes = Vec::new();
    for contribution in &package.contributes.themes {
        let theme_path = match resolve_within(&package_root, &contribution.path) {
            Ok(path) => path,
            Err(error) => {
                log::warn!("skipping theme in {extension_id}: {error:#}");
                continue;
            }
        };
        let theme_bytes = match std::fs::read(&theme_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "skipping theme {:?} in {extension_id}: {error:#}",
                    contribution.path
                );
                continue;
            }
        };

        let name = contribution
            .label
            .clone()
            .or_else(|| package.display_name.clone())
            .or_else(|| package.name.clone())
            .unwrap_or_else(|| extension_id.to_string());
        let appearance = appearance_for_ui_theme(contribution.ui_theme.as_deref());

        match convert_vscode_theme(name, appearance, &theme_bytes) {
            Ok(theme) => themes.push(theme),
            Err(error) => log::warn!(
                "skipping theme {:?} in {extension_id}: {error:#}",
                contribution.path
            ),
        }
    }

    if themes.is_empty() {
        bail!("none of the color themes in {extension_id} could be converted");
    }

    let family = ThemeFamilyContent {
        name: package
            .display_name
            .clone()
            .or_else(|| package.name.clone())
            .unwrap_or_else(|| extension_id.to_string()),
        author: package.publisher.clone().unwrap_or_default(),
        themes,
    };

    let mut family_value =
        serde_json::to_value(&family).context("serializing converted theme family")?;
    if let Some(object) = family_value.as_object_mut() {
        object.insert(
            "$schema".to_string(),
            serde_json::Value::String(ZED_THEME_SCHEMA_URL.to_string()),
        );
    }
    let family_json = serde_json::to_string_pretty(&family_value)?;

    let themes_dir = dest_dir.join("themes");
    std::fs::create_dir_all(&themes_dir).context("creating themes dir")?;
    std::fs::write(
        themes_dir.join(format!("{}.json", theme_file_stem(extension_id))),
        family_json,
    )
    .context("writing converted theme")?;

    let repository = package
        .repository
        .map(RepositoryField::url)
        .unwrap_or_else(|| {
            split_extension_id(extension_id)
                .map(|(namespace, name)| extension_page_url(namespace, name))
                .unwrap_or_default()
        });
    let manifest = SynthesizedManifest {
        id: extension_id.to_string(),
        name: package
            .display_name
            .or_else(|| package.name.clone())
            .unwrap_or_else(|| extension_id.to_string()),
        version: package.version.unwrap_or_else(|| "0.0.0".to_string()),
        schema_version: SYNTHESIZED_SCHEMA_VERSION,
        description: package.description,
        authors: package.publisher.into_iter().collect(),
        repository,
    };
    let manifest_toml = toml::to_string(&manifest).context("serializing synthesized manifest")?;
    std::fs::write(dest_dir.join("extension.toml"), manifest_toml)
        .context("writing synthesized extension.toml")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_zip::Compression;
    use async_zip::ZipEntryBuilder;
    use async_zip::base::write::ZipFileWriter;
    use futures::io::Cursor;

    #[test]
    fn maps_search_response_to_metadata() {
        let body = serde_json::json!({
            "offset": 0,
            "totalSize": 1,
            "extensions": [
                {
                    "url": "https://open-vsx.org/api/sdras/night-owl",
                    "name": "night-owl",
                    "namespace": "sdras",
                    "version": "2.1.1",
                    "timestamp": "2025-01-01T03:32:58.240458Z",
                    "displayName": "Night Owl",
                    "description": "A VS Code theme",
                    "downloadCount": 74581
                }
            ]
        })
        .to_string();

        let response: SearchResponse = serde_json::from_str(&body).unwrap();
        let metadata: Vec<_> = response
            .extensions
            .into_iter()
            .map(SearchEntry::into_metadata)
            .collect();

        assert_eq!(metadata.len(), 1);
        let entry = &metadata[0];
        assert_eq!(entry.id.as_ref(), "sdras.night-owl");
        assert_eq!(entry.manifest.name, "Night Owl");
        assert_eq!(entry.manifest.version.as_ref(), "2.1.1");
        assert_eq!(entry.manifest.schema_version, Some(1));
        assert!(entry.manifest.wasm_api_version.is_none());
        assert!(entry.manifest.provides.contains(&ExtensionProvides::Themes));
        assert_eq!(entry.download_count, 74581);
        assert_eq!(
            entry.manifest.repository,
            "https://open-vsx.org/extension/sdras/night-owl"
        );
    }

    #[test]
    fn rejects_non_theme_extension_id() {
        assert!(split_extension_id("not-an-id").is_err());
        assert!(split_extension_id(".name").is_err());
        assert!(split_extension_id("namespace.").is_err());
        assert_eq!(
            split_extension_id("ms-python.python").unwrap(),
            ("ms-python", "python")
        );
    }

    async fn build_vsix(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buffer = Cursor::new(Vec::new());
        let mut writer = ZipFileWriter::new(&mut buffer);
        for (name, data) in entries {
            let builder = ZipEntryBuilder::new((*name).into(), Compression::Stored);
            writer.write_entry_whole(builder, data).await.unwrap();
        }
        writer.close().await.unwrap();
        buffer.into_inner()
    }

    #[test]
    fn converts_vsix_color_theme_into_zed_extension() {
        futures::executor::block_on(async {
            let package_json = serde_json::json!({
                "name": "night-owl",
                "displayName": "Night Owl",
                "version": "2.1.1",
                "publisher": "sdras",
                "description": "A nice theme",
                "contributes": {
                    "themes": [
                        {
                            "label": "Night Owl",
                            "uiTheme": "vs-dark",
                            "path": "./themes/night-owl-color-theme.json"
                        }
                    ]
                }
            })
            .to_string();
            let theme_json = serde_json::json!({
                "name": "Night Owl",
                "colors": {
                    "editor.background": "#011627",
                    "editor.foreground": "#d6deeb"
                },
                "tokenColors": []
            })
            .to_string();

            let vsix = build_vsix(&[
                ("extension/package.json", package_json.as_bytes()),
                (
                    "extension/themes/night-owl-color-theme.json",
                    theme_json.as_bytes(),
                ),
            ])
            .await;

            let dest = tempfile::tempdir().unwrap();
            write_extension_from_vsix(&vsix, "sdras.night-owl", dest.path())
                .await
                .unwrap();

            let manifest = std::fs::read_to_string(dest.path().join("extension.toml")).unwrap();
            assert!(manifest.contains("id = \"sdras.night-owl\""));
            assert!(manifest.contains("schema_version = 1"));

            let theme =
                std::fs::read_to_string(dest.path().join("themes").join("sdras-night-owl.json"))
                    .unwrap();
            let family: ThemeFamilyContent = serde_json_lenient::from_str(&theme).unwrap();
            assert_eq!(family.themes.len(), 1);
            assert_eq!(family.themes[0].name, "Night Owl");
        });
    }

    #[test]
    fn rejects_non_theme_vsix() {
        futures::executor::block_on(async {
            let package_json = serde_json::json!({
                "name": "some-lsp",
                "displayName": "Some LSP",
                "version": "1.0.0",
                "publisher": "acme",
                "contributes": {}
            })
            .to_string();
            let vsix = build_vsix(&[("extension/package.json", package_json.as_bytes())]).await;

            let dest = tempfile::tempdir().unwrap();
            let result = write_extension_from_vsix(&vsix, "acme.some-lsp", dest.path()).await;
            assert!(result.is_err());
        });
    }
}
