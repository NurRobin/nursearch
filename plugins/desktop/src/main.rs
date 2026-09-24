//! Desktop Toggles plugin. Keyword: `d`
//!
//! Provides quick toggles for KDE Plasma 6 desktop settings:
//! - Night Color: toggle via `org.kde.KWin.NightLight` inhibit/uninhibit D-Bus calls
//! - Do Not Disturb: toggle via `org.freedesktop.Notifications` Inhibit/UnInhibit
//!
//! Both toggles use the inhibitor pattern: enabling DND or disabling night color
//! acquires an inhibitor and saves its cookie to a runtime file under XDG_RUNTIME_DIR;
//! toggling back releases the inhibitor. This means the toggles survive plugin
//! restarts only if the runtime files still exist.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{Item, ListView, ResultItem, View, ViewEvent};
use std::path::PathBuf;
use std::process::Command;

struct Desktop;

impl Plugin for Desktop {
    fn query(&mut self, _host: &mut dyn HostApi, _text: &str) -> Vec<ResultItem> {
        vec![ResultItem {
            id: "open".to_string(),
            title: "Desktop Toggles".to_string(),
            subtitle: Some(status_summary()),
            icon: Some("preferences-desktop".to_string()),
            score: 5_200,
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
            return Some(Response::Render(toggles_view()));
        }
        None
    }

    fn event(&mut self, _host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> {
        match event {
            ViewEvent::Action { action_id, item_id } => {
                let target = item_id.as_deref().unwrap_or(&action_id);
                match target {
                    "toggle-nightcolor" => toggle_night_color(),
                    "toggle-dnd" => toggle_dnd(),
                    _ => {}
                }
                Some(Response::Replace(toggles_view()))
            }
            _ => None,
        }
    }
}

fn status_summary() -> String {
    let nc = if night_color_inhibited() {
        "Night Color: Off"
    } else {
        "Night Color: On"
    };
    let dnd = if dnd_active() { "DND: On" } else { "DND: Off" };
    format!("{nc}  •  {dnd}")
}

fn toggles_view() -> View {
    let nc_inhibited = night_color_inhibited();
    let dnd_on = dnd_active();

    View::List(ListView {
        title: Some("Desktop Toggles".to_string()),
        items: vec![
            Item {
                id: "toggle-nightcolor".to_string(),
                title: if nc_inhibited {
                    "Night Color: Off — click to re-enable"
                } else {
                    "Night Color: On — click to disable"
                }
                .to_string(),
                subtitle: Some("org.kde.KWin.NightLight  (KWin night light filter)".to_string()),
                icon: Some(
                    if nc_inhibited {
                        "weather-clear"
                    } else {
                        "night-light"
                    }
                    .to_string(),
                ),
                accessories: Vec::new(),
                actions: Vec::new(),
            },
            Item {
                id: "toggle-dnd".to_string(),
                title: if dnd_on {
                    "Do Not Disturb: On — click to disable"
                } else {
                    "Do Not Disturb: Off — click to enable"
                }
                .to_string(),
                subtitle: Some(
                    "org.freedesktop.Notifications  (suppress system notifications)".to_string(),
                ),
                icon: Some(
                    if dnd_on {
                        "notifications-disabled"
                    } else {
                        "preferences-desktop-notification"
                    }
                    .to_string(),
                ),
                accessories: Vec::new(),
                actions: Vec::new(),
            },
        ],
        empty_text: None,
        placeholder: None,
        actions: Vec::new(),
    })
}

// ---------------------------------------------------------------------------
// Night Color
// ---------------------------------------------------------------------------

fn night_color_inhibited() -> bool {
    cookie_path("nightcolor").exists()
}

fn toggle_night_color() {
    if night_color_inhibited() {
        release_night_color();
    } else {
        inhibit_night_color();
    }
}

fn inhibit_night_color() {
    // Call org.kde.KWin.NightLight.inhibit() — returns a u32 cookie.
    let output = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.kde.KWin",
            "/org/kde/KWin/NightLight",
            "org.kde.KWin.NightLight",
            "inhibit",
        ])
        .output()
        .ok();
    if let Some(output) = output
        && output.status.success()
    {
        // busctl returns "u <cookie>\n"
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(cookie) = parse_uint_result(&stdout) {
            let _ = std::fs::write(cookie_path("nightcolor"), cookie.to_string());
        }
    }
}

fn release_night_color() {
    let path = cookie_path("nightcolor");
    if let Ok(s) = std::fs::read_to_string(&path)
        && let Ok(cookie) = s.trim().parse::<u32>()
    {
        let _ = Command::new("busctl")
            .args([
                "--user",
                "call",
                "org.kde.KWin",
                "/org/kde/KWin/NightLight",
                "org.kde.KWin.NightLight",
                "uninhibit",
                "u",
                &cookie.to_string(),
            ])
            .status();
    }
    let _ = std::fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// Do Not Disturb
// ---------------------------------------------------------------------------

fn dnd_active() -> bool {
    cookie_path("dnd").exists()
}

fn toggle_dnd() {
    if dnd_active() {
        release_dnd();
    } else {
        inhibit_dnd();
    }
}

fn inhibit_dnd() {
    // Call org.freedesktop.Notifications.Inhibit(app, reason, hints{})
    // Signature: ssa{sv} → u (cookie)
    let output = Command::new("busctl")
        .args([
            "--user",
            "call",
            "org.freedesktop.Notifications",
            "/org/freedesktop/Notifications",
            "org.freedesktop.Notifications",
            "Inhibit",
            "ssa{sv}",
            "nursearch",
            "Do Not Disturb",
            "0",
        ])
        .output()
        .ok();
    if let Some(output) = output
        && output.status.success()
    {
        let stdout = String::from_utf8_lossy(&output.stdout);
        if let Some(cookie) = parse_uint_result(&stdout) {
            let _ = std::fs::write(cookie_path("dnd"), cookie.to_string());
        }
    }
}

fn release_dnd() {
    let path = cookie_path("dnd");
    if let Ok(s) = std::fs::read_to_string(&path)
        && let Ok(cookie) = s.trim().parse::<u32>()
    {
        let _ = Command::new("busctl")
            .args([
                "--user",
                "call",
                "org.freedesktop.Notifications",
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
                "UnInhibit",
                "u",
                &cookie.to_string(),
            ])
            .status();
    }
    let _ = std::fs::remove_file(path);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Path for storing an inhibitor cookie, under XDG_RUNTIME_DIR.
fn cookie_path(name: &str) -> PathBuf {
    let runtime_dir =
        std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{}", libc_getuid()));
    PathBuf::from(runtime_dir).join(format!("nursearch-{name}-cookie"))
}

/// Parse a `busctl` uint result line of the form `"u <value>\n"`.
fn parse_uint_result(s: &str) -> Option<u32> {
    let token = s.trim().strip_prefix("u ")?.trim();
    token.parse().ok()
}

/// Minimal libc uid binding to avoid adding a crate dependency.
fn libc_getuid() -> u32 {
    // Safety: getuid() is always safe to call — it has no preconditions.
    unsafe extern "C" {
        fn getuid() -> u32;
    }
    unsafe { getuid() }
}

fn main() {
    run(Desktop);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_busctl_uint_result() {
        assert_eq!(parse_uint_result("u 42\n"), Some(42));
        assert_eq!(parse_uint_result("u 0"), Some(0));
        assert_eq!(parse_uint_result("b false"), None);
        assert_eq!(parse_uint_result(""), None);
    }
}
