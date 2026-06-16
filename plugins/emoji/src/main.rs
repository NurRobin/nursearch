//! Emoji / symbol picker. Activated with the `e` keyword: `e heart` lists
//! matching emoji; activating one copies it to the clipboard.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{ResultItem, ViewEvent};

const EMOJI: &[(&str, &str)] = &[
    ("😀", "grinning face smile happy"),
    ("😂", "joy laughing tears"),
    ("😍", "love heart eyes"),
    ("😎", "cool sunglasses"),
    ("😢", "crying sad tears"),
    ("😡", "angry mad rage"),
    ("👍", "thumbs up yes approve"),
    ("👎", "thumbs down no"),
    ("🙏", "pray thanks please"),
    ("👏", "clap applause"),
    ("🔥", "fire lit hot"),
    ("✨", "sparkles shiny new"),
    ("🎉", "party tada celebrate"),
    ("❤️", "red heart love"),
    ("💔", "broken heart"),
    ("⭐", "star favorite"),
    ("✅", "check done ok success"),
    ("❌", "cross no error fail"),
    ("⚠️", "warning caution"),
    ("💡", "idea light bulb"),
    ("🚀", "rocket launch ship fast"),
    ("🐛", "bug insect"),
    ("💻", "laptop computer code"),
    ("📋", "clipboard copy"),
    ("🔍", "search magnifying glass"),
    ("⏰", "alarm clock time"),
    ("📅", "calendar date"),
    ("📌", "pin location"),
    ("🎯", "target goal bullseye"),
    ("🤔", "thinking hmm"),
];

struct Emoji;

impl Plugin for Emoji {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        let needle = text.trim().to_lowercase();
        EMOJI
            .iter()
            .filter(|(emoji, keywords)| {
                needle.is_empty()
                    || keywords.contains(needle.as_str())
                    || keywords.contains(needle.as_str())
                    || keywords
                        .split_whitespace()
                        .any(|word| word.starts_with(&needle))
                    || emoji.contains(&needle)
            })
            .take(12)
            .map(|(emoji, keywords)| ResultItem {
                id: emoji.to_string(),
                title: format!("{emoji}  {}", first_word(keywords)),
                subtitle: Some(keywords.to_string()),
                icon: None,
                score: 5_000,
                command_id: emoji.to_string(),
                actions: Vec::new(),
            })
            .collect()
    }

    fn activate(
        &mut self,
        host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        host.clipboard_set(command_id);
        Some(Response::Close { hide: true })
    }

    fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
        None
    }
}

fn first_word(keywords: &str) -> &str {
    keywords.split_whitespace().next().unwrap_or(keywords)
}

fn main() {
    run(Emoji);
}
