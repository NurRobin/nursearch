use crate::db::UsageStats;
use crate::desktop::DesktopEntry;

/// Score how well an app matches the (already normalized) query, combining a
/// strong name match with a weaker fallback over generic name / comment / keywords.
pub fn app_match_score(app: &DesktopEntry, query: &str) -> Option<i64> {
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

/// Bonus applied to a base match score based on how often and how recently the
/// result was launched, both for this exact query and globally.
pub fn usage_score(stats: &UsageStats) -> i64 {
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
}
