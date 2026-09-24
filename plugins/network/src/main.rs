//! Network plugin. Keyword: `n`
//!
//! Provides quick access to network controls:
//! - Toggle Wi-Fi on/off (`nmcli radio wifi on|off`)
//! - Toggle Bluetooth on/off (`rfkill block/unblock bluetooth`)
//! - List known Wi-Fi networks and connect to one (`nmcli device wifi connect`)
//!
//! Requires `nmcli` (NetworkManager) for Wi-Fi control. Bluetooth toggle uses
//! `rfkill` which is always present on Linux. If `nmcli` is not available, a
//! single informative result explains which package is needed.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{Item, ListView, ResultItem, View, ViewEvent};
use std::process::Command;

struct Network;

impl Plugin for Network {
    fn query(&mut self, _host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: "Network".to_string(),
            subtitle: Some(status_summary()),
            icon: Some("network-wireless".to_string()),
            score: 5_000,
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
            return Some(Response::Render(main_view("")));
        }
        None
    }

    fn event(&mut self, host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Input { text } => Some(Response::Replace(main_view(&text))),
            ViewEvent::Action { action_id, item_id } => {
                handle_action(host, &action_id, item_id.as_deref());
                // After a toggle, refresh the view to show the new state.
                Some(Response::Replace(main_view("")))
            }
            _ => None,
        }
    }
}

fn status_summary() -> String {
    let wifi = wifi_status();
    let bt = bluetooth_status();
    format!("Wi-Fi: {wifi}  •  Bluetooth: {bt}")
}

fn main_view(filter: &str) -> View {
    if !nmcli_available() {
        return View::List(ListView {
            title: Some("Network".to_string()),
            items: vec![Item {
                id: "missing".to_string(),
                title: "nmcli not found".to_string(),
                subtitle: Some(
                    "Install the 'networkmanager' package for Wi-Fi control".to_string(),
                ),
                icon: Some("dialog-warning".to_string()),
                accessories: Vec::new(),
                actions: Vec::new(),
            }],
            ..Default::default()
        });
    }

    let needle = filter.to_lowercase();
    let mut items = Vec::new();

    // Wi-Fi toggle
    let wifi_on = wifi_enabled();
    let wifi_label = if wifi_on {
        "Wi-Fi: On — click to turn off"
    } else {
        "Wi-Fi: Off — click to turn on"
    };
    if needle.is_empty() || "wifi wi-fi wireless".contains(&*needle) {
        items.push(Item {
            id: "toggle-wifi".to_string(),
            title: wifi_label.to_string(),
            subtitle: None,
            icon: Some(
                if wifi_on {
                    "network-wireless"
                } else {
                    "network-wireless-disconnected"
                }
                .to_string(),
            ),
            accessories: Vec::new(),
            actions: Vec::new(),
        });
    }

    // Bluetooth toggle
    let bt_on = bluetooth_enabled();
    let bt_label = if bt_on {
        "Bluetooth: On — click to turn off"
    } else {
        "Bluetooth: Off — click to turn on"
    };
    if needle.is_empty() || "bluetooth".contains(&*needle) {
        items.push(Item {
            id: "toggle-bt".to_string(),
            title: bt_label.to_string(),
            subtitle: None,
            icon: Some(
                if bt_on {
                    "bluetooth-active"
                } else {
                    "bluetooth-disabled"
                }
                .to_string(),
            ),
            accessories: Vec::new(),
            actions: Vec::new(),
        });
    }

    // Known Wi-Fi networks (only shown when Wi-Fi is on)
    if wifi_on {
        for (ssid, in_use) in list_wifi_networks() {
            if !needle.is_empty() && !ssid.to_lowercase().contains(&*needle) {
                continue;
            }
            items.push(Item {
                id: format!("connect:{ssid}"),
                title: ssid.clone(),
                subtitle: Some(if in_use {
                    "Connected".to_string()
                } else {
                    "Known network — click to connect".to_string()
                }),
                icon: Some("network-wireless".to_string()),
                accessories: if in_use {
                    vec!["✓".to_string()]
                } else {
                    Vec::new()
                },
                actions: Vec::new(),
            });
        }
    }

    View::List(ListView {
        title: Some("Network".to_string()),
        placeholder: Some("Filter…".to_string()),
        items,
        empty_text: Some("No matches".to_string()),
        actions: Vec::new(),
    })
}

