//! Unified search across every result source. App ranking, the calculator, and
//! system commands all produce a common [`SearchResult`] so the UI can render
//! and act on them uniformly.

use crate::calc;
use crate::db::{StatsSnapshot, normalize_query};
use crate::desktop::DesktopEntry;
use crate::rank::{app_match_score, usage_score};
use crate::system::{COMMANDS, SystemCommand};
use nursearch_proto::ResultItem;

const MAX_RESULTS: usize = 12;

/// What activating a result does.
#[derive(Clone, Debug)]
pub enum Action {
    /// Launch a discovered desktop application.
    Launch(DesktopEntry),
    /// Run a fixed command (system actions).
    Run(Vec<String>),
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
    /// A contribution from a plugin; carries the plugin's display name for the badge.
    Plugin(String),
}

impl Kind {
    pub fn badge(&self) -> Option<String> {
        match self {
            Kind::App => None,
            Kind::Calculator => Some("=".to_string()),
            Kind::System => Some(crate::i18n::badge_system().to_string()),
            Kind::Plugin(name) => Some(name.clone()),
        }
    }
}

/// Convert a plugin's protocol result item into a renderable root result.
pub fn result_from_plugin(plugin_id: &str, plugin_name: &str, item: ResultItem) -> SearchResult {
    SearchResult {
        title: item.title,
        subtitle: item.subtitle,
        icon: item.icon,
        kind: Kind::Plugin(plugin_name.to_string()),
        score: item.score,
        history_key: Some(format!("plugin:{plugin_id}:{}", item.id)),
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
    finalize(core_results(apps, query, snapshot))
}

/// Results from the mandatory in-process core (apps, calculator, system),
/// unranked and untruncated so plugin contributions can be merged in before
/// [`finalize`].
pub fn core_results(
    apps: &[DesktopEntry],
    query: &str,
    snapshot: &StatsSnapshot,
) -> Vec<SearchResult> {
    let normalized = normalize_query(query);
    let mut results = Vec::new();

    if !normalized.is_empty() {
        if let Some(result) = calculator_result(&normalized) {
            results.push(result);
        }
        results.extend(system_results(&normalized, snapshot));
    }
    results.extend(app_results(apps, &normalized, snapshot));
    results
}

/// Rank a merged set of results (core + plugin) and cap it to the display limit.
pub fn finalize(mut results: Vec<SearchResult>) -> Vec<SearchResult> {
    results.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });
    results.truncate(MAX_RESULTS);
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
    let base = crate::rank::match_score(&command.search_text(), normalized)?;
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
