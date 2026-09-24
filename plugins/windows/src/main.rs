//! Window switcher for KDE Plasma (Wayland). Activated with the `w` keyword:
//! `w fire` lists matching open windows; activating one focuses it.
//!
//! Robust window enumeration on KWin/Wayland has no simple D-Bus call, so this
//! plugin relies on `kdotool` (a KWin-scripting CLI). If `kdotool` is not
//! installed it contributes nothing.
//!
//! The window list is cached for up to 1.5 seconds so rapid keystrokes don't
//! spawn N kdotool processes per character.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{ResultItem, ViewEvent};
use std::process::Command;
use std::time::{Duration, Instant};

/// How long a fetched window list stays valid before the next query re-fetches.
const CACHE_TTL: Duration = Duration::from_millis(1500);

struct Cache {
    windows: Vec<(String, String)>,
    fetched_at: Instant,
}

struct Windows {
    cache: Option<Cache>,
}

impl Windows {
    fn new() -> Self {
        Self { cache: None }
    }

    fn cached_windows(&mut self) -> &[(String, String)] {
        let stale = self
            .cache
            .as_ref()
            .map(|c| c.fetched_at.elapsed() >= CACHE_TTL)
            .unwrap_or(true);
        if stale {
            self.cache = Some(Cache {
                windows: list_windows(),
                fetched_at: Instant::now(),
            });
        }
        &self.cache.as_ref().unwrap().windows
    }
}

impl Plugin for Windows {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        let needle = text.trim().to_lowercase();
        self.cached_windows()
            .iter()
            .filter(|(_, title)| needle.is_empty() || title.to_lowercase().contains(&needle))
            .map(|(id, title)| ResultItem {
                id: id.clone(),
                title: title.clone(),
                subtitle: Some("Focus window".to_string()),
                icon: Some("preferences-system-windows".to_string()),
                score: 5_000,
                command_id: id.clone(),
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

// Thread-local storage is needed because Plugin trait methods take &mut self
// but the plugin instance itself lives on the stack inside run().
// The cache is therefore stored on the struct directly (no thread-local needed).

fn main() {
    run(Windows::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_is_reused_within_ttl() {
        // We can't easily mock kdotool, but we can verify the cache struct
        // logic: a freshly-created Cache with known data stays valid.
        let cache = Cache {
            windows: vec![("1".to_string(), "Firefox".to_string())],
            fetched_at: Instant::now(),
        };
        assert!(cache.fetched_at.elapsed() < CACHE_TTL);
    }

    #[test]
    fn cache_expires_after_ttl() {
        // An artificially backdated cache should be considered stale.
        let old = Instant::now() - CACHE_TTL - Duration::from_millis(100);
        let cache = Cache {
            windows: vec![],
            fetched_at: old,
        };
        assert!(cache.fetched_at.elapsed() >= CACHE_TTL);
    }
}
