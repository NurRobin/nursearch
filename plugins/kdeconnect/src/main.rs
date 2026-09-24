//! KDE Connect plugin. Keyword: `k`
//!
//! Lists paired and reachable KDE Connect devices via `kdeconnect-cli`. For
//! each device the user can: ring/find it, send a ping, or push the current
//! clipboard to the device.
//!
//! If `kdeconnect-cli` is not installed, a single informative result is shown
//! explaining which package to install.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{Action, ActionKind, Item, ListView, ResultItem, View, ViewEvent};
use std::process::Command;

struct KdeConnect;

impl Plugin for KdeConnect {
    fn query(&mut self, _host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: "KDE Connect".to_string(),
            subtitle: Some(device_summary()),
            icon: Some("kdeconnect".to_string()),
            score: 5_500,
            command_id: "open".to_string(),
            actions: Vec::new(),
        }]
    }

    fn activate(
        &mut self,
        _host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        if command_id == "open" {
            return Some(Response::Render(device_list_view("")));
        }
        None
    }

    fn event(&mut self, host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Input { text } => Some(Response::Replace(device_list_view(&text))),
            ViewEvent::Action {
                action_id,
                item_id: Some(device_id),
            } => {
                handle_action(host, &action_id, &device_id);
                Some(Response::Close { hide: true })
            }
            _ => None,
        }
    }
}

fn device_summary() -> String {
    if !kdeconnect_available() {
        return "kdeconnect-cli not found — install kdeconnect".to_string();
    }
    match list_devices() {
        devices if devices.is_empty() => "No reachable devices".to_string(),
        devices => format!("{} device(s) reachable", devices.len()),
    }
}

fn device_list_view(filter: &str) -> View {
    if !kdeconnect_available() {
        return View::List(ListView {
            title: Some("KDE Connect".to_string()),
            items: vec![Item {
                id: "missing".to_string(),
                title: "kdeconnect-cli not found".to_string(),
                subtitle: Some("Install the 'kdeconnect' package to use this plugin".to_string()),
                icon: Some("dialog-warning".to_string()),
                accessories: Vec::new(),
                actions: Vec::new(),
            }],
            ..Default::default()
        });
    }

    let devices = list_devices();
    let needle = filter.to_lowercase();
    let items = devices
        .into_iter()
        .filter(|(_, name)| needle.is_empty() || name.to_lowercase().contains(&needle))
        .map(|(id, name)| Item {
            id: id.clone(),
            title: name,
            subtitle: Some("Reachable".to_string()),
            icon: Some("kdeconnect".to_string()),
            accessories: Vec::new(),
            actions: vec![
                Action {
                    id: "ring".to_string(),
                    title: "Ring / Find Device".to_string(),
                    icon: Some("audio-volume-high".to_string()),
                    shortcut: None,
                    kind: ActionKind::Plugin,
                },
                Action {
                    id: "ping".to_string(),
                    title: "Send Ping".to_string(),
                    icon: Some("network-connect".to_string()),
                    shortcut: None,
                    kind: ActionKind::Plugin,
                },
                Action {
                    id: "clipboard".to_string(),
                    title: "Send Clipboard".to_string(),
                    icon: Some("edit-paste".to_string()),
                    shortcut: None,
                    kind: ActionKind::Plugin,
                },
            ],
        })
        .collect();

    View::List(ListView {
        title: Some("KDE Connect".to_string()),
        placeholder: Some("Filter devices…".to_string()),
        items,
        empty_text: Some("No reachable devices found".to_string()),
        actions: Vec::new(),
    })
}

fn handle_action(host: &mut dyn HostApi, action_id: &str, device_id: &str) {
    match action_id {
        "ring" => {
            let _ = Command::new("kdeconnect-cli")
                .args(["--device", device_id, "--ring"])
                .status();
        }
        "ping" => {
            let _ = Command::new("kdeconnect-cli")
                .args(["--device", device_id, "--ping"])
                .status();
        }
        "clipboard" => {
            host.run_command(vec![
                "kdeconnect-cli".to_string(),
                "--device".to_string(),
                device_id.to_string(),
                "--send-clipboard".to_string(),
            ]);
        }
        _ => {}
    }
}

/// Returns (id, name) pairs for all paired and reachable devices.
fn list_devices() -> Vec<(String, String)> {
    let output = Command::new("kdeconnect-cli")
        .args(["--list-available", "--id-name-only"])
        .output()
        .ok();
    let Some(output) = output else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            // Format: "<id> <name>" (space-separated, id has no spaces)
            let (id, name) = line.split_once(' ')?;
            Some((id.trim().to_string(), name.trim().to_string()))
        })
        .collect()
}

fn kdeconnect_available() -> bool {
    Command::new("kdeconnect-cli")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn main() {
    run(KdeConnect);
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_device_line() {
        let line = "abc123def456 My Phone";
        let (id, name) = line.split_once(' ').unwrap();
        assert_eq!(id, "abc123def456");
        assert_eq!(name, "My Phone");
    }
}
