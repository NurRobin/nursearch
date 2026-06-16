# Writing a NurSearch Plugin

A plugin is a normal program that speaks the NurSearch protocol over stdio. It
runs as a persistent process; the launcher sends it requests and it replies with
results and views. Plugins are language-agnostic, but the easiest path is the
Rust SDK (`nursearch-plugin`).

## Anatomy

A plugin is a directory containing a `nursearch-plugin.toml` manifest and an
executable. Install it under `~/.local/share/nursearch/plugins/<id>/`, or point
`NURSEARCH_PLUGIN_DIR` at a directory of plugins during development.

```toml
id = "com.example.notes"      # unique, reverse-DNS recommended
name = "Notes"
description = "Search your notes"
version = "0.1.0"
protocol_version = 1
entry = ["python3", "main.py"]   # resolved with cwd = the plugin directory
capabilities = ["clipboard", "storage", "open", "run"]  # what the host lets you call

[activation]
mode = "keyword"   # "global" (every query) or "keyword"
keyword = "n"      # active when the query starts with "n "
```

## Protocol in one minute

Newline-delimited JSON, one object per line. The host sends, you reply:

- `initialize` → reply `initialized`.
- `query` (global / active-keyword plugins) → reply `results` with flat,
  rankable items. Tag them with the query `generation` the host sent.
- `activate` (a result was chosen) → reply `render` with a view (`list`,
  `detail`, or `form`).
- `event` (input/select/action/submit/pop within a view) → reply `render`,
  `pop`, or `close`.

You may call host capabilities at any time (`clipboardSet`, `open`, `run`,
`toast`, `storageGet/Set/Delete/List`, `closeLauncher`); the host replies with a
correlated result. Storage is per-plugin and persists across reboots.

See `crates/nursearch-proto` for the exact message and view schema.

## Rust SDK

Implement the `Plugin` trait and call `run`. The SDK handles framing, the
handshake, and host calls.

```rust
use nursearch_plugin::{run, HostApi, Plugin, Response};
use nursearch_proto::{ResultItem, View, ViewEvent};

struct Notes;
impl Plugin for Notes {
    fn query(&mut self, host: &mut dyn HostApi, text: &str) -> Vec<ResultItem> { /* … */ vec![] }
    fn activate(&mut self, host: &mut dyn HostApi, command_id: &str, item_id: Option<String>) -> Option<Response> { None }
    fn event(&mut self, host: &mut dyn HostApi, event: ViewEvent) -> Option<Response> { None }
}
fn main() { run(Notes); }
```

The bundled plugins are worked examples:

- `plugins/demo` — list → detail navigation and a copy action.
- `plugins/clipboard` — host storage (reboot-safe) and a List session.
- `plugins/emoji` / `plugins/web` / `plugins/files` — root contributions that
  copy / open on activate.
- `plugins/windows` — KDE Wayland window switcher via `kdotool`.

## More

- **Agent / step-by-step guide:** [AGENTS-PLUGIN-GUIDE.md](AGENTS-PLUGIN-GUIDE.md)
  — a complete, self-contained protocol reference an LLM can author from.
- **Templates:** copy `templates/python-plugin` (no build step) or
  `templates/rust-plugin` (uses the SDK) to start.
- **Manifest schema:** [plugin-manifest.schema.json](plugin-manifest.schema.json).
