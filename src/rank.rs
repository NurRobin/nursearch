use crate::db::{HistoryDb, UsageStats};
use crate::desktop::DesktopEntry;

#[derive(Clone, Debug)]
pub struct RankedApp {
    pub app: DesktopEntry,
    pub score: i64,
}

pub fn rank_apps(apps: &[DesktopEntry], query: &str, db: &HistoryDb) -> Vec<RankedApp> {
    let normalized_query = query.trim().to_lowercase();
    let mut ranked = Vec::new();

    for app in apps {
        let Some(match_score) = app_match_score(app, &normalized_query) else {
            continue;
        };
        let stats = db
            .stats_for(&normalized_query, &app.path.to_string_lossy())
            .unwrap_or_default();
        let score = match_score + usage_score(&stats);
        ranked.push(RankedApp {
            app: app.clone(),
            score,
        });
    }

    ranked.sort_by(|left, right| {
        right.score.cmp(&left.score).then_with(|| {
            left.app
                .name
                .to_lowercase()
                .cmp(&right.app.name.to_lowercase())
        })
    });

    ranked.truncate(12);
    ranked
}

fn app_match_score(app: &DesktopEntry, query: &str) -> Option<i64> {
    let name_score = match_score(&app.name, query);
    let metadata_score = match_score(&app.search_text(), query).map(|score| score - 2_500);

    name_score.max(metadata_score)
}

pub(crate) fn match_score(text: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(1_000);
    }

    let text = text.to_lowercase();
    if text == query {
        return Some(10_000);
    }
    if text.starts_with(query) {
        return Some(8_000 - text.len() as i64);
    }
    if let Some(index) = text.find(query) {
        return Some(6_000 - index as i64 - text.len() as i64);
    }

    fuzzy_score(&text, query).map(|score| 3_000 + score)
}

fn fuzzy_score(name: &str, query: &str) -> Option<i64> {
    let mut score = 0;
    let mut last_index = None;
    let mut search_start = 0;

    for query_char in query.chars() {
        let relative_index = name[search_start..].find(query_char)?;
        let index = search_start + relative_index;

        score += 80;
        if let Some(previous) = last_index {
            if index == previous + 1 {
                score += 40;
            } else {
                score -= (index - previous) as i64;
            }
        } else {
            score -= index as i64;
        }

        last_index = Some(index);
        search_start = index + query_char.len_utf8();
    }

    Some(score - name.len() as i64)
}

fn usage_score(stats: &UsageStats) -> i64 {
    let query_score = stats.query_count.min(50) * 220 + recency_bonus(stats.query_last_used);
    let global_score = stats.global_count.min(50) * 45 + recency_bonus(stats.global_last_used) / 4;
    query_score + global_score
}

fn recency_bonus(last_used: i64) -> i64 {
    if last_used <= 0 {
        return 0;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or_default();
    let age_hours = ((now - last_used).max(0)) / 3_600;

    match age_hours {
        0..=24 => 120,
        25..=168 => 60,
        169..=720 => 25,
        _ => 5,
    }
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
    fn exact_match_scores_highest() {
        assert!(
            match_score("Firefox", "firefox").unwrap() > match_score("Firefox", "fire").unwrap()
        );
    }

    #[test]
    fn substring_beats_sparse_fuzzy_match() {
        assert!(
            match_score("Web Browser", "browser").unwrap()
                > match_score("Word Builder", "wb").unwrap()
        );
    }

    #[test]
    fn returns_none_when_query_characters_are_missing() {
        assert_eq!(match_score("Terminal", "xyz"), None);
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

        let results = rank_apps(&apps, "internet", &db);

        assert_eq!(results.len(), 1);
        assert_eq!(results[0].app.name, "Firefox");
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

        let results = rank_apps(&apps, "term", &db);

        assert_eq!(results[0].app.name, "Term");
    }
}
