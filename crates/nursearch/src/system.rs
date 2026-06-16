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

/// The available system actions. `logout` targets KDE Plasma (the primary
/// platform); the rest go through `systemctl`/`loginctl` and work broadly.
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
        command: &["qdbus", "org.kde.Shutdown", "/Shutdown", "logout"],
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
        command: &["systemctl", "reboot"],
    },
    SystemCommand {
        id: "system:shutdown",
        icon: "system-shutdown",
        keywords: &["shutdown", "poweroff", "herunterfahren", "ausschalten"],
        command: &["systemctl", "poweroff"],
    },
];

impl SystemCommand {
    pub fn title(&self) -> &'static str {
        i18n::system_title(self.id)
    }

    pub fn subtitle(&self) -> &'static str {
        i18n::system_subtitle(self.id)
    }

    /// Text searched against the query: localized title plus every keyword.
    pub fn search_text(&self) -> String {
        let mut text = String::from(self.title());
        for keyword in self.keywords {
            text.push(' ');
            text.push_str(keyword);
        }
        text
    }
}
