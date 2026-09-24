//! Unified search across every result source. App ranking, the calculator, and
//! system commands all produce a common [`SearchResult`] so the UI can render
//! and act on them uniformly.

use crate::calc;
use crate::config::Quicklink;
use crate::db::{StatsSnapshot, normalize_query};
use crate::desktop::DesktopEntry;
use crate::direct::{self, Target};
use crate::kcm::SettingsPage;
use crate::rank::{app_match_score, usage_score};
use crate::system::{COMMANDS, SystemCommand};
use nursearch_proto::ResultItem;

/// What activating a result does.
#[derive(Clone, Debug)]
pub enum Action {
    /// Launch a discovered desktop application.
    Launch(DesktopEntry),
    /// Run a short-lived fixed command (system actions).
    Run(Vec<String>),
    /// Start a long-running program in its own systemd unit, like an app.
    Detached {
        command: Vec<String>,
        app_id: String,
    },
    /// Open a URL or file path with its default application.
    Open(String),
    /// Copy text to the clipboard (calculator results).
    Copy(String),
    /// Enter a plugin view session for this item.
    OpenPlugin {
        plugin_id: String,
        command_id: String,
        item_id: String,
    },
}

/// A short category label shown as a badge in the result row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    App,
    Calculator,
    System,
    /// A KDE System Settings page.
    Settings,
    /// A URL, path or quicklink opened directly.
    Link,
    /// A `> command` shell line.
    Command,
    /// A contribution from a plugin; carries the plugin's display name for the badge.
    Plugin(String),
}

impl Kind {
    pub fn badge(&self) -> Option<String> {
        match self {
            Kind::App => None,
            Kind::Calculator => Some("=".to_string()),
            Kind::System => Some(crate::i18n::badge_system().to_string()),
            Kind::Settings => Some(crate::i18n::badge_settings().to_string()),
            Kind::Link => Some(crate::i18n::badge_open().to_string()),
            Kind::Command => Some(">".to_string()),
            Kind::Plugin(name) => Some(name.clone()),
        }
    }
}

/// Convert a plugin's protocol result item into a renderable root result,
/// applying the same launch-history boost the core results get so a
/// frequently-used plugin item climbs the ranking.
pub fn result_from_plugin(
    plugin_id: &str,
    plugin_name: &str,
    item: ResultItem,
    snapshot: &StatsSnapshot,
) -> SearchResult {
    let history_key = format!("plugin:{plugin_id}:{}", item.id);
    let score = item.score + usage_score(&snapshot.stats_for(&history_key));
    SearchResult {
        title: item.title,
        subtitle: item.subtitle,
        icon: item.icon,
        kind: Kind::Plugin(plugin_name.to_string()),
        score,
        history_key: Some(history_key),
        action: Action::OpenPlugin {
            plugin_id: plugin_id.to_string(),
            command_id: item.command_id,
            item_id: item.id,
        },
    }
}

/// One renderable, actionable search result.
#[derive(Clone, Debug)]
pub struct SearchResult {
    pub title: String,
    pub subtitle: Option<String>,
    pub icon: Option<String>,
    pub kind: Kind,
    pub score: i64,
    /// History key for usage learning; `None` means the result is not recorded.
    pub history_key: Option<String>,
    pub action: Action,
}

/// Build the ranked result list for a query across all providers.
#[cfg(test)]
pub fn search(apps: &[DesktopEntry], query: &str, snapshot: &StatsSnapshot) -> Vec<SearchResult> {
    let sources = Sources {
        apps,
        ..Sources::default()
    };
    finalize(core_results(&sources, query, snapshot), 12)
}

/// Everything the in-process core searches.
#[derive(Default)]
pub struct Sources<'a> {
    pub apps: &'a [DesktopEntry],
    pub settings: &'a [SettingsPage],
    pub quicklinks: &'a [Quicklink],
    /// Home directory for `~` paths.
    pub home: Option<&'a std::path::Path>,
}

/// Score of a directly named target (URL, path, quicklink, command): above
/// any app, since the query can only mean that target.
const DIRECT_SCORE: i64 = 12_000;

