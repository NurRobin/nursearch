# NurSearch Plugin Platform — Design

Date: 2026-06-16
Status: Approved direction; first spec of a multi-spec platform.

## Vision

NurSearch grows from an app launcher into a **command-palette multitool platform**: a
fast built-in core plus a plugin ecosystem (windows, settings, files, clipboard, emoji,
web, and community extensions), an eventual marketplace, and an AI-friendly authoring
kit. The name no longer fits ("not *only* search") — renaming is deferred and out of
scope here.

This is a platform, not a feature, so it is decomposed into sequential sub-projects:

1. **Plugin runtime + protocol** ← *this spec*
2. Official ("first-party") plugins
3. Marketplace (discovery / install / update / trust)
4. AI authoring kit (manifest schema, templates, agent-readable docs)

Per the goal driving implementation, this spec also carries the **first-party plugins**
into scope as the proof and payload of the runtime (sub-project 2 folded in, built e2e
on top of the runtime). Marketplace and AI-kit remain later specs.

## Locked decisions

- **Execution model:** persistent plugin processes speaking **JSON-RPC 2.0 over stdio**
  (newline-delimited, one compact JSON object per line). Language-agnostic.
- **Interaction:** full **push-views/forms** — a stateful render session per plugin.
- **Core split:** a true mandatory **in-process core** (apps, calculator, system) stays
  built-in and always-on; everything else is a plugin.
- **Render model:** **declarative view tree** with a fixed, bounded widget vocabulary
  (`List` / `Detail` / `Form`). Plugins emit complete views as typed data; the host
  renders and diffs.

## Architecture: two planes

### Plane 1 — Root search (flat, aggregated, instant)

The start screen. Sources merge into one ranked flat list:

- **Built-in core** (apps / calc / system): in-process, synchronous, always-on, zero
  latency. Refactored behind an internal `CoreProvider` so it shares the aggregation
  path with plugin contributions.
- **Plugin contributions:** *global* plugins receive every query; *keyword* plugins
  (e.g. `w …`, `g …`) receive only queries beginning with their keyword. Contributions
  arrive **asynchronously** and merge into the ranked list as they land. The root never
  blocks on a plugin.

Ranking reuses the current match + usage-history scoring. History keys are namespaced:
`app:<path>`, `system:<id>`, `plugin:<plugin-id>:<item-id>`. Calculator stays unrecorded.

### Plane 2 — Plugin view session (stateful push-views)

Activating a result that belongs to a plugin hands the screen to that plugin. From then
on it is a render session: the plugin sends `List` / `Detail` / `Form` views, the search
box becomes the active view's input, and keystrokes / selection / actions flow to the
plugin as events. The plugin re-renders (`push`), goes back (`pop`), or ends (`close`).

**Navigation stack & keys:** Root → view → view … `Esc` pops one level; `Esc` at root
hides the launcher (daemon). `Enter` runs the primary action; `Alt+Enter` opens the
action menu.

The split keeps the root instant (core + async plugin crumbs) while the richer,
multi-step interaction is encapsulated in the heavier stateful session protocol.

## Protocol

Transport: JSON-RPC 2.0, one compact JSON object per line, over the plugin's stdin
(host→plugin) and stdout (plugin→host). stderr is captured for plugin logging. A
`protocolVersion` integer is exchanged at `initialize`; mismatch disables the plugin.

### Host → plugin

- `initialize { protocolVersion, hostVersion, pluginId, preferences }` → returns
  `{ protocolVersion, capabilities? }`.
- `query { text, generation }` (root contributions; only for global/keyword plugins) →
  the plugin streams `results` notifications tagged with `generation`.
- `activate { commandId, itemId?, generation }` — a plugin-owned root item was chosen;
  the plugin starts a view session and replies with an initial `render`.
