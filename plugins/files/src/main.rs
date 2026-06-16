//! File search. Activated with the `f` keyword: `f report.pdf` searches your
//! home directory; activating a result opens it with the default application.
//!
//! Uses `fd` when available (fast), falling back to `find`.

use nursearch_plugin::{HostApi, Plugin, Response, run};
use nursearch_proto::{ResultItem, ViewEvent};
use std::path::Path;
use std::process::Command;

const MAX_RESULTS: usize = 20;

struct Files;

impl Plugin for Files {
    fn query(&mut self, _host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> {
        let needle = text.trim();
        if needle.len() < 2 {
            return Vec::new();
        }
        search(needle)
            .into_iter()
            .enumerate()
            .map(|(index, path)| {
                let name = Path::new(&path)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.clone());
                ResultItem {
                    id: path.clone(),
                    title: name,
                    subtitle: Some(path.clone()),
                    icon: Some("text-x-generic".to_string()),
                    score: 5_000 - index as i64,
                    command_id: path,
                    actions: Vec::new(),
                }
            })
            .collect()
    }

    fn activate(
        &mut self,
        host: &mut dyn HostApi,
        command_id: &str,
        _item_id: Option<String>,
    ) -> Option<Response> {
        host.open(command_id);
        Some(Response::Close { hide: true })
    }

    fn event(&mut self, _host: &mut dyn HostApi, _event: ViewEvent) -> Option<Response> {
        None
    }
}

fn home() -> String {
    std::env::var("HOME").unwrap_or_else(|_| ".".to_string())
}

/// Search for files matching `needle`, preferring `fd` and falling back to `find`.
fn search(needle: &str) -> Vec<String> {
    // Refuse needles that look like flags so they cannot smuggle options into
    // the child process; `--` further separates options from positionals.
    if needle.starts_with('-') {
        return Vec::new();
    }

    if let Some(lines) = run_lines(
        "fd",
        &[
            "--type",
            "f",
            "--hidden",
            "--exclude",
            ".git",
            "--max-results",
            &MAX_RESULTS.to_string(),
            "--",
            needle,
            &home(),
        ],
    ) {
        return lines;
    }

    run_lines(
        "find",
        &[
            &home(),
            "-type",
            "f",
            "-iname",
            &format!("*{needle}*"),
        ],
    )
    .map(|mut lines| {
        lines.truncate(MAX_RESULTS);
        lines
    })
    .unwrap_or_default()
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
    run(Files);
}
