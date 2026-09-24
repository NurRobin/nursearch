//! Queries that name their target directly instead of searching for it: a URL
//! or file path to open, a `> command` to run, or a user quicklink
//! (`aw wayland`). They work on the raw query, because paths and URLs are
//! case-sensitive.

use crate::config::Quicklink;
use std::path::{Path, PathBuf};

/// What a direct query resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// Open a URL in the default browser.
    Url(String),
    /// Open an existing file or folder with its default application.
    Path(PathBuf),
    /// Run a shell command line.
    Command(String),
    /// A quicklink; `url` is ready to open. `term` is `None` for a bookmark
    /// or a search quicklink typed without a term (then `url` is its site).
    Quicklink {
        name: String,
        url: String,
        term: Option<String>,
        icon: Option<String>,
    },
}

/// Top-level domains accepted for scheme-less URLs ("github.com/x"). A fixed
/// list keeps names like "firefox.desktop" or "v1.2" from turning into URLs.
const KNOWN_TLDS: &[&str] = &[
    "com", "org", "net", "de", "io", "dev", "app", "eu", "info", "me", "co", "uk", "at", "ch",
    "nl", "fr", "it", "es", "gg", "tv", "sh", "rs", "ai", "xyz", "gov", "edu", "wiki", "social",
];

/// Resolve `query` to a direct target, if it is one.
pub fn resolve(query: &str, quicklinks: &[Quicklink], home: Option<&Path>) -> Option<Target> {
    let query = query.trim();
    if query.is_empty() {
        return None;
    }
    if let Some(command) = query.strip_prefix('>') {
        let command = command.trim();
        return (!command.is_empty()).then(|| Target::Command(command.to_string()));
    }
    quicklink(query, quicklinks)
        .or_else(|| url(query).map(Target::Url))
        .or_else(|| path(query, home).map(Target::Path))
}

fn quicklink(query: &str, quicklinks: &[Quicklink]) -> Option<Target> {
    let (keyword, term) = match query.split_once(char::is_whitespace) {
        Some((keyword, term)) => (keyword, term.trim()),
        None => (query, ""),
    };
    let keyword = keyword.to_lowercase();
    let link = quicklinks.iter().find(|link| link.keyword == keyword)?;
    let target = |url: String, term: Option<String>| Target::Quicklink {
        name: link.name.clone(),
        url,
        term,
        icon: link.icon.clone(),
    };

    if !link.url.contains("{query}") {
        // A bookmark only matches its bare keyword.
        return term.is_empty().then(|| target(link.url.clone(), None));
    }
    if term.is_empty() {
        return site_root(&link.url).map(|root| target(root, None));
    }
    Some(target(
        link.url.replace("{query}", &percent_encode(term)),
        Some(term.to_string()),
    ))
}

/// `https://host/` of a URL, for a search quicklink typed without a term.
fn site_root(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{scheme}://{host}/"))
}

fn url(query: &str) -> Option<String> {
    if query.chars().any(char::is_whitespace) {
        return None;
    }
    let lower = query.to_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        return (query.len() > "https://".len()).then(|| query.to_string());
    }
    let host = lower.split(['/', '?', '#']).next()?;
    let host = host.split(':').next()?;
    let labels: Vec<&str> = host.split('.').collect();
    let valid_labels = labels.len() >= 2
        && labels.iter().all(|label| {
            !label.is_empty() && label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
        });
    let tld = labels.last()?;
    (valid_labels && KNOWN_TLDS.contains(tld)).then(|| format!("https://{query}"))
}

fn path(query: &str, home: Option<&Path>) -> Option<PathBuf> {
    let path = if query == "~" {
        home?.to_path_buf()
    } else if let Some(rest) = query.strip_prefix("~/") {
        home?.join(rest)
    } else if query.starts_with('/') {
        PathBuf::from(query)
    } else {
        return None;
    };
    path.exists().then_some(path)
}

/// Percent-encode a search term for a URL query (RFC 3986 unreserved kept).
fn percent_encode(text: &str) -> String {
    let mut encoded = String::with_capacity(text.len());
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;

    fn links() -> Vec<Quicklink> {
        vec![
            Quicklink {
                keyword: "aw".into(),
                name: "Arch Wiki".into(),
                url: "https://wiki.archlinux.org/index.php?search={query}".into(),
                icon: None,
            },
            Quicklink {
                keyword: "mail".into(),
                name: "Mail".into(),
                url: "https://mail.example.org".into(),
                icon: None,
            },
        ]
    }

    #[test]
    fn quicklink_fills_in_the_encoded_term() {
        let Some(Target::Quicklink { url, term, .. }) = resolve("aw Wayland & KDE", &links(), None)
        else {
            panic!("expected a quicklink");
        };
        assert_eq!(
            url,
            "https://wiki.archlinux.org/index.php?search=Wayland%20%26%20KDE"
        );
        assert_eq!(term.as_deref(), Some("Wayland & KDE"));
    }

    #[test]
    fn quicklink_without_term_opens_the_site_and_bookmarks_match_bare_keyword() {
        assert!(matches!(
            resolve("AW", &links(), None),
            Some(Target::Quicklink { url, term: None, .. }) if url == "https://wiki.archlinux.org/"
        ));
        assert!(matches!(
            resolve("mail", &links(), None),
            Some(Target::Quicklink { url, .. }) if url == "https://mail.example.org"
        ));
        assert_eq!(resolve("mail something", &links(), None), None);
        assert_eq!(resolve("awesome", &links(), None), None);
    }

    #[test]
    fn recognizes_urls_but_not_app_names() {
        assert_eq!(
            resolve("https://Example.com/A", &[], None),
            Some(Target::Url("https://Example.com/A".into()))
        );
        assert_eq!(
            resolve("github.com/NurRobin", &[], None),
            Some(Target::Url("https://github.com/NurRobin".into()))
        );
        for query in [
            "firefox.desktop",
            "v1.2",
            "3.5",
            "node.js",
            "https://",
            "foo bar.com",
        ] {
            assert_eq!(resolve(query, &[], None), None, "{query}");
        }
    }

    #[test]
    fn opens_existing_paths_only() {
        let home = std::env::temp_dir();
        assert_eq!(
            resolve("~", &[], Some(&home)),
            Some(Target::Path(home.clone()))
        );
        assert_eq!(
            resolve("/", &[], None),
            Some(Target::Path(PathBuf::from("/")))
        );
        assert_eq!(resolve("/definitely/not/here", &[], None), None);
    }

    #[test]
    fn shell_command_needs_text_after_the_prompt() {
        assert_eq!(
            resolve("> code ~/Projects", &[], None),
            Some(Target::Command("code ~/Projects".into()))
        );
        assert_eq!(resolve(">", &[], None), None);
    }
}
