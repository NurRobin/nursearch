use std::collections::{HashMap, HashSet};
use std::env;
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct DesktopEntry {
    pub name: String,
    pub generic_name: Option<String>,
    pub comment: Option<String>,
    pub keywords: Vec<String>,
    pub exec: Option<String>,
    pub icon: Option<String>,
    pub path: PathBuf,
    pub dbus_activatable: bool,
    pub terminal: bool,
}

impl DesktopEntry {
    pub fn search_text(&self) -> String {
        let mut parts = vec![self.name.as_str()];

        if let Some(generic_name) = self.generic_name.as_deref() {
            parts.push(generic_name);
        }
        if let Some(comment) = self.comment.as_deref() {
            parts.push(comment);
        }
        for keyword in &self.keywords {
            parts.push(keyword);
        }

        parts.join(" ")
    }

    pub fn exec_args(&self) -> io::Result<Vec<String>> {
        let Some(exec) = self.exec.as_deref() else {
            return Ok(Vec::new());
        };

        parse_exec_args(
            exec,
            &ExecContext {
                name: &self.name,
                icon: self.icon.as_deref(),
                desktop_file: &self.path,
            },
        )
    }
}

struct ExecContext<'a> {
    name: &'a str,
    icon: Option<&'a str>,
    desktop_file: &'a Path,
}

