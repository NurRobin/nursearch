//! `nursearch-plugins`: a minimal plugin manager.
//!
//! Subcommands:
//!   list                 List installed plugins
//!   install <path|giturl> Install from a local directory or a git repository
//!   remove <id>          Remove an installed plugin
//!   dir                  Print the plugin directory

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

const MANIFEST_FILE: &str = "nursearch-plugin.toml";

#[derive(Deserialize)]
struct Manifest {
    id: String,
    name: String,
    #[serde(default)]
    version: String,
    #[serde(default)]
    description: String,
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("list") => cmd_list(),
        Some("install") => match args.get(1) {
            Some(source) => cmd_install(source),
            None => Err("usage: nursearch-plugins install <path|git-url>".into()),
        },
        Some("remove") => match args.get(1) {
            Some(id) => cmd_remove(id),
            None => Err("usage: nursearch-plugins remove <id>".into()),
        },
        Some("dir") => {
            println!("{}", plugin_dir().display());
            Ok(())
        }
        _ => {
            usage();
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn usage() {
    eprintln!(
        "nursearch-plugins — manage NurSearch plugins\n\n\
         USAGE:\n  \
         nursearch-plugins list\n  \
         nursearch-plugins install <path|git-url>\n  \
         nursearch-plugins remove <id>\n  \
         nursearch-plugins dir"
    );
}

fn cmd_list() -> Result<(), String> {
    let dir = plugin_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        println!("No plugins installed ({} does not exist).", dir.display());
        return Ok(());
    };
    let mut found = false;
    for entry in entries.flatten() {
        let manifest_path = entry.path().join(MANIFEST_FILE);
        if let Ok(manifest) = read_manifest(&manifest_path) {
            found = true;
            let version = if manifest.version.is_empty() {
                String::new()
            } else {
                format!(" v{}", manifest.version)
            };
            println!("{}{}  [{}]", manifest.name, version, manifest.id);
            if !manifest.description.is_empty() {
                println!("    {}", manifest.description);
            }
        }
    }
    if !found {
        println!("No plugins installed in {}.", dir.display());
    }
    Ok(())
}

fn cmd_install(source: &str) -> Result<(), String> {
    // Keep the cloned temp dir alive until the copy below completes.
    let mut _staging: Option<tempfile::TempDir> = None;
    let source_dir: PathBuf = if is_git_url(source) {
        let temp = clone_to_temp(source)?;
        let path = temp.path().to_path_buf();
        _staging = Some(temp);
        path
    } else {
        let path = PathBuf::from(source);
        if !path.is_dir() {
            return Err(format!("'{source}' is not a directory or a git URL"));
        }
        path
    };

    let manifest = read_manifest(&source_dir.join(MANIFEST_FILE))
        .map_err(|err| format!("no valid {MANIFEST_FILE} in source: {err}"))?;

    // The id becomes a directory name, so it must be a single safe component
    // (untrusted source: a cloned repo or arbitrary directory).
    let dest = safe_dest(&manifest.id)?;
    if dest.exists() {
        return Err(format!(
            "plugin '{}' is already installed; remove it first",
            manifest.id
        ));
    }
    // Stage the copy in a hidden directory next to the plugin dir (same
    // filesystem, not scanned by discovery), then atomically rename into place.
    // A failed copy leaves nothing discoverable under the plugin directory.
    let root = plugin_dir();
    std::fs::create_dir_all(&root).map_err(|e| e.to_string())?;
    let staging_root = root.parent().map(Path::to_path_buf).unwrap_or_else(|| root.clone());
    std::fs::create_dir_all(&staging_root).map_err(|e| e.to_string())?;
    let staging = staging_root.join(format!(".nursearch-install-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);

    if let Err(err) = copy_dir(&source_dir, &staging) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(err.to_string());
    }
    // Drop a cloned .git directory; it is not needed at runtime.
    let _ = std::fs::remove_dir_all(staging.join(".git"));
    if let Err(err) = std::fs::rename(&staging, &dest) {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("could not finalize install: {err}"));
    }

    println!("Installed '{}' ({}) to {}", manifest.name, manifest.id, dest.display());
    Ok(())
}

fn cmd_remove(id: &str) -> Result<(), String> {
    let dest = safe_dest(id)?;
    if !dest.exists() {
        return Err(format!("plugin '{id}' is not installed"));
    }
    std::fs::remove_dir_all(&dest).map_err(|e| e.to_string())?;
    println!("Removed '{id}'");
    Ok(())
}

/// Resolve an installed-plugin directory from an id, rejecting anything that is
/// not a single safe path component so it cannot escape the plugin directory.
fn safe_dest(id: &str) -> Result<PathBuf, String> {
    if !is_valid_id(id) {
        return Err(format!("refusing unsafe plugin id '{id}'"));
    }
    let root = plugin_dir();
    let dest = root.join(id);
    // Defense in depth: the joined path must stay within the plugin directory.
    if !dest.starts_with(&root) {
        return Err(format!("refusing plugin id '{id}' that escapes the plugin directory"));
    }
    Ok(dest)
}

/// A plugin id must be a single, normal path component from a safe charset.
fn is_valid_id(id: &str) -> bool {
    if id.is_empty() || id.starts_with('.') {
        return false;
    }
    if !id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
    {
        return false;
    }
    let mut components = Path::new(id).components();
    matches!(components.next(), Some(std::path::Component::Normal(_))) && components.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::{copy_dir, is_valid_id};

    #[test]
    fn accepts_reverse_dns_ids() {
        assert!(is_valid_id("dev.nursearch.demo"));
        assert!(is_valid_id("com.example.my-plugin_2"));
    }

    #[test]
    fn rejects_traversal_and_unsafe_ids() {
        for bad in ["", ".", "..", "../evil", "a/b", "/abs", ".hidden", "a\\b", "a\0b", "x/../y"] {
            assert!(!is_valid_id(bad), "should reject {bad:?}");
        }
    }

    #[test]
    fn copy_dir_rejects_symlinks() {
        use std::os::unix::fs::symlink;
        let base = std::env::temp_dir().join(format!("nspm-symlink-{}", std::process::id()));
        let src = base.join("src");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("ok.txt"), "hi").unwrap();
        symlink("/etc/hostname", src.join("evil")).unwrap();

        let result = copy_dir(&src, &base.join("dst"));
        assert!(result.is_err(), "copy_dir should refuse a symlinked source");

        let _ = std::fs::remove_dir_all(&base);
    }
}