- `event { sessionId, kind, ... }` — input/selection/action/submit/pop within a session:
  - `{ kind: "input", text }`
  - `{ kind: "select", itemId }`
  - `{ kind: "action", actionId, itemId? }`
  - `{ kind: "submit", values }` (form)
  - `{ kind: "pop" }` (user pressed Esc / back)
- `shutdown {}` — graceful stop request.

### Plugin → host (notifications)

- `results { generation, items: [Item], done: bool }` — incremental root contributions;
  the host discards any whose `generation` is stale.
- `render { sessionId, view: View, replace?: bool }` — push (default) or replace the top
  view of the session.
- `pop { sessionId }` / `close { sessionId, hideLauncher?: bool }`.
- Host-capability calls (request/response): `host.clipboardSet { text }`,
  `host.open { target }` (url/path), `host.run { argv }`, `host.toast { text, kind }`,
  `host.storageGet/Set/Delete/List { key, value? }`, `host.closeLauncher {}`.

### View schema (declarative, bounded)

Common to every view: optional `title` (breadcrumb), `placeholder` (search box hint),
and `actions: [Action]` (view-level actions in the action menu).

- `List { items: [Item], emptyText? }`
  - `Item { id, title, subtitle?, icon?, accessories?: [Text|Tag], actions?: [Action] }`
- `Detail { markdown?, metadata?: [{label,value}], actions? }`
- `Form { fields: [Field], submitLabel?, actions? }`
  - `Field { id, type: text|password|number|select|checkbox, label, value?, options?,
    placeholder? }`

`Action { id, title, icon?, shortcut?, kind }` where `kind` is host-handled
(`copy` / `openUrl` / `run` / `paste` / `close`) or `plugin` (re-enters the plugin via an
`event { kind: "action" }`). The first action is primary (Enter).

### Generations & staleness

Every root `query` and session `input` carries a monotonic `generation`. The host renders
only the newest generation's results; late or out-of-order plugin output is dropped. This
keeps fast typing responsive.

## Manifest & packaging

A plugin is a directory containing `nursearch-plugin.toml`:

```toml
id = "com.example.windows"
name = "Window Switcher"
description = "Search and focus open windows"
version = "0.1.0"
author = "…"
license = "MIT"
icon = "preferences-system-windows"
protocolVersion = 1
entry = ["python3", "main.py"]          # or a binary path, relative to the plugin dir

[activation]
mode = "keyword"                         # "global" | "keyword"
keyword = "w"
fallback = false                         # also show when nothing else matches

[[preferences]]                          # rendered by host settings as a Form
id = "limit"
type = "number"
label = "Max results"
default = 20

capabilities = ["run", "open"]           # declared APIs the plugin may call
```

Discovery scans `~/.local/share/nursearch/plugins/<id>/` (user) and a bundled directory.
Manifests are validated (required fields, `protocolVersion`). Invalid plugins are skipped
with a logged reason.

## Capabilities & trust

Capabilities are **declared** in the manifest and gate the host-mediated APIs
(`run`, `open`, `clipboard`, `storage`, `network` is informational). The host cannot
truly sandbox a native subprocess, so the model is **honest trust-on-install**: the
manifest's declared capabilities are shown to the user before enabling, and host APIs
are refused if not declared. Real sandboxing (WASM/bubblewrap) is a future spec.

## Plugin storage (reboot-safe)

The host exposes a per-plugin persistent **key-value + append-log** store, backed by the
existing SQLite database (`history.sqlite`, new tables namespaced per plugin id). This is
what makes a **reboot-safe clipboard history** plugin possible: the clipboard plugin
appends entries via `host.storageSet` / a log API and they survive restarts and reboots.

## Process lifecycle

- **Lazy start:** global plugins start at first root query; keyword plugins start when
  their keyword is first typed. Plugins run as detached child processes.
- **Idle timeout:** a plugin idle for N minutes is shut down (`shutdown`, then kill on
  timeout); restarted on next demand. State persists via the storage API.
- **Crash / misbehavior:** malformed JSON, protocol-version mismatch, or a per-message
  timeout disables the plugin for the session and surfaces an error result; the UI is
  never blocked.