pub fn discover_apps() -> Vec<DesktopEntry> {
    let mut seen_dirs = HashSet::new();
    let mut seen_files = HashSet::new();
    let mut apps = Vec::new();
    let desktop_envs = current_desktops();

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
            if let Some(app) = parse_desktop_file(&path, &desktop_envs) {
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

fn current_desktops() -> Vec<String> {
    env::var("XDG_CURRENT_DESKTOP")
        .unwrap_or_default()
        .split(':')
        .filter(|desktop| !desktop.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_desktop_file(path: &Path, desktop_envs: &[String]) -> Option<DesktopEntry> {
    let content = fs::read_to_string(path).ok()?;
    parse_desktop_content(&content, path, desktop_envs)
}

fn parse_desktop_content(
    content: &str,
    path: &Path,
    desktop_envs: &[String],
) -> Option<DesktopEntry> {
    let mut fields = DesktopFields::default();
    let mut in_desktop_entry = false;

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

        let Some((raw_key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim().to_string();
        let (key, locale) = split_locale_key(raw_key.trim());

        match key {
            "Name" => {
                fields.name.insert(locale, value);
            }
            "GenericName" => {
                fields.generic_name.insert(locale, value);
            }
            "Comment" => {
                fields.comment.insert(locale, value);
            }
            "Keywords" => {
                fields.keywords.insert(locale, value);
            }
            "Exec" => fields.exec = Some(value),
            "Icon" => fields.icon = Some(value),
            "Type" => fields.entry_type = Some(value),
            "NoDisplay" => fields.no_display = parse_bool(&value),
            "Hidden" => fields.hidden = parse_bool(&value),
            "OnlyShowIn" => fields.only_show_in = parse_list(&value),
            "NotShowIn" => fields.not_show_in = parse_list(&value),
            "TryExec" => fields.try_exec = Some(value),
            "DBusActivatable" => fields.dbus_activatable = parse_bool(&value),
            "Terminal" => fields.terminal = parse_bool(&value),
            _ => {}
        }
    }

    if fields.entry_type.as_deref() != Some("Application")
        || fields.no_display
        || fields.hidden
        || !shows_in_desktop(&fields, desktop_envs)
        || !try_exec_available(fields.try_exec.as_deref())
    {
        return None;
    }

    let name = localized_value(&fields.name)?;
    if name.is_empty() {
        return None;
    }

    let exec = fields.exec.filter(|exec| !exec.is_empty());
    if exec.is_none() && !fields.dbus_activatable {
        return None;
    }

    Some(DesktopEntry {
        name,
        generic_name: localized_value(&fields.generic_name),
        comment: localized_value(&fields.comment),
        keywords: localized_value(&fields.keywords)
            .map(|keywords| parse_list(&keywords))
            .unwrap_or_default(),
        exec,
        icon: fields.icon.filter(|icon| !icon.is_empty()),
        path: path.to_path_buf(),
        dbus_activatable: fields.dbus_activatable,
        terminal: fields.terminal,
    })
}

#[derive(Default)]
struct DesktopFields {
    name: HashMap<Option<String>, String>,
    generic_name: HashMap<Option<String>, String>,
    comment: HashMap<Option<String>, String>,
    keywords: HashMap<Option<String>, String>,
    exec: Option<String>,
    icon: Option<String>,
    entry_type: Option<String>,
    no_display: bool,
    hidden: bool,
    only_show_in: Vec<String>,
    not_show_in: Vec<String>,
    try_exec: Option<String>,
    dbus_activatable: bool,
    terminal: bool,
}

fn split_locale_key(key: &str) -> (&str, Option<String>) {
    let Some(start) = key.find('[') else {
        return (key, None);
    };
    if !key.ends_with(']') {
        return (key, None);
    }

    (
        &key[..start],
        Some(key[start + 1..key.len() - 1].to_string()),
    )
}

fn localized_value(values: &HashMap<Option<String>, String>) -> Option<String> {
    let locale = env::var("LANG").unwrap_or_default();
    let language = locale
        .split(['.', '@'])
        .next()
        .filter(|part| !part.is_empty())
        .unwrap_or("");
    let language_prefix = language.split('_').next().unwrap_or("");

    if !language.is_empty() {
        if let Some(value) = values.get(&Some(language.to_string())) {
            return Some(value.clone());
        }
    }
    if !language_prefix.is_empty() {
        if let Some(value) = values.get(&Some(language_prefix.to_string())) {
            return Some(value.clone());
        }
    }

    values.get(&None).cloned()
}

fn shows_in_desktop(fields: &DesktopFields, desktop_envs: &[String]) -> bool {
    if desktop_envs
        .iter()
        .any(|desktop| fields.not_show_in.iter().any(|item| item == desktop))
    {
        return false;
    }

    if fields.only_show_in.is_empty() {
        return true;
    }

    desktop_envs
        .iter()
        .any(|desktop| fields.only_show_in.iter().any(|item| item == desktop))
}

fn try_exec_available(try_exec: Option<&str>) -> bool {
    let Some(try_exec) = try_exec.filter(|value| !value.is_empty()) else {
        return true;
    };

    let path = Path::new(try_exec);
    if path.is_absolute() {
        return is_executable(path);
    }

    env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).any(|dir| is_executable(&dir.join(path))))
        .unwrap_or(false)
}

fn is_executable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

fn parse_bool(value: &str) -> bool {
    value.trim().eq_ignore_ascii_case("true")
}

fn parse_list(value: &str) -> Vec<String> {
    value
        .split(';')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

fn parse_exec_args(exec: &str, context: &ExecContext<'_>) -> io::Result<Vec<String>> {
    let args = shell_words::split(exec)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;
    let mut expanded = Vec::new();

    for arg in args {
        match arg.as_str() {
            "%f" | "%F" | "%u" | "%U" => {}
            "%i" => {
                if let Some(icon) = context.icon.filter(|icon| !icon.is_empty()) {
                    expanded.push("--icon".to_string());
                    expanded.push(icon.to_string());
                }
            }
            _ => {
                if contains_standalone_only_code(&arg) {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidInput,
                        format!("field code in '{arg}' must stand alone"),
                    ));
                }

                let value = expand_exec_arg(&arg, context)?;
                if !value.is_empty() {
                    expanded.push(value);
                }
            }
        }
    }

    Ok(expanded)
}

fn contains_standalone_only_code(arg: &str) -> bool {
    arg != "%i" && ["%F", "%U"].iter().any(|code| arg.contains(code))
}

fn expand_exec_arg(arg: &str, context: &ExecContext<'_>) -> io::Result<String> {
    let mut result = String::new();
    let mut chars = arg.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '%' {
            result.push(ch);
            continue;
        }

        let Some(code) = chars.next() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "exec command ends with an incomplete field code",
            ));
        };

        match code {
            '%' => result.push('%'),
            'f' | 'F' | 'u' | 'U' | 'd' | 'D' | 'n' | 'N' | 'v' | 'm' => {}
            'c' => result.push_str(context.name),
            'k' => result.push_str(&context.desktop_file.to_string_lossy()),
            'i' => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "%i must stand alone in an exec command",
                ));
            }
            other if other.is_ascii_alphabetic() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("invalid exec field code %{other}"),
                ));
            }
            other => {
                result.push('%');
                result.push(other);
            }
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn parse(content: &str) -> Option<DesktopEntry> {
        parse_desktop_content(
            content,
            Path::new("/tmp/test.desktop"),
            &["KDE".to_string()],
        )
    }

    #[test]
    fn parses_search_metadata() {
        let app = parse(
            "
            [Desktop Entry]
            Type=Application
            Name=Browser
            GenericName=Web Browser
            Comment=View sites
            Keywords=web;internet;
            Exec=browser %U
            Icon=browser
            ",
        )
        .unwrap();

        assert_eq!(app.name, "Browser");
        assert_eq!(app.generic_name.as_deref(), Some("Web Browser"));
        assert_eq!(app.comment.as_deref(), Some("View sites"));
        assert_eq!(app.keywords, ["web", "internet"]);
        assert_eq!(app.icon.as_deref(), Some("browser"));
    }

    #[test]
    fn respects_only_show_in_and_not_show_in() {
        assert!(
            parse(
                "
                [Desktop Entry]
                Type=Application
                Name=Visible
                OnlyShowIn=KDE;GNOME;
                Exec=visible
                ",
            )
            .is_some()
        );

        assert!(
            parse(
                "
                [Desktop Entry]
                Type=Application
                Name=Hidden
                NotShowIn=KDE;
                Exec=hidden
                ",
            )
            .is_none()
        );
    }

    #[test]
    fn skips_missing_try_exec() {
        assert!(
            parse(
                "
                [Desktop Entry]
                Type=Application
                Name=Missing
                TryExec=/definitely/not/installed
                Exec=missing
                ",
            )
            .is_none()
        );
    }

    #[test]
    fn accepts_executable_try_exec() {
        let dir = env::temp_dir().join(format!("nursearch-test-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let bin = dir.join("tool");
        let mut file = fs::File::create(&bin).unwrap();
        writeln!(file, "#!/bin/sh").unwrap();
        let mut perms = fs::metadata(&bin).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&bin, perms).unwrap();

        let app = parse_desktop_content(
            &format!(
                "
                [Desktop Entry]
                Type=Application
                Name=Present
                TryExec={}
                Exec=present
                ",
                bin.display()
            ),
            Path::new("/tmp/test.desktop"),
            &["KDE".to_string()],
        );

        assert!(app.is_some());
        fs::remove_file(&bin).unwrap();
        fs::remove_dir(&dir).unwrap();
    }

    #[test]
    fn allows_dbus_activatable_without_exec() {
        let app = parse(
            "
            [Desktop Entry]
            Type=Application
            Name=Portal App
            DBusActivatable=true
            ",
        )
        .unwrap();

        assert!(app.dbus_activatable);
        assert!(app.exec.is_none());
    }

    #[test]
    fn expands_exec_field_codes_for_launch_without_files() {
        let app = parse(
            "
            [Desktop Entry]
            Type=Application
            Name=Example App
            Exec=example --name %c --desktop %k %i %U
            Icon=example-icon
            ",
        )
        .unwrap();

        assert_eq!(
            app.exec_args().unwrap(),
            [
                "example",
                "--name",
                "Example App",
                "--desktop",
                "/tmp/test.desktop",
                "--icon",
                "example-icon"
            ]
        );
    }

    #[test]
    fn rejects_invalid_exec_field_codes() {
        let app = parse(
            "
            [Desktop Entry]
            Type=Application
            Name=Bad
            Exec=bad %Z
            ",
        )
        .unwrap();

        assert!(app.exec_args().is_err());
    }
}
