use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DesktopEntry {
    pub name: String,
    pub exec: String,
    pub icon: Option<String>,
    pub path: PathBuf,
}

pub fn discover_apps() -> Vec<DesktopEntry> {
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut apps = Vec::new();

    for dir in application_dirs() {
        if !seen_dirs.insert(dir.clone()) {
            continue;
        }

        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) != Some("desktop") {
                continue;
            }
            if !seen_files.insert(path.clone()) {
                continue;
            }
            if let Some(app) = parse_desktop_file(&path) {
                apps.push(app);
            }
        }
    }

    apps.sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    apps
}

fn application_dirs() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/usr/share/applications"),
        PathBuf::from("/usr/local/share/applications"),
    ];

    if let Ok(home) = env::var("HOME") {
        dirs.push(PathBuf::from(home).join(".local/share/applications"));
    }

    if let Ok(xdg_dirs) = env::var("XDG_DATA_DIRS") {
        for dir in xdg_dirs.split(':').filter(|dir| !dir.is_empty()) {
            dirs.push(PathBuf::from(dir).join("applications"));
        }
    }

    dirs
}

fn parse_desktop_file(path: &Path) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;
    let mut in_desktop_entry = false;
    let mut name = None;
    let mut exec = None;
    let mut icon = None;
    let mut entry_type = None;
    let mut no_display = false;
    let mut hidden = false;

    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if line.starts_with('[') && line.ends_with(']') {
            in_desktop_entry = line == "[Desktop Entry]";
            continue;
        }

        if !in_desktop_entry {
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            continue;
        };

        match key {
            "Name" => name = Some(value.trim().to_string()),
            "Exec" => exec = Some(clean_exec(value)),
            "Icon" => icon = Some(value.trim().to_string()),
            "Type" => entry_type = Some(value.trim().to_string()),
            "NoDisplay" => no_display = parse_bool(value),
            "Hidden" => hidden = parse_bool(value),
            _ => {}
        }
    }

    if entry_type.as_deref() != Some("Application") || no_display || hidden {
        return None;
    }

    let name = name?;
    let exec = exec?;
    if name.is_empty() || exec.is_empty() {
        return None;
    }

    Some(DesktopEntry {
        name,
        exec,
        icon,
        path: path.to_path_buf(),
    })
}

fn parse_bool(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}

fn clean_exec(exec: &str) -> String {
    let mut parts = Vec::new();
    let mut chars = exec.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '%' {
            match chars.peek().copied() {
                Some('%') => {
                    parts.push('%');
                    chars.next();
                }
                Some('u' | 'U' | 'f' | 'F' | 'i' | 'c' | 'k') => {
                    chars.next();
                }
                Some(_) => {}
                None => parts.push(ch),
            }
        } else {
            parts.push(ch);
        }
    }

    parts
        .into_iter()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}
