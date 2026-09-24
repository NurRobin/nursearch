//! Built-in system actions (lock, suspend, reboot, …) surfaced as search
//! results so the launcher can do more than open applications.

use crate::i18n;

/// A system action the launcher can run as an external command. Display strings
/// are localized via [`i18n`]; keywords stay multilingual so a query in either
/// language matches regardless of the active locale.
pub struct SystemCommand {
    /// Stable id used as the history key and to look up localized labels.
    pub id: &'static str,
    pub icon: &'static str,
    pub keywords: &'static [&'static str],
    /// Command and arguments to spawn.
    pub command: &'static [&'static str],
}

/// Asks Plasma's logout prompt (the same dialog as the Kickoff power buttons,
/// with its countdown and cancel button) to run `method`.
macro_rules! plasma_prompt {
    ($method:literal) => {
        &[
            "gdbus",
            "call",
            "--session",
            "--dest",
            "org.kde.LogoutPrompt",
            "--object-path",
            "/LogoutPrompt",
            "--method",
            concat!("org.kde.LogoutPrompt.", $method),
        ]
    };
}

/// Score penalty for a title match, so an app whose name starts with the same
/// letters wins the tie; an exactly typed action name still ranks on top.
const TITLE_PENALTY: i64 = 1_000;
/// Score penalty for a keyword-only match, mirroring the app metadata penalty
/// in [`crate::rank::app_match_score`].
const KEYWORD_PENALTY: i64 = 2_500;

/// The available system actions. Session-ending actions (log out, restart,
/// shut down) never act directly: they open Plasma's confirmation prompt, since
/// a launcher is driven by muscle memory and Enter often lands before reading.
/// Lock and suspend are harmless to undo and run immediately.
pub const COMMANDS: &[SystemCommand] = &[
    SystemCommand {
        id: "system:lock",
        icon: "system-lock-screen",
        keywords: &["lock", "sperren", "bildschirm", "screen"],
        command: &["loginctl", "lock-session"],
    },
    SystemCommand {
        id: "system:logout",
        icon: "system-log-out",
        keywords: &["logout", "abmelden", "logoff", "exit"],
        command: plasma_prompt!("promptLogout"),
    },
    SystemCommand {
        id: "system:suspend",
        icon: "system-suspend",
        keywords: &["suspend", "standby", "sleep", "schlaf", "energie"],
        command: &["systemctl", "suspend"],
    },
    SystemCommand {
        id: "system:reboot",
        icon: "system-reboot",
        keywords: &["reboot", "restart", "neustart", "neu starten"],
        command: plasma_prompt!("promptReboot"),
    },
    SystemCommand {
        id: "system:shutdown",
        icon: "system-shutdown",
        keywords: &["shutdown", "poweroff", "herunterfahren", "ausschalten"],
        command: plasma_prompt!("promptShutDown"),
    },
];

impl SystemCommand {
    pub fn title(&self) -> &'static str {
        i18n::system_title(self.id)
    }

    pub fn subtitle(&self) -> &'static str {
        i18n::system_subtitle(self.id)
    }

    /// Score the (normalized) query against this action. Only word *starts*
    /// count: substring and fuzzy matching is what let "ter" (from "terminal")
    /// hit "herun*ter*fahren" and outrank the terminal app.
    pub fn match_score(&self, query: &str) -> Option<i64> {
        if query.is_empty() {
            return None;
        }
        let title = self.title().to_lowercase();
        let title_score = (title.starts_with(query)
            || title.split_whitespace().any(|word| word.starts_with(query)))
        .then(|| crate::rank::match_score(&title, query))
        .flatten()
        .map(|score| score - TITLE_PENALTY);
        let keyword_score = self
            .keywords
            .iter()
            .filter(|keyword| keyword.starts_with(query))
            .filter_map(|keyword| crate::rank::match_score(keyword, query))
            .max()
            .map(|score| score - KEYWORD_PENALTY);
        title_score.max(keyword_score)
    }
}
