# Building a NurSearch Plugin (Agent Guide)

This is a complete, self-contained reference for writing a NurSearch plugin. An
LLM or a developer should be able to produce a working plugin from this file
alone. Copy a template from `templates/` and adapt it.

## Mental model

A plugin is a long-running process. The launcher (host) talks to it over stdio
with **newline-delimited JSON** — one JSON object per line. The plugin reads
requests on stdin and writes responses/notifications on stdout. stderr is free
for logging.

Two interaction modes share the channel:

1. **Root contributions** — flat, ranked list items shown on the launcher's
   start screen. The host sends `query`; you reply with `results`.
2. **View sessions** — when the user activates one of your items, you take over
   the screen and push declarative views (`list` / `detail` / `form`). The host
   sends `activate` then `event`s; you reply with `render` / `pop` / `close`.

Activation is set in the manifest: `global` plugins receive every query;
`keyword` plugins (e.g. `g`) only receive queries beginning with their keyword,
and then they own the root list.

## Manifest (`nursearch-plugin.toml`)

```toml
id = "com.example.thing"        # unique; reverse-DNS recommended
name = "Thing"
description = "What it does"
version = "0.1.0"
protocol_version = 1            # must equal the host's; current = 1
entry = ["python3", "main.py"]  # launched with cwd = this directory
capabilities = ["clipboard", "open", "run", "storage"]  # host APIs you may call

[activation]
mode = "keyword"                # "global" or "keyword"
keyword = "t"                   # required for keyword mode
```

Install at `~/.local/share/nursearch/plugins/<dir>/`, or set
`NURSEARCH_PLUGIN_DIR=/path/to/plugins` for development.

## Messages: host → plugin

Each is a JSON object with a `type`. Field names are camelCase.

- `{"type":"initialize","protocolVersion":1,"hostVersion":"…","pluginId":"…","preferences":{…}}`
- `{"type":"query","generation":N,"text":"…"}` — root query (global/active-keyword only)
- `{"type":"activate","generation":N,"commandId":"…","itemId":"…"}` — open a session
- `{"type":"event","generation":N,"event":{…}}` — see view events below
- `{"type":"hostResult","id":N,"outcome":{…}}` — reply to your host call
- `{"type":"shutdown"}` — exit cleanly

View events (`event.event`), discriminated by `kind`:

- `{"kind":"input","text":"…"}` — the search box changed
- `{"kind":"select","itemId":"…"}` — an item was highlighted
- `{"kind":"action","actionId":"…","itemId":"…"}` — an action fired
- `{"kind":"submit","values":{"fieldId":"value"}}` — a form was submitted
- `{"kind":"pop"}` — reserved; the reference host owns back navigation and does not currently send this

## Messages: plugin → host

- `{"type":"initialized","protocolVersion":1,"capabilities":[]}` — reply to initialize
- `{"type":"results","generation":N,"items":[ResultItem],"done":true}` — echo the query's generation
- `{"type":"render","generation":N,"replace":false,"view":View}` — push (or replace) a view. Echo the `generation` from the `activate`/`event` you are answering so the host can drop stale renders from older input. Omitting it (0) skips that protection.
- `{"type":"pop","generation":N}` / `{"type":"close","generation":N,"hideLauncher":true}` — also echo the triggering `generation`, like render, so stale pops/closes are dropped (0 skips that protection). Note: the host owns back navigation, so it does not send a `pop` event on Esc; send a `pop` message only when your plugin itself wants to go back.
- `{"type":"hostCall","id":N,"call":{…}}` — invoke a host capability (await `hostResult`)
- `{"type":"log","level":"info","message":"…"}`

`ResultItem`: `{ "id", "title", "subtitle"?, "icon"?, "score", "commandId", "actions"? }`.
Activating it sends `activate` with its `commandId`.

## Views (declarative)

```json
{"type":"list","title":"…","placeholder":"…","emptyText":"…",
 "items":[{"id","title","subtitle"?,"icon"?,"accessories"?:["tag"],"actions"?:[Action]}],
 "actions":[Action]}

{"type":"detail","title":"…","markdown":"…","metadata":[{"label","value"}],"actions":[Action]}

{"type":"form","title":"…","submitLabel":"…",
 "fields":[{"id","type":"text|password|number|select|checkbox","label","value"?,"placeholder"?,
            "options"?:[{"value","label"}]}],
 "actions":[Action]}
```

`Action`: `{ "id", "title", "icon"?, "shortcut"?, "kind": {…} }`. Action kinds:

- `{"do":"plugin"}` — re-enters you via an `action` event (default)
- `{"do":"copy","text":"…"}` — host copies to clipboard, then closes
- `{"do":"openUrl","url":"…"}` / `{"do":"run","argv":["…"]}` / `{"do":"paste","text":"…"}` / `{"do":"close"}`

The first action is the primary one (Enter); the rest appear in the Alt+Enter menu.

## Host capabilities

Send `{"type":"hostCall","id":N,"call":{"name":"…",…}}` with a unique `id` and
wait for the `hostResult` carrying that same `id`. Calls require the capability
to be declared in the manifest. While you wait for the result, the host may have
already sent further `query`/`event`/`shutdown` messages: buffer any non-matching
message and process it after the call instead of discarding it, or you will drop
input. The Rust SDK and the Python template both do this for you.

- `{"name":"clipboardSet","text":"…"}`
- `{"name":"open","target":"url-or-path"}`
- `{"name":"run","argv":["prog","arg"]}`
- `{"name":"toast","text":"…"}`
- `{"name":"storageGet","key":"…"}` → outcome.value is the string or null
- `{"name":"storageSet","key":"…","value":"…"}`
- `{"name":"storageDelete","key":"…"}`
- `{"name":"storageList","prefix":"…"}` → outcome.value is `[{"key","value"}]`
- `{"name":"closeLauncher"}` (only honored for the plugin that owns the active session)

Storage is per-plugin and persists across restarts and reboots.

## Minimal Python plugin (no dependencies)

```python
import sys, json
def send(o): sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    m = json.loads(line) if line.strip() else {}
    t = m.get("type")
    if t == "initialize":
        send({"type": "initialized", "protocolVersion": 1})
    elif t == "query":
        send({"type": "results", "generation": m["generation"], "done": True,
              "items": [{"id": "1", "title": "Hello " + m["text"],
                         "commandId": "open", "score": 100}]})
    elif t == "activate":
        send({"type": "render", "generation": m.get("generation", 0),
              "view": {"type": "detail", "markdown": "Hello!"}})
    elif t == "shutdown":
        break
```

## Rust plugin (with the SDK)

Implement `nursearch_plugin::Plugin` and call `run`. See `templates/rust-plugin`
and the worked examples under `plugins/`.

## Checklist

- [ ] Manifest validates: unique `id`, `protocol_version = 1`, non-empty `entry`,
      `keyword` set if `mode = "keyword"`.
- [ ] Reply to `initialize` with `initialized` before anything else.
- [ ] Echo the `generation` from `query` in your `results`.
- [ ] Declare every capability you call.
- [ ] Keep `query` fast; do slow work lazily inside a session.
