//! Reboot-safe clipboard history. Activated with the `c` keyword.
//!
//! History is persisted through the host storage capability (SQLite-backed), so
//! it survives restarts and reboots. The current clipboard is captured via
//! `wl-paste` whenever the plugin is queried; entries can be copied back or
//! deleted from a List view.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{Action, ActionKind, Item, ListView, ResultItem, View, ViewEvent};
use std::process::Command;

const HISTORY_KEY: &str = "history";
const MAX_ENTRIES: usize = 50;

struct Clipboard;

impl Plugin for Clipboard {
    fn query(&mut self, host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        capture(host);
        let count = load(host).len();
        vec![ResultItem {
            id: "open".to_string(),
            title: "Clipboard history".to_string(),
            subtitle: Some(format!("{count} entries")),
            icon: Some("edit-paste".to_string()),
            score: 6_000,
            command_id: "open".to_string(),
            actions: Vec::new(),
        }]
    }

    fn activate(
        &mut self,
        host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        (command_id == "open").then(|| Response::Render(list_view(host, "")))
    }

    fn event(&mut self, host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Input { text } => Some(Response::Replace(list_view(host, &text))),
            ViewEvent::Action {
                action_id,
                item_id: Some(index),
            } if action_id == "delete" => {
                delete_entry(host, &index);
                Some(Response::Replace(list_view(host, "")))
            }
            _ => None,
        }
    }
}

/// Capture the current clipboard into history if it is new.
fn capture(host: &mut dyn HostApi) {
    let Some(current) = wl_paste() else {
        return;
    };
    if current.trim().is_empty() {
        return;
    }
    let mut history = load(host);
    if history.first() == Some(&current) {
        return;
    }
    history.retain(|entry| entry != &current);
    history.insert(0, current);
    history.truncate(MAX_ENTRIES);
    save(host, &history);
}

fn list_view(host: &mut dyn HostApi, filter: &str) -> View {
    let needle = filter.to_lowercase();
    let history = load(host);
    let items = history
        .iter()
        .enumerate()
        .filter(|(_, entry)| entry.to_lowercase().contains(&needle))
        .map(|(index, entry)| Item {
            id: index.to_string(),
            title: preview(entry),
            subtitle: None,
            icon: Some("edit-paste".to_string()),
            accessories: Vec::new(),
            actions: vec![
                Action {
                    id: "copy".to_string(),
                    title: "Copy".to_string(),
                    icon: None,
                    shortcut: None,
                    kind: ActionKind::Copy {
                        text: entry.clone(),
                    },
                },
                Action {
                    id: "delete".to_string(),
                    title: "Delete".to_string(),
                    icon: None,
                    shortcut: None,
                    kind: ActionKind::Plugin,
                },
            ],
        })
        .collect();
    View::List(ListView {
        title: Some("Clipboard history".to_string()),
        placeholder: Some("Filter clipboard history".to_string()),
        items,
        empty_text: Some("No clipboard history yet".to_string()),
        actions: Vec::new(),
    })
}

fn delete_entry(host: &mut dyn HostApi, index: &str) {
    let Ok(index) = index.parse::<usize>() else {
        return;
    };
    let mut history = load(host);
    if index < history.len() {
        history.remove(index);
        save(host, &history);
    }
}

/// Single-line preview of a (possibly multi-line) entry.
fn preview(entry: &str) -> String {
    let line = entry.lines().next().unwrap_or("").trim();
    if line.chars().count() > 80 {
        format!("{}…", line.chars().take(80).collect::<String>())
    } else {
        line.to_string()
    }
}

fn load(host: &mut dyn HostApi) -> Vec<String> {
    host.storage_get(HISTORY_KEY)
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

fn save(host: &mut dyn HostApi, history: &[String]) {
    if let Ok(raw) = serde_json::to_string(history) {
        host.storage_set(HISTORY_KEY, &raw);
    }
}

fn wl_paste() -> Option<String> {
    let output = Command::new("wl-paste").arg("--no-newline").output().ok()?;
    if output.status.success() {
        String::from_utf8(output.stdout).ok()
    } else {
        None
    }
}

fn main() {
    run(Clipboard);
}
