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
        let Some(match_score) = match_score(&app.name, &normalized_query) else {
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

fn match_score(name: &str, query: &str) -> Option<i64> {
    if query.is_empty() {
        return Some(1_000);
    }

    let name = name.to_lowercase();
    if name == query {
        return Some(10_000);
    }
    if name.starts_with(query) {
        return Some(8_000 - name.len() as i64);
    }
    if let Some(index) = name.find(query) {
        return Some(6_000 - index as i64 - name.len() as i64);
    }

    fuzzy_score(&name, query).map(|score| 3_000 + score)
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