fn handle_action(_host: &mut dyn HostApi, action_id: &str, item_id: Option<&str>) {
    let target = item_id.unwrap_or(action_id);
    if target == "toggle-wifi" {
        let on = wifi_enabled();
        let arg = if on { "off" } else { "on" };
        let _ = Command::new("nmcli").args(["radio", "wifi", arg]).status();
    } else if target == "toggle-bt" {
        let on = bluetooth_enabled();
        if on {
            let _ = Command::new("rfkill").args(["block", "bluetooth"]).status();
        } else {
            let _ = Command::new("rfkill")
                .args(["unblock", "bluetooth"])
                .status();
        }
    } else if let Some(ssid) = target.strip_prefix("connect:")
        && is_safe_ssid(ssid)
    {
        let _ = Command::new("nmcli")
            .args(["device", "wifi", "connect", ssid])
            .status();
    }
}

fn wifi_status() -> &'static str {
    if wifi_enabled() { "On" } else { "Off" }
}

fn bluetooth_status() -> &'static str {
    if bluetooth_enabled() { "On" } else { "Off" }
}

fn wifi_enabled() -> bool {
    Command::new("nmcli")
        .args(["-t", "-f", "WIFI", "radio"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim() == "enabled")
        .unwrap_or(false)
}

fn bluetooth_enabled() -> bool {
    // rfkill reports "0" for unblocked (= powered on)
    Command::new("rfkill")
        .args(["list", "bluetooth"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.contains("Soft blocked: no"))
        .unwrap_or(false)
}

/// SSIDs are broadcast by anyone nearby and passed to nmcli as a positional
/// argument. nmcli has no `--` separator, so a name like "--ask" would be
/// taken as an option: such networks are not offered at all.
fn is_safe_ssid(ssid: &str) -> bool {
    !ssid.is_empty() && !ssid.starts_with('-')
}

/// Undo `nmcli -t` escaping (`\:` and `\\`) in a field value.
fn unescape_terse(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c == '\\'
            && let Some(next) = chars.next()
        {
            out.push(next);
        } else {
            out.push(c);
        }
    }
    out
}

/// Returns (SSID, in_use) for known Wi-Fi networks.
fn list_wifi_networks() -> Vec<(String, bool)> {
    let output = Command::new("nmcli")
        .args(["-t", "-f", "IN-USE,SSID", "device", "wifi", "list"])
        .output()
        .ok();
    let Some(output) = output else {
        return Vec::new();
    };
    let mut networks = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        // Format: "*:SSID" for connected, ":SSID" for others
        let (marker, ssid) = match line.split_once(':') {
            Some(pair) => pair,
            None => continue,
        };
        let ssid = unescape_terse(ssid.trim());
        if ssid.is_empty() || !is_safe_ssid(&ssid) {
            continue;
        }
        if seen.insert(ssid.clone()) {
            networks.push((ssid, marker.trim() == "*"));
        }
    }
    networks
}

fn nmcli_available() -> bool {
    Command::new("nmcli")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn main() {
    run(Network);
}

#[cfg(test)]
mod tests {
    #[test]
    fn parses_wifi_list_line_connected() {
        let line = "*:MyNetwork";
        let (marker, ssid) = line.split_once(':').unwrap();
        assert_eq!(marker.trim(), "*");
        assert_eq!(ssid.trim(), "MyNetwork");
    }

    #[test]
    fn parses_wifi_list_line_disconnected() {
        let line = ":OtherNetwork";
        let (marker, ssid) = line.split_once(':').unwrap();
        assert_ne!(marker.trim(), "*");
        assert_eq!(ssid.trim(), "OtherNetwork");
    }
}

#[cfg(test)]
mod ssid_tests {
    use super::*;

    #[test]
    fn rejects_option_like_ssids_and_unescapes_terse_output() {
        assert!(!is_safe_ssid("--ask"));
        assert!(!is_safe_ssid("-x"));
        assert!(is_safe_ssid("Gustav"));
        assert_eq!(unescape_terse("Cafe\\:Gast"), "Cafe:Gast");
        assert_eq!(unescape_terse("a\\\\b"), "a\\b");
    }
}