- **Concurrency without a second runtime:** plugin stdio is driven on the GTK/GLib main
  context using async GIO streams (`spawn_future_local` + `gio` input/output streams), so
  the single-threaded GTK model is preserved and the main loop stays responsive.

## Hybrid core integration

The existing `SearchResult` / `Action` model becomes the **root item** model. Built-in
providers move behind an internal `CoreProvider` trait; the aggregator merges core results
and plugin `results` notifications into one ranked list. Activating an item with a plugin
owner opens a view session; activating a core item runs its existing action.

## Settings

- **Per-plugin preferences** are declared in the manifest and rendered by the host as a
  `Form` (reusing the form widget) under a built-in **Settings** command; values persist
  via plugin storage / host config and are passed to the plugin at `initialize`.
- A **global settings GUI** (theme, enabled plugins, plugin management) is **out of scope**
  here but is intentionally served by the same `Form`/preferences mechanism in a later
  spec.

## Plugin SDK

A minimal **Rust reference SDK** crate (`nursearch-plugin`) wraps the stdio JSON-RPC loop,
the view builders, and the host-capability calls, so a plugin author implements a small
trait (`query`, `activate`, `event`) rather than the wire protocol. The AI authoring kit
(later spec) builds docs/templates on top of this.

## First-party plugins (in scope, built e2e on the runtime)

Each is a separate module/crate using the SDK, validating a different protocol facet:

- **Window switcher** (`w`) — list/focus/close windows via KWin D-Bus scripting on KDE
  Wayland; exercises keyword activation + actions.
- **Clipboard history** (`c`) — reboot-safe via storage API; exercises storage + a long
  list + paste action.
- **File search** (`f`) — walk/locate under home; exercises async streaming results.
- **Emoji / symbol picker** (`e`) — exercises a large static list + copy action.
- **Web / quicklinks** (`g`, `ddg`, …) — exercises keyword + openUrl + a Form for
  managing quicklinks.

The window and clipboard plugins are the primary validation targets; the rest follow.

## Error handling

- Protocol: version mismatch, malformed message, per-message timeout → plugin disabled +
  error surfaced; root stays usable.
- Stale results: dropped via generation counter.
- Host-capability misuse (undeclared capability, bad target) → error response to plugin,
  logged; no crash.

## Testing

- **Protocol unit tests:** an in-process mock plugin over pipes drives
  query→results, activate→render, event→render/pop/close, version mismatch, malformed
  input, and timeouts.
- **View rendering tests:** view-tree → widget construction for `List`/`Detail`/`Form`.
- **Storage tests:** per-plugin namespacing, persistence across reopen.
- **Reference + first-party integration tests:** spawn the real process, drive a session,
  assert renders and actions.
- Existing built-in provider / db / desktop / calc tests remain green.

## Milestones (implementation order)

1. **Core refactor + protocol types + design doc** (this doc; `proto` module; built-ins
   behind `CoreProvider`; root item model). 
2. **Plugin host runtime** — discovery, manifest, process lifecycle, async stdio JSON-RPC,
   `initialize`/`shutdown`, error handling.
3. **Root contributions** — `query`/`results`, global + keyword activation, generation
   merging into the ranked root list.
4. **View session** — navigation stack, `activate`/`event`/`render`/`pop`/`close`, GTK
   rendering of `List`/`Detail`/`Form`, action menu.
5. **Host capabilities + storage** — clipboard/open/run/toast/close + SQLite-backed
   per-plugin storage.
6. **Rust SDK** + a minimal reference/echo plugin (protocol proof).
7. **First-party plugins** — clipboard history, window switcher, then files / emoji / web.

## Out of scope (later specs)

- Marketplace (discovery, install, update, signing/trust UI).
- AI authoring kit (docs/templates/generator).
- Global settings GUI.
- True sandboxing (WASM/bubblewrap).
- Rename of the project.
