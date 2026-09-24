//! KDE System Settings pages (KCMs) as search results, so "bluetooth", "hdr"
//! or "maus" opens the matching settings page directly.
//!
//! Names, descriptions, icons and search keywords come from the Qt plugin
//! metadata embedded in each KCM's `.so` (an ELF note holding CBOR). The
//! `kcm_*.desktop` aliases only carry a name, while the embedded keywords are
//! what makes a query like "resolution" find "Display Configuration".

use crate::i18n;
use ciborium::Value;
use log::{debug, warn};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

/// Plugin subdirectories whose KCMs open inside `systemsettings`.
const KCM_SUBDIRS: &[&str] = &[
    "plasma/kcms/systemsettings",
    "plasma/kcms/systemsettings_qwidgets",
];

/// Marker preceding the metadata in Qt 6 plugins (`.note.qt.metadata`).
const QT_NOTE_NAME: &[u8] = b"qt-project!\0";

/// The metadata note sits near the start of the file; never read more.
const MAX_SCAN_BYTES: u64 = 512 * 1024;

/// One System Settings page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsPage {
    /// Plugin id, e.g. `kcm_kscreen`; passed to `systemsettings`.
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub icon: Option<String>,
    /// Lowercased search keywords, English plus the session language.
    pub keywords: Vec<String>,
}

/// Discover the settings pages of the running Plasma session. Returns nothing
/// outside KDE or when `systemsettings` is missing, so other desktops never
/// show results they cannot open.
pub fn discover() -> Vec<SettingsPage> {
    let is_kde = std::env::var("XDG_CURRENT_DESKTOP")
        .map(|desktops| desktops.split(':').any(|desktop| desktop == "KDE"))
        .unwrap_or(false);
    if !is_kde || !in_path("systemsettings") {
        return Vec::new();
    }

    let mut pages: Vec<SettingsPage> = plugin_dirs()
        .iter()
        .flat_map(|dir| KCM_SUBDIRS.iter().map(move |sub| dir.join(sub)))
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "so"))
        .filter_map(|path| read_page(&path))
        .collect();
    // The same plugin can exist in several plugin dirs; the first one wins.
    pages.sort_by(|a, b| a.id.cmp(&b.id));
    pages.dedup_by(|a, b| a.id == b.id);
    debug!("discovered {} settings pages", pages.len());
    pages
}

fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::var("QT_PLUGIN_PATH")
        .map(|paths| std::env::split_paths(&paths).collect())
        .unwrap_or_default();
    dirs.push(PathBuf::from("/usr/lib/qt6/plugins"));
    dirs
}

fn in_path(program: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|paths| std::env::split_paths(&paths).any(|dir| dir.join(program).is_file()))
}

fn read_page(path: &Path) -> Option<SettingsPage> {
    let id = path.file_stem()?.to_str()?.to_string();
    let mut bytes = Vec::new();
    if let Err(err) =
        File::open(path).and_then(|file| file.take(MAX_SCAN_BYTES).read_to_end(&mut bytes))
    {
        warn!("could not read settings page {}: {err}", path.display());
        return None;
    }
    let metadata = embedded_metadata(&bytes)?;
    page_from_metadata(id, &metadata, language_code())
}

/// Decode the plugin metadata map from a Qt 6 plugin binary.
fn embedded_metadata(bytes: &[u8]) -> Option<Value> {
    let start = find(bytes, QT_NOTE_NAME)?;
    // Four header bytes (format version, Qt major, Qt minor, flags) precede
    // the CBOR map; its key 4 holds the plugin's JSON metadata.
    let cbor = bytes.get(start + QT_NOTE_NAME.len() + 4..)?;
    let root: Value = ciborium::from_reader(cbor).ok()?;
    map_get(&root, &Value::Integer(4.into())).cloned()
}

fn page_from_metadata(id: String, metadata: &Value, lang: &str) -> Option<SettingsPage> {
    let plugin = map_get_str(metadata, "KPlugin")?;
    let name = localized(plugin, "Name", lang)?;
    if name.is_empty() || !shows_on_this_platform(metadata) {
        return None;
    }

    let mut keywords: Vec<String> = [
        text(metadata, "X-KDE-Keywords"),
        localized_only(metadata, "X-KDE-Keywords", lang),
    ]
    .into_iter()
    .flatten()
    .flat_map(|list| {
        list.split(',')
            .map(|keyword| keyword.trim().to_lowercase())
            .collect::<Vec<_>>()
    })
    .filter(|keyword| !keyword.is_empty())
    .collect();
    keywords.sort();
    keywords.dedup();

    Some(SettingsPage {
        id,
        name,
        description: localized(plugin, "Description", lang).filter(|text| !text.is_empty()),
        icon: text(plugin, "Icon").filter(|icon| !icon.is_empty()),
        keywords,
    })
}

