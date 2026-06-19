mod color;
pub mod vscode;

use anyhow::{Context as _, Result};
use collections::IndexMap;
use serde::Deserialize;
use theme::{Appearance, AppearanceContent};

pub use theme_settings::{ThemeContent, ThemeFamilyContent};
pub use vscode::{VsCodeTheme, VsCodeThemeConverter};

/// The JSON schema URL stamped onto themes that are exported in Zed's theme
/// format.
pub const ZED_THEME_SCHEMA_URL: &str = "https://zed.dev/schema/themes/v0.2.0.json";

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ThemeAppearanceJson {
    Light,
    Dark,
}

impl From<ThemeAppearanceJson> for AppearanceContent {
    fn from(value: ThemeAppearanceJson) -> Self {
        match value {
            ThemeAppearanceJson::Light => Self::Light,
            ThemeAppearanceJson::Dark => Self::Dark,
        }
    }
}

impl From<ThemeAppearanceJson> for Appearance {
    fn from(value: ThemeAppearanceJson) -> Self {
        match value {
            ThemeAppearanceJson::Light => Self::Light,
            ThemeAppearanceJson::Dark => Self::Dark,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct ThemeMetadata {
    pub name: String,
    pub file_name: String,
    pub appearance: ThemeAppearanceJson,
}

/// Converts a single VS Code color-theme JSON document into a Zed
/// [`ThemeContent`].
///
/// The input is parsed leniently (VS Code theme files frequently contain
/// comments and trailing commas), so callers can pass the raw bytes read out
/// of a `.vsix` archive directly.
pub fn convert_vscode_theme(
    name: String,
    appearance: ThemeAppearanceJson,
    vscode_theme_json: &[u8],
) -> Result<ThemeContent> {
    let vscode_theme: VsCodeTheme =
        serde_json_lenient::from_slice(vscode_theme_json).context("parsing VS Code color theme")?;
    let metadata = ThemeMetadata {
        name,
        file_name: String::new(),
        appearance,
    };

    VsCodeThemeConverter::new(vscode_theme, metadata, IndexMap::default()).convert()
}