/// Results from the mandatory in-process core (apps, calculator, system,
/// settings pages, direct targets), unranked and untruncated so plugin
/// contributions can be merged in before [`finalize`].
pub fn core_results(sources: &Sources, query: &str, snapshot: &StatsSnapshot) -> Vec<SearchResult> {
    let normalized = normalize_query(query);
    let mut results = Vec::new();

    match direct::resolve(query, sources.quicklinks, sources.home) {
        // A command line is an explicit mode: nothing else is meant.
        Some(Target::Command(command)) => return vec![command_result(command)],
        Some(target) => results.push(direct_result(target)),
        None => {}
    }

    let settings = sources.settings;
    let apps = sources.apps;
    if !normalized.is_empty() {
        if let Some(result) = calculator_result(&normalized) {
            results.push(result);
        }
        results.extend(system_results(&normalized, snapshot));
        results.extend(settings_results(settings, &normalized, snapshot));
    }
    results.extend(app_results(apps, &normalized, snapshot));
    results
}

/// Rank a merged set of results (core + plugin) and cap it to the display limit.
pub fn finalize(mut results: Vec<SearchResult>, max_results: usize) -> Vec<SearchResult> {
    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    results.truncate(max_results);
    results
}

fn app_results(
    apps: &[DesktopEntry],
    normalized: &str,
    snapshot: &StatsSnapshot,
) -> Vec<SearchResult> {
    apps.iter()
        .filter_map(|app| {
            let base = app_match_score(app, normalized)?;
            let key = app.path.to_string_lossy().to_string();
            let score = base + usage_score(&snapshot.stats_for(&key));
            Some(SearchResult {
                title: app.name.clone(),
                subtitle: app.generic_name.clone().or_else(|| app.comment.clone()),
                icon: app.icon.clone(),
                kind: Kind::App,
                score,
                history_key: Some(key),
                action: Action::Launch(app.clone()),
            })
        })
        .collect()
}

/// Score penalty for settings pages, so an app with the same match wins: the
/// "Bluetooth" app should outrank the Bluetooth settings page.
const SETTINGS_PENALTY: i64 = 500;
/// Additional penalty when only a keyword matched, as for app metadata.
const SETTINGS_KEYWORD_PENALTY: i64 = 2_500;

fn settings_results(
    pages: &[SettingsPage],
    normalized: &str,
    snapshot: &StatsSnapshot,
) -> Vec<SearchResult> {
    pages
        .iter()
        .filter_map(|page| {
            let name_score = crate::rank::match_score(&page.name, normalized);
            // Score keywords one by one: joined, the long keyword lists would
            // drown every match in the length penalty.
            let keyword_score = page
                .keywords
                .iter()
                .filter_map(|keyword| crate::rank::word_start_score(keyword, normalized))
                .max()
                .map(|score| score - SETTINGS_KEYWORD_PENALTY);
            let base = name_score.max(keyword_score)? - SETTINGS_PENALTY;
            let key = format!("kcm:{}", page.id);
            let score = base + usage_score(&snapshot.stats_for(&key));
            Some(SearchResult {
                title: page.name.clone(),
                subtitle: page.description.clone(),
                icon: page.icon.clone(),
                kind: Kind::Settings,
                score,
                history_key: Some(key),
                action: Action::Detached {
                    command: vec!["systemsettings".to_string(), page.id.clone()],
                    app_id: "systemsettings".to_string(),
                },
            })
        })
        .collect()
}

fn command_result(command: String) -> SearchResult {
    SearchResult {
        title: command.clone(),
        subtitle: Some(crate::i18n::run_command_hint().to_string()),
        icon: Some("utilities-terminal".to_string()),
        kind: Kind::Command,
        score: DIRECT_SCORE,
        history_key: None,
        action: Action::Detached {
            command: vec!["sh".to_string(), "-c".to_string(), command],
            app_id: "shell".to_string(),
        },
    }
}

fn direct_result(target: Target) -> SearchResult {
    let (title, subtitle, icon, open) = match target {
        Target::Url(url) => (
            url.clone(),
            crate::i18n::open_url_hint().to_string(),
            "internet-web-browser".to_string(),
            url,
        ),
        Target::Path(path) => {
            let icon = if path.is_dir() {
                "folder"
            } else {
                "text-x-generic"
            };
            let shown = path.to_string_lossy().into_owned();
            (
                shown.clone(),
                crate::i18n::open_path_hint().to_string(),
                icon.to_string(),
                shown,
            )
        }
        Target::Quicklink {
            name,
            url,
            term,
            icon,
        } => {
            let title = match term {
                Some(term) => format!("{name}: {term}"),
                None => name,
            };
            (
                title,
                url.clone(),
                icon.unwrap_or_else(|| "internet-web-browser".to_string()),
                url,
            )
        }
        Target::Command(_) => unreachable!("handled by command_result"),
    };
    SearchResult {
        title,
        subtitle: Some(subtitle),
        icon: Some(icon),
        kind: Kind::Link,
        score: DIRECT_SCORE,
        history_key: None,
        action: Action::Open(open),
    }
}