fn is_git_url(source: &str) -> bool {
    source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("git@")
        || source.ends_with(".git")
}

fn clone_to_temp(url: &str) -> Result<tempfile::TempDir, String> {
    // Atomically created, randomly named, process-private directory.
    let dir = tempfile::Builder::new()
        .prefix("nursearch-pm-")
        .tempdir()
        .map_err(|e| format!("could not create temp dir: {e}"))?;
    let status = Command::new("git")
        .args(["clone", "--depth", "1", url])
        .arg(dir.path())
        .status()
        .map_err(|e| format!("could not run git: {e}"))?;
    if !status.success() {
        return Err("git clone failed".into());
    }
    Ok(dir)
}

fn read_manifest(path: &Path) -> Result<Manifest, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    toml::from_str(&text).map_err(|e| e.to_string())
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        // `file_type()` does not follow symlinks, so this catches a hostile
        // source that links to a path outside the plugin. Reject rather than
        // follow it into a sensitive or unbounded location.
        let file_type = entry.file_type()?;
        if file_type.is_symlink() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("refusing to install symlink: {}", entry.path().display()),
            ));
        }
        let target = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

fn plugin_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NURSEARCH_PLUGIN_DIR")
        && let Some(first) = dir.split(':').find(|d| !d.is_empty())
    {
        return PathBuf::from(first);
    }
    let base = std::env::var("XDG_DATA_HOME").unwrap_or_else(|_| {
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
        format!("{home}/.local/share")
    });
    PathBuf::from(base).join("nursearch/plugins")
}
