//! Systemd user-service plugin. Keyword: `sc`
//!
//! Lists `--user` systemd services and provides start / stop / restart / status
//! actions. Only `--user` services are shown; system services are left to
//! `pkexec` / terminal workflows to avoid privilege prompts in a launcher.
//!
//! Never stops safety-critical user units: any unit whose name starts with
//! `app-`, `plasma-`, `pipewire`, `wireplumber`, `dbus`, `xdg-`, or
//! `nursearch` is presented as read-only (status only, no start/stop/restart).

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{
    Action, ActionKind, DetailView, Item, ListView, MetaPair, ResultItem, View, ViewEvent,
};
use std::process::Command;

/// Unit name prefixes that must not be stopped or restarted.
const PROTECTED_PREFIXES: &[&str] = &[
    "app-",
    "plasma-",
    "pipewire",
    "wireplumber",
    "dbus",
    "xdg-",
    "nursearch",
];

#[derive(Clone)]
struct ServiceInfo {
    name: String,
    active: String,
    sub: String,
    description: String,
}

struct Systemd {
    services: Vec<ServiceInfo>,
    /// Name of the service whose detail view is currently open.
    detail: Option<String>,
}

impl Systemd {
    fn new() -> Self {
        Self {
            services: Vec::new(),
            detail: None,
        }
    }
}

impl Plugin for Systemd {
    fn query(&mut self, _host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: "Systemd User Services".to_string(),
            subtitle: Some("List and control --user services".to_string()),
            icon: Some("application-x-executable".to_string()),
            score: 4_800,
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
            self.services = list_services();
            self.detail = None;
            return Some(Response::Render(service_list_view(&self.services, "")));
        }
        None
    }

    fn event(&mut self, _host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Input { text } => {
                if self.detail.is_some() {
                    return None; // input ignored in detail view
                }
                Some(Response::Replace(service_list_view(&self.services, &text)))
            }
            ViewEvent::Action {
                action_id,
                item_id: Some(service_name),
            } => match action_id.as_str() {
                "status" => {
                    let detail = service_detail(&service_name);
                    self.detail = Some(service_name);
                    Some(Response::Render(detail))
                }
                "start" | "stop" | "restart" => {
                    if !is_protected(&service_name) {
                        let _ = Command::new("systemctl")
                            .args(["--user", &action_id, &service_name])
                            .status();
                        // Refresh list after action
                        self.services = list_services();
                    }
                    self.detail = None;
                    Some(Response::Replace(service_list_view(&self.services, "")))
                }
                _ => None,
            },
            ViewEvent::Pop => {
                self.detail = None;
                Some(Response::Replace(service_list_view(&self.services, "")))
            }
            _ => None,
        }
    }
}

fn service_list_view(services: &[ServiceInfo], filter: &str) -> View {
    let needle = filter.to_lowercase();
    let items: Vec<Item> = services
        .iter()
        .filter(|s| {
            needle.is_empty()
                || s.name.to_lowercase().contains(&needle)
                || s.description.to_lowercase().contains(&needle)
        })
        .map(|s| {
            let protected = is_protected(&s.name);
            let running = s.active == "active";
            let mut actions = vec![Action {
                id: "status".to_string(),
                title: "Status".to_string(),
                icon: Some("dialog-information".to_string()),
                shortcut: None,
                kind: ActionKind::Plugin,
            }];
            if !protected {
                if running {
                    actions.push(Action {
                        id: "stop".to_string(),
                        title: "Stop".to_string(),
                        icon: Some("process-stop".to_string()),
                        shortcut: None,
                        kind: ActionKind::Plugin,
                    });
                    actions.push(Action {
                        id: "restart".to_string(),
                        title: "Restart".to_string(),
                        icon: Some("view-refresh".to_string()),
                        shortcut: None,
                        kind: ActionKind::Plugin,
                    });
                } else {
                    actions.push(Action {
                        id: "start".to_string(),
                        title: "Start".to_string(),
                        icon: Some("media-playback-start".to_string()),
                        shortcut: None,
                        kind: ActionKind::Plugin,
                    });
                }
            }
            Item {
                id: s.name.clone(),
                title: s.name.clone(),
                subtitle: Some(s.description.clone()),
                icon: Some(
                    if running {
                        "media-playback-start"
                    } else {
                        "media-playback-stop"
                    }
                    .to_string(),
                ),
                accessories: vec![format!("{} ({})", s.active, s.sub)],
                actions,
            }
        })
        .collect();

    View::List(ListView {
        title: Some("Systemd User Services".to_string()),
        placeholder: Some("Filter services…".to_string()),
        items,
        empty_text: Some("No services found".to_string()),
        actions: Vec::new(),
    })
}

fn service_detail(name: &str) -> View {
    let status_output = Command::new("systemctl")
        .args(["--user", "--no-pager", "status", name])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .unwrap_or_else(|| "(could not get status)".to_string());

    View::Detail(DetailView {
        title: Some(name.to_string()),
        markdown: Some(format!("```\n{status_output}\n```")),
        metadata: vec![MetaPair {
            label: "Unit".to_string(),
            value: name.to_string(),
        }],
        ..Default::default()
    })
}

fn is_protected(name: &str) -> bool {
    PROTECTED_PREFIXES
        .iter()
        .any(|prefix| name.starts_with(prefix))
}

fn list_services() -> Vec<ServiceInfo> {
    let output = Command::new("systemctl")
        .args([
            "--user",
            "list-units",
            "--type=service",
            "--all",
            "--no-legend",
            "--no-pager",
            "--plain",
        ])
        .output()
        .ok();
    let Some(output) = output else {
        return Vec::new();
    };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(parse_service_line)
        .collect()
}

/// Parse a `systemctl list-units --plain` line.
/// Format: `<unit>  <load>  <active>  <sub>  <description...>`
fn parse_service_line(line: &str) -> Option<ServiceInfo> {
    let mut cols = line.split_whitespace();
    let name = cols.next()?.trim_start_matches('●').to_string();
    if name.is_empty() {
        return None;
    }
    let _load = cols.next()?; // loaded/not-found/masked — not displayed
    let active = cols.next()?.to_string();
    let sub = cols.next()?.to_string();
    let description = cols.collect::<Vec<_>>().join(" ");
    Some(ServiceInfo {
        name,
        active,
        sub,
        description,
    })
}

fn main() {
    run(Systemd::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_running_service_line() {
        let line =
            "app-org.kde.dolphin@abc.service  loaded  active  running  Dolphin - File Manager";
        let info = parse_service_line(line).unwrap();
        assert_eq!(info.name, "app-org.kde.dolphin@abc.service");
        assert_eq!(info.active, "active");
        assert_eq!(info.sub, "running");
        assert_eq!(info.description, "Dolphin - File Manager");
    }

    #[test]
    fn parses_inactive_service_line() {
        let line = "arch-update.service  loaded  inactive  dead  Run arch-update auto check";
        let info = parse_service_line(line).unwrap();
        assert_eq!(info.name, "arch-update.service");
        assert_eq!(info.active, "inactive");
    }

    #[test]
    fn parses_bullet_prefixed_line() {
        let line = "●arch-update-tray.service  not-found  inactive  dead  arch-update-tray.service";
        let info = parse_service_line(line).unwrap();
        assert_eq!(info.name, "arch-update-tray.service");
    }

    #[test]
    fn protected_units_are_identified() {
        assert!(is_protected("app-org.kde.dolphin@abc.service"));
        assert!(is_protected("pipewire.service"));
        assert!(is_protected("plasma-kwin_wayland.service"));
        assert!(!is_protected("arch-update.service"));
    }
}