/// KCMs can be limited to X11 or Wayland; hide the ones for the other one.
fn shows_on_this_platform(metadata: &Value) -> bool {
    let Some(platforms) = text(metadata, "X-KDE-OnlyShowOnQtPlatforms") else {
        return true;
    };
    let current = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        "wayland"
    } else {
        "xcb"
    };
    platforms
        .split(',')
        .any(|platform| platform.trim() == current)
}

/// Two-letter code of the session language, matching the UI language.
fn language_code() -> &'static str {
    match i18n::lang() {
        i18n::Lang::En => "en",
        i18n::Lang::De => "de",
    }
}

/// `key[lang]` if present, else the untranslated `key`.
fn localized(map: &Value, key: &str, lang: &str) -> Option<String> {
    localized_only(map, key, lang).or_else(|| text(map, key))
}

fn localized_only(map: &Value, key: &str, lang: &str) -> Option<String> {
    if lang == "en" {
        return None;
    }
    text(map, &format!("{key}[{lang}]"))
}

fn text(map: &Value, key: &str) -> Option<String> {
    match map_get_str(map, key)? {
        Value::Text(text) => Some(text.clone()),
        _ => None,
    }
}

fn map_get_str<'a>(map: &'a Value, key: &str) -> Option<&'a Value> {
    map_get(map, &Value::Text(key.to_string()))
}

fn map_get<'a>(map: &'a Value, key: &Value) -> Option<&'a Value> {
    map.as_map()?
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, value)| value)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(entries: Vec<(&str, Value)>) -> Value {
        Value::Map(
            entries
                .into_iter()
                .map(|(k, v)| (Value::Text(k.into()), v))
                .collect(),
        )
    }

    fn sample() -> Value {
        metadata(vec![
            (
                "KPlugin",
                metadata(vec![
                    ("Name", Value::Text("Display Configuration".into())),
                    ("Name[de]", Value::Text("Anzeige-Einrichtung".into())),
                    (
                        "Description",
                        Value::Text("Manage and configure monitors".into()),
                    ),
                    (
                        "Icon",
                        Value::Text("preferences-desktop-display-randr".into()),
                    ),
                ]),
            ),
            (
                "X-KDE-Keywords",
                Value::Text("display,Monitor, resolution".into()),
            ),
            (
                "X-KDE-Keywords[de]",
                Value::Text("Bildschirm,Auflösung,monitor".into()),
            ),
        ])
    }

    #[test]
    fn reads_metadata_from_a_qt_plugin_note() {
        let mut binary = b"\x7fELF garbage".to_vec();
        binary.extend_from_slice(QT_NOTE_NAME);
        binary.extend_from_slice(&[0, 6, 11, 4]);
        let root = Value::Map(vec![
            (
                Value::Integer(2.into()),
                Value::Text("org.kde.KPluginFactory".into()),
            ),
            (Value::Integer(4.into()), sample()),
        ]);
        ciborium::into_writer(&root, &mut binary).unwrap();

        let page = page_from_metadata(
            "kcm_kscreen".into(),
            &embedded_metadata(&binary).unwrap(),
            "en",
        )
        .unwrap();

        assert_eq!(page.name, "Display Configuration");
        assert_eq!(
            page.icon.as_deref(),
            Some("preferences-desktop-display-randr")
        );
        assert_eq!(page.keywords, vec!["display", "monitor", "resolution"]);
    }

    #[test]
    fn session_language_adds_translated_name_and_keywords() {
        let page = page_from_metadata("kcm_kscreen".into(), &sample(), "de").unwrap();

        assert_eq!(page.name, "Anzeige-Einrichtung");
        assert_eq!(
            page.keywords,
            vec![
                "auflösung",
                "bildschirm",
                "display",
                "monitor",
                "resolution"
            ]
        );
    }

    #[test]
    fn plugin_without_metadata_note_is_skipped() {
        assert!(embedded_metadata(b"\x7fELF no qt metadata here").is_none());
    }
}
