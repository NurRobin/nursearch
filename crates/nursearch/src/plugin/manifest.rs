//! Plugin manifest (`nursearch-plugin.toml`) parsing, validation, and discovery.

use nursearch_proto::PROTOCOL_VERSION;
use serde::Deserialize;
use std::path::{Path, PathBuf};

pub const MANIFEST_FILE: &str = "nursearch-plugin.toml";

/// A validated plugin manifest plus the directory it was loaded from.
#[derive(Clone, Debug)]
pub struct Plugin {
    pub manifest: Manifest,
    pub dir: PathBuf,
}

impl Plugin {
    /// The command to launch. Relative paths resolve against the plugin
    /// directory because the process is spawned with its working directory set
    /// there (see [`crate::plugin::process`]).
    pub fn launch_argv(&self) -> Vec<String> {
        self.manifest.entry.clone()
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub icon: Option<String>,
    pub protocol_version: u32,
    /// Command + args to launch the plugin process.
    pub entry: Vec<String>,
    #[serde(default)]
    pub activation: Activation,
    #[serde(default)]
    pub preferences: Vec<Preference>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Activation {
    #[serde(default)]
    pub mode: ActivationMode,
    #[serde(default)]
    pub keyword: Option<String>,
    /// Also contribute when the query matches nothing else.
    #[serde(default)]
    pub fallback: bool,
}

#[derive(Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ActivationMode {
    #[default]
    Global,
    Keyword,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Preference {
    pub id: String,
    #[serde(rename = "type")]
    pub pref_type: String,
    pub label: String,
    #[serde(default)]
    pub default: Option<toml::Value>,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Debug)]
pub enum ManifestError {
    Read(std::io::Error),
    Parse(toml::de::Error),
    Invalid(String),
}

impl std::fmt::Display for ManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ManifestError::Read(err) => write!(f, "could not read manifest: {err}"),
            ManifestError::Parse(err) => write!(f, "invalid manifest TOML: {err}"),
            ManifestError::Invalid(msg) => write!(f, "{msg}"),
        }
    }
}

/// Parse and validate a manifest from TOML text.
pub fn parse_manifest(text: &str) -> Result<Manifest, ManifestError> {
    let manifest: Manifest = toml::from_str(text).map_err(ManifestError::Parse)?;
    validate(&manifest)?;
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.id.trim().is_empty() {
        return Err(ManifestError::Invalid("manifest id is empty".into()));
    }
    if manifest.entry.is_empty() {
        return Err(ManifestError::Invalid(format!(
            "plugin '{}' has an empty entry command",
            manifest.id
        )));
    }
    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(ManifestError::Invalid(format!(
            "plugin '{}' targets protocol {} but host speaks {}",
            manifest.id, manifest.protocol_version, PROTOCOL_VERSION
        )));
    }
    if manifest.activation.mode == ActivationMode::Keyword
        && manifest
            .activation
            .keyword
            .as_deref()
            .map(str::trim)
            .filter(|keyword| !keyword.is_empty())
            .is_none()
    {
        return Err(ManifestError::Invalid(format!(
            "plugin '{}' uses keyword activation but defines no keyword",
            manifest.id
        )));
    }
    Ok(())
}

/// Directories scanned for plugins, in priority order.
pub fn plugin_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(extra) = std::env::var("NURSEARCH_PLUGIN_DIR") {
        for dir in extra.split(':').filter(|dir| !dir.is_empty()) {
            dirs.push(PathBuf::from(dir));
        }
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    dirs.push(PathBuf::from(home).join(".local/share/nursearch/plugins"));
    dirs.push(PathBuf::from("/usr/share/nursearch/plugins"));
    dirs
}

/// Discover and validate all plugins, de-duplicating by id (first wins).
pub fn discover() -> Vec<Plugin> {
    let mut seen = std::collections::HashSet::new();
    let mut plugins = Vec::new();

    for root in plugin_dirs() {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            let manifest_path = dir.join(MANIFEST_FILE);
            if !manifest_path.is_file() {
                continue;
            }
            match load(&manifest_path) {
                Ok(manifest) => {
                    if seen.insert(manifest.id.clone()) {
                        plugins.push(Plugin { manifest, dir });
                    }
                }
                Err(err) => log::warn!("skipping plugin at {}: {err}", dir.display()),
            }
        }
    }
    plugins
}

fn load(path: &Path) -> Result<Manifest, ManifestError> {
    let text = std::fs::read_to_string(path).map_err(ManifestError::Read)?;
    parse_manifest(&text)
}

#[cfg(test)]
mod tests {
    use super::*;

    const VALID: &str = r#"
        id = "com.example.demo"
        name = "Demo"
        protocol_version = 1
        entry = ["python3", "main.py"]

        [activation]
        mode = "keyword"
        keyword = "d"
    "#;

    #[test]
    fn parses_valid_manifest() {
        let manifest = parse_manifest(VALID).unwrap();
        assert_eq!(manifest.id, "com.example.demo");
        assert_eq!(manifest.activation.mode, ActivationMode::Keyword);
        assert_eq!(manifest.activation.keyword.as_deref(), Some("d"));
    }

    #[test]
    fn rejects_wrong_protocol_version() {
        let text = VALID.replace("protocol_version = 1", "protocol_version = 99");
        assert!(matches!(
            parse_manifest(&text),
            Err(ManifestError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_keyword_activation_without_keyword() {
        let text = r#"
            id = "x"
            name = "X"
            protocol_version = 1
            entry = ["x"]
            [activation]
            mode = "keyword"
        "#;
        assert!(matches!(
            parse_manifest(text),
            Err(ManifestError::Invalid(_))
        ));
    }

    #[test]
    fn defaults_to_global_activation() {
        let text = r#"
            id = "x"
            name = "X"
            protocol_version = 1
            entry = ["x"]
        "#;
        let manifest = parse_manifest(text).unwrap();
        assert_eq!(manifest.activation.mode, ActivationMode::Global);
    }

    #[test]
    fn launch_argv_is_the_entry_command() {
        let plugin = Plugin {
            manifest: parse_manifest(VALID).unwrap(),
            dir: PathBuf::from("/plugins/demo"),
        };
        // argv is passed through unchanged; cwd resolution handles relative paths.
        assert_eq!(plugin.launch_argv(), vec!["python3", "main.py"]);
    }
}