fn calculator_result(normalized: &str) -> Option<SearchResult> {
    let value = calc::evaluate(normalized)?;
    Some(SearchResult {
        title: value.clone(),
        subtitle: Some(crate::i18n::calc_hint().to_string()),
        icon: Some("accessories-calculator".to_string()),
        kind: Kind::Calculator,
        // Outrank a typical app match so a valid expression sits at the top.
        score: 11_000,
        history_key: None,
        action: Action::Copy(value),
    })
}

fn system_results(normalized: &str, snapshot: &StatsSnapshot) -> Vec<SearchResult> {
    COMMANDS
        .iter()
        .filter_map(|command| system_result(command, normalized, snapshot))
        .collect()
}

fn system_result(
    command: &SystemCommand,
    normalized: &str,
    snapshot: &StatsSnapshot,
) -> Option<SearchResult> {
    let base = command.match_score(normalized)?;
    let score = base + usage_score(&snapshot.stats_for(command.id));
    Some(SearchResult {
        title: command.title().to_string(),
        subtitle: Some(command.subtitle().to_string()),
        icon: Some(command.icon.to_string()),
        kind: Kind::System,
        score,
        history_key: Some(command.id.to_string()),
        action: Action::Run(command.command.iter().map(|s| s.to_string()).collect()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::HistoryDb;
    use std::path::PathBuf;

    fn test_app(
        name: &str,
        generic_name: Option<&str>,
        comment: Option<&str>,
        keywords: &[&str],
        path: &str,
    ) -> DesktopEntry {
        DesktopEntry {
            name: name.to_string(),
            generic_name: generic_name.map(ToOwned::to_owned),
            comment: comment.map(ToOwned::to_owned),
            keywords: keywords.iter().map(|keyword| keyword.to_string()).collect(),
            exec: Some(name.to_lowercase()),
            icon: None,
            path: PathBuf::from(path),
            dbus_activatable: false,
            terminal: false,
        }
    }

    #[test]
    fn ranks_apps_by_desktop_metadata() {
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![test_app(
            "Firefox",
            Some("Web Browser"),
            Some("Browse the web"),
            &["internet"],
            "/tmp/firefox.desktop",
        )];

        let results = search(&apps, "internet", &db.snapshot("internet"));

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Firefox");
    }

    #[test]
    fn exact_app_name_beats_shorter_prefix_match_with_metadata() {
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![
            test_app(
                "Term",
                Some("Terminal Emulator"),
                Some("A long comment that used to reduce exact-name score"),
                &["shell", "console"],
                "/tmp/term.desktop",
            ),
            test_app("Terminal", None, None, &[], "/tmp/terminal.desktop"),
        ];

        let results = search(&apps, "term", &db.snapshot("term"));

        assert_eq!(results[0].title, "Term");
    }

    #[test]
    fn calculator_result_sits_on_top() {
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![test_app("Calc App", None, None, &[], "/tmp/calc.desktop")];

        let results = search(&apps, "2+2", &db.snapshot("2+2"));

        assert_eq!(results[0].kind, Kind::Calculator);
        assert_eq!(results[0].title, "4");
    }

    #[test]
    fn system_command_matches_keyword() {
        let db = HistoryDb::open_in_memory().unwrap();
        let results = search(&[], "sperren", &db.snapshot("sperren"));

        assert!(results.iter().any(|result| result.kind == Kind::System
            && matches!(&result.action, Action::Run(cmd) if cmd.first().map(String::as_str) == Some("loginctl"))));
    }

    #[test]
    fn partial_word_inside_system_keyword_does_not_match() {
        // "ter" sits inside "herunterfahren"; typing the start of "terminal"
        // must never surface (let alone rank first) a power action.
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![test_app(
            "Ghostty",
            None,
            Some("A terminal emulator"),
            &["terminal", "tty", "pty"],
            "/tmp/ghostty.desktop",
        )];

        let results = search(&apps, "ter", &db.snapshot("ter"));

        assert!(results.iter().all(|result| result.kind != Kind::System));
        assert_eq!(results[0].title, "Ghostty");
    }

    #[test]
    fn keyword_word_start_beats_mid_word_name_match() {
        // "ter" is the start of the keyword "terminal" but only the tail of
        // "Center"; the terminal is what the user means.
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![
            test_app("Info Center", None, None, &[], "/tmp/info.desktop"),
            test_app(
                "Ghostty",
                None,
                Some("A terminal emulator"),
                &["terminal", "tty", "pty"],
                "/tmp/ghostty.desktop",
            ),
        ];

        let results = search(&apps, "ter", &db.snapshot("ter"));

        assert_eq!(results[0].title, "Ghostty");
    }

    #[test]
    fn scattered_letters_in_metadata_do_not_match() {
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![test_app(
            "LibreOffice Math",
            Some("Formula Editor"),
            Some("Create and edit scientific formulas and equations"),
            &["equation", "office", "math", "formula"],
            "/tmp/math.desktop",
        )];

        assert!(search(&apps, "bluetooth", &db.snapshot("bluetooth")).is_empty());
    }

    #[test]
    fn app_name_prefix_outranks_system_keyword_prefix() {
        let db = HistoryDb::open_in_memory().unwrap();
        let apps = vec![test_app(
            "Restic Browser",
            None,
            None,
            &[],
            "/tmp/restic.desktop",
        )];

        let results = search(&apps, "res", &db.snapshot("res"));

        assert_eq!(results[0].title, "Restic Browser");
        assert!(results.iter().any(|result| result.kind == Kind::System));
    }

    #[test]
    fn power_actions_go_through_the_plasma_confirmation_prompt() {
        for id in ["system:shutdown", "system:reboot", "system:logout"] {
            let command = COMMANDS.iter().find(|command| command.id == id).unwrap();
            assert!(
                !command.command.contains(&"systemctl"),
                "{id} must ask for confirmation instead of acting directly"
            );
            assert!(
                command
                    .command
                    .iter()
                    .any(|arg| arg.contains("LogoutPrompt"))
            );
        }
    }

    fn display_page() -> SettingsPage {
        SettingsPage {
            id: "kcm_kscreen".to_string(),
            name: "Display Configuration".to_string(),
            description: None,
            icon: None,
            keywords: vec![
                "monitor".to_string(),
                "hdr".to_string(),
                "resolution".to_string(),
            ],
        }
    }

    #[test]
    fn settings_page_is_found_by_keyword_and_opens_system_settings() {
        let db = HistoryDb::open_in_memory().unwrap();
        let results = finalize(
            core_results(
                &Sources {
                    settings: &[display_page()],
                    ..Sources::default()
                },
                "hdr",
                &db.snapshot("hdr"),
            ),
            12,
        );

        assert_eq!(results[0].kind, Kind::Settings);
        assert!(
            matches!(&results[0].action, Action::Detached { command, .. } if command == &["systemsettings", "kcm_kscreen"])
        );
    }

    #[test]
    fn settings_keyword_needs_a_word_start() {
        // "sol" sits inside "resolution" and must not surface the page.
        let db = HistoryDb::open_in_memory().unwrap();
        let results = finalize(
            core_results(
                &Sources {
                    settings: &[display_page()],
                    ..Sources::default()
                },
                "sol",
                &db.snapshot("sol"),
            ),
            12,
        );

        assert!(results.is_empty());
    }

    #[test]
    fn plugin_result_gets_usage_boost() {
        let db = HistoryDb::open_in_memory().unwrap();
        let item = ResultItem {
            id: "abc".to_string(),
            title: "Thing".to_string(),
            subtitle: None,
            icon: None,
            score: 100,
            command_id: "open".to_string(),
            actions: Vec::new(),
        };

        // Cold: no history, so the score is just the plugin's own hint.
        let cold = result_from_plugin("p", "Plugin", item.clone(), &db.snapshot("q"));
        assert_eq!(cold.score, 100);

        // After recording a launch under the item's synthetic history key for
        // query "q", the same item ranks higher.
        db.record_launch("q", "plugin:p:abc").unwrap();
        let warm = result_from_plugin("p", "Plugin", item, &db.snapshot("q"));
        assert!(
            warm.score > 100,
            "expected a usage boost, got {}",
            warm.score
        );
    }

    #[test]
    fn usage_history_boosts_query_specific_match() {
        let apps = vec![
            test_app("Termite", None, None, &[], "/tmp/termite.desktop"),
            test_app("Terminal", None, None, &[], "/tmp/terminal.desktop"),
        ];

        // Without history "Termite" (shorter name) edges out "Terminal".
        let cold = HistoryDb::open_in_memory().unwrap();
        let cold_results = search(&apps, "term", &cold.snapshot("term"));
        assert_eq!(cold_results[0].title, "Termite");

        // After launching "Terminal" for this query, it should rank first.
        let warm = HistoryDb::open_in_memory().unwrap();
        warm.record_launch("term", "/tmp/terminal.desktop").unwrap();
        let warm_results = search(&apps, "term", &warm.snapshot("term"));
        assert_eq!(warm_results[0].title, "Terminal");
    }
}
