//! Theming and configuration. A built-in stylesheet is always applied as the
//! base, and a user file at `~/.config/nursearch/style.css` is overlaid on top
//! and watched for changes so edits take effect without restarting. Behaviour
//! settings and quicklinks live in `~/.config/nursearch/config.toml`.

use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use gtk4 as gtk;
use log::{debug, warn};
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};

/// Built-in stylesheet, always loaded as the base layer.
pub const DEFAULT_CSS: &str = include_str!("style.css");

/// Starter file written to `~/.config/nursearch/style.css`: comments only, so
/// the built-in theme (and its updates) apply until the user adds overrides.
const USER_CSS_TEMPLATE: &str = include_str!("style-user.css");

/// FNV-1a hashes of built-in themes that older versions copied verbatim into
/// the user file. Such an untouched copy would pin the old look forever, so it
/// is swapped for [`USER_CSS_TEMPLATE`]; edited files never match and are kept.
const LEGACY_DEFAULT_HASHES: &[u64] = &[
    0x500f_2cf6_9c62_ddbc, // v0.2.x – v0.3.0
];

/// `~/.config/nursearch`, honoring `XDG_CONFIG_HOME`.
pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("nursearch");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/nursearch")
}

/// Install the base theme plus the user overlay, returning the file monitor
/// that drives hot reloading. The monitor must be kept alive by the caller.
pub fn install_css() -> Option<gio::FileMonitor> {
    let display = gdk::Display::default()?;

    let base = gtk::CssProvider::new();
    base.load_from_data(DEFAULT_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &base,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let style_path = ensure_user_style();
    let user = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &user, gtk::STYLE_PROVIDER_PRIORITY_USER);
    load_user_style(&user, &style_path);

    let file = gio::File::for_path(&style_path);
    let monitor = file
        .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        .ok()?;
    monitor.connect_changed(move |_, _, _, _| {
        debug!("style.css changed; reloading theme");
        load_user_style(&user, &style_path);
    });
    Some(monitor)
}

/// Write the override template to the config dir on first run so users have a
/// starting point, and replace an untouched copy of an old built-in theme.
/// Returns the path either way.
fn ensure_user_style() -> PathBuf {
    let dir = config_dir();
    let path = dir.join("style.css");
    let replace = match fs::read(&path) {
        Ok(existing) => is_legacy_default(&existing),
        Err(_) => !path.exists(),
    };
    if replace {
        if let Err(err) =
            fs::create_dir_all(&dir).and_then(|()| fs::write(&path, USER_CSS_TEMPLATE))
        {
            warn!("could not write style.css template: {err}");
        } else {
            debug!("wrote style.css template to {}", path.display());
        }
    }
    path
}

/// Whether `css` is a byte-identical copy of a previously shipped built-in theme.
fn is_legacy_default(css: &[u8]) -> bool {
    LEGACY_DEFAULT_HASHES.contains(&fnv1a(css))
}

/// 64-bit FNV-1a; stable across Rust versions, unlike `DefaultHasher`.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    })
}

fn load_user_style(provider: &gtk::CssProvider, path: &PathBuf) {
    if path.exists() {
        provider.load_from_path(path);
    } else {
        // File removed: fall back to the built-in base only.
        provider.load_from_data("");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_template_parses_to_defaults_plus_quicklinks() {
        let settings = Settings::parse(SETTINGS_TEMPLATE).unwrap();
        assert_eq!(settings.window, WindowSettings::default());
        assert_eq!(settings.search, SearchSettings::default());
        assert!(settings.quicklinks.iter().any(|link| link.keyword == "aw"));
    }

    #[test]
    fn partial_settings_keep_defaults_and_clamp() {
        let settings =
            Settings::parse("[search]\nmax_results = 500\n[window]\nposition = \"top\"").unwrap();
        assert_eq!(settings.search.max_results, 50);
        assert_eq!(settings.window.position, Position::Top);
        assert_eq!(settings.window.width, 720);
        assert!(Settings::parse("").unwrap().quicklinks.is_empty());
    }

    #[test]
    fn unknown_keys_are_reported() {
        assert!(Settings::parse("[window]\nwidht = 800").is_err());
    }

    #[test]
    fn detects_untouched_legacy_theme_only() {
        let legacy = include_bytes!("../tests/fixtures/style-v0.3.0.css");
        assert!(is_legacy_default(legacy));

        let mut edited = legacy.to_vec();
        edited.extend_from_slice(b"\n.result-name { color: red; }\n");
        assert!(!is_legacy_default(&edited));
        assert!(!is_legacy_default(USER_CSS_TEMPLATE.as_bytes()));
        assert!(!is_legacy_default(DEFAULT_CSS.as_bytes()));
    }
}

/// Commented starter `config.toml`, written on first run.
const SETTINGS_TEMPLATE: &str = include_str!("settings-template.toml");

/// Contents of `config.toml`. Every field has a default, so a partial or
/// missing file is fine.
#[derive(Clone, Debug, Default, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub window: WindowSettings,
    pub search: SearchSettings,
    pub quicklinks: Vec<Quicklink>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[default]
    Center,
    Top,
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct WindowSettings {
    pub position: Position,
    pub width: i32,
    pub top_margin: i32,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            position: Position::Center,
            width: 720,
            top_margin: 160,
        }
    }
}

#[derive(Clone, Debug, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SearchSettings {
    pub max_results: usize,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self { max_results: 12 }
    }
}

/// A user-defined web shortcut: `keyword term` opens `url` with `{query}`
/// replaced; without `{query}` it is a plain bookmark.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Quicklink {
    pub keyword: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub icon: Option<String>,
}

impl Settings {
    /// Parse settings, clamping values that would break the window.
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut settings: Settings = toml::from_str(text).map_err(|err| err.to_string())?;
        settings.window.width = settings.window.width.clamp(360, 2000);
        settings.window.top_margin = settings.window.top_margin.clamp(0, 2000);
        settings.search.max_results = settings.search.max_results.clamp(1, 50);
        settings
            .quicklinks
            .retain(|link| !link.keyword.trim().is_empty() && !link.url.trim().is_empty());
        for link in &mut settings.quicklinks {
            link.keyword = link.keyword.trim().to_lowercase();
        }
        Ok(settings)
    }
}

/// Path of `config.toml`; written from the template on first run.
pub fn settings_path() -> PathBuf {
    let dir = config_dir();
    let path = dir.join("config.toml");
    if !path.exists()
        && let Err(err) =
            fs::create_dir_all(&dir).and_then(|()| fs::write(&path, SETTINGS_TEMPLATE))
    {
        warn!("could not write config.toml template: {err}");
    }
    path
}

/// Load settings from `path`. A missing file yields the defaults; a broken
/// one yields the defaults plus the error to show the user.
pub fn load_settings(path: &Path) -> (Settings, Option<String>) {
    match fs::read_to_string(path) {
        Ok(text) => match Settings::parse(&text) {
            Ok(settings) => (settings, None),
            Err(err) => {
                warn!("invalid {}: {err}", path.display());
                (Settings::default(), Some(err))
            }
        },
        Err(_) => (Settings::default(), None),
    }
}
