//! Lightweight localization. English is the default; German is used when the
//! session locale requests it. All user-facing strings live here so adding a
//! language means extending the tables below.

use std::sync::OnceLock;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    En,
    De,
}

static LANG: OnceLock<Lang> = OnceLock::new();

/// The active language, detected once from the locale environment.
pub fn lang() -> Lang {
    *LANG.get_or_init(detect)
}

fn detect() -> Lang {
    let raw = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default()
        .to_lowercase();
    if raw.starts_with("de") {
        Lang::De
    } else {
        Lang::En
    }
}

/// Define a localized string accessor with one variant per supported language.
macro_rules! tr {
    ($name:ident, en = $en:expr, de = $de:expr) => {
        pub fn $name() -> &'static str {
            match lang() {
                Lang::En => $en,
                Lang::De => $de,
            }
        }
    };
}

tr!(search_placeholder, en = "Search apps", de = "Apps suchen");
tr!(no_results, en = "No matches", de = "Keine Treffer");
tr!(
    calc_hint,
    en = "Press Enter to copy the result",
    de = "Enter kopiert das Ergebnis"
);
tr!(badge_system, en = "System", de = "System");

tr!(hint_open, en = "Open", de = "Öffnen");
tr!(hint_navigate, en = "Navigate", de = "Navigieren");
tr!(hint_close, en = "Close", de = "Schließen");

/// Localized title for a built-in system action, keyed by its stable id.
pub fn system_title(id: &str) -> &'static str {
    match (id, lang()) {
        ("system:lock", Lang::En) => "Lock screen",
        ("system:lock", Lang::De) => "Bildschirm sperren",
        ("system:logout", Lang::En) => "Log out",
        ("system:logout", Lang::De) => "Abmelden",
        ("system:suspend", Lang::En) => "Suspend",
        ("system:suspend", Lang::De) => "Energie sparen",
        ("system:reboot", Lang::En) => "Restart",
        ("system:reboot", Lang::De) => "Neu starten",
        ("system:shutdown", Lang::En) => "Shut down",
        ("system:shutdown", Lang::De) => "Herunterfahren",
        _ => "",
    }
}

/// Localized subtitle for a built-in system action, keyed by its stable id.
pub fn system_subtitle(id: &str) -> &'static str {
    match (id, lang()) {
        ("system:lock", Lang::En) => "Lock the session",
        ("system:lock", Lang::De) => "Sitzung sperren",
        ("system:logout", Lang::En) => "End the session",
        ("system:logout", Lang::De) => "Sitzung beenden",
        ("system:suspend", Lang::En) => "Enter standby",
        ("system:suspend", Lang::De) => "In den Standby wechseln",
        ("system:reboot", Lang::En) => "Restart the system",
        ("system:reboot", Lang::De) => "System neu starten",
        ("system:shutdown", Lang::En) => "Power off the system",
        ("system:shutdown", Lang::De) => "System ausschalten",
        _ => "",
    }
}

/// User-facing error when an action could not be started.
pub fn error_run(title: &str, err: &str) -> String {
    match lang() {
        Lang::En => format!("Could not run {title}: {err}"),
        Lang::De => format!("{title} konnte nicht ausgeführt werden: {err}"),
    }
}

/// User-facing error when the launch history could not be updated.
pub fn error_history(err: &str) -> String {
    match lang() {
        Lang::En => format!("Could not update launch history: {err}"),
        Lang::De => format!("Verlauf konnte nicht aktualisiert werden: {err}"),
    }
}

/// User-facing notice when the history database falls back to memory.
pub fn warn_history_memory(err: &str) -> String {
    match lang() {
        Lang::En => format!("History database is unavailable; using temporary history: {err}"),
        Lang::De => {
            format!("Verlaufsdatenbank nicht verfügbar; temporärer Verlauf wird genutzt: {err}")
        }
    }
}

/// User-facing notice when an action is blocked for lacking a capability.
pub fn error_capability(capability: &str) -> String {
    match lang() {
        Lang::En => format!("Action blocked: plugin lacks the '{capability}' capability"),
        Lang::De => format!("Aktion blockiert: Plugin hat die '{capability}'-Berechtigung nicht"),
    }
}
