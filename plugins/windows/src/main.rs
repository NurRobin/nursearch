//! Window switcher for KDE Plasma (Wayland). Activated with the `w` keyword:
//! `w fire` lists matching open windows; activating one focuses it.
//!
//! Robust window enumeration on KWin/Wayland has no simple D-Bus call, so this
//! plugin relies on `kdotool` (a KWin-scripting CLI). If `kdotool` is not
//! installed it contributes nothing.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{ResultItem, ViewEvent};
use std::process::Command;

struct Windows;

impl Plugin for Windows {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        let needle = text.trim().to_lowercase();
        list_windows()
            .into_iter()
            .filter(|(_, title)| needle.is_empty() || title.to_lowercase().contains(&needle))
            .map(|(id, title)| ResultItem {
                id: id.clone(),
                title,
                subtitle: Some("Focus window".to_string()),
                icon: Some("preferences-system-windows".to_string()),
                score: 5_000,
                command_id: id,
                actions: Vec::new(),
            })
            .collect()
    }

    fn activate(
        &mut self,
        _host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        let _ = Command::new("kdotool")
            .args(["windowactivate", command_id])
            .status();
        Some(Response::Close { hide: true })
    }

    fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
        None
    }
}

/// Enumerate windows as (id, title) pairs via `kdotool`.
fn list_windows() -> Vec<(String, String)> {
    let Some(ids) = run_lines("kdotool", &["search", "--name", "."]) else {
        return Vec::new();
    };
    ids.into_iter()
        .filter_map(|id| {
            let title = run_lines("kdotool", &["getwindowname", &id])?.join(" ");
            let title = title.trim().to_string();
            (!title.is_empty()).then_some((id, title))
        })
        .collect()
}

fn run_lines(program: &str, args: &[&str]) -> Option<Vec<String>> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect(),
    )
}

fn main() {
    run(Windows);
}
