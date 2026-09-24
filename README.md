# NurSearch

NurSearch is a small local GTK4 app launcher for Linux desktops. It is meant to be a fast,
privacy-friendly launcher in the spirit of KRunner, Alfred, or Raycast, but without cloud APIs,
AI services, or a background account dependency.

The initial MVP targets CachyOS / Arch Linux with KDE Plasma on Wayland.

## Features

- Undecorated GTK4 search window with a gold-accent theme and keyboard hint bar
- Discovers visible `.desktop` applications from common system and user application directories
- Fuzzy app search with exact, prefix, substring, character-order, keyword, generic-name, and
  comment matching
- Inline calculator: type an expression (e.g. `2 + 3 * 4`, `2,5 × 4`, `2^10`); `Enter` copies the
  result. A decimal comma is answered with a comma
- Direct targets: paste a URL (`https://…`, `github.com/…`) or a path (`~/Downloads`, `/etc`) and
  press Enter to open it; `> command` runs a shell command line
- Quicklinks from `~/.config/nursearch/config.toml`: `aw wayland` searches the Arch Wiki, `gh`,
  `aur` and `yt` are preset, and you can add your own
- KDE System Settings pages as results: `bluetooth`, `hdr` or `mouse` opens the matching page,
  found through the keywords each settings module ships
- Built-in system actions: lock, log out, suspend, reboot, shut down (matched by word start).
  Log out, reboot, and shut down open Plasma's confirmation prompt instead of acting directly
- Launch with `Enter` or by activating a selected result row
- `Escape` hides the launcher
- Up/down arrow navigation
- Local SQLite launch history at `~/.local/share/nursearch/history.sqlite`
- Global usage ranking and query-specific learning
- Live reload: the app list updates automatically when `.desktop` files change
- Daemon mode: the first launch stays resident; later invocations reopen instantly.
  The package starts it hidden at login (`nursearch --background`), so even the first
  open after login skips GTK's cold start
- Themeable via `~/.config/nursearch/style.css` with live reload on save
- Localized UI: English by default, German when the session locale starts with `de`
- Plugin platform: a fast built-in core (apps, calculator, system) plus a
  plugin protocol with push-view sessions, per-plugin persistent storage, and a
  Rust SDK. Bundled plugins: clipboard history, emoji picker, web search, file
  search, and a KDE window switcher. Manage them with the `nursearch-plugins`
  CLI (`list` / `install <path|git-url>` / `remove <id>`). See
  [docs/PLUGINS.md](docs/PLUGINS.md) and the
  [agent authoring guide](docs/AGENTS-PLUGIN-GUIDE.md)

## Dependencies

On CachyOS / Arch Linux:

```sh
sudo pacman -S --needed rustup gtk4 pkgconf
rustup default stable
```

SQLite is used through the system library via the Rust `rusqlite` crate.

## Build And Run

```sh
cargo run
```

For release builds:

```sh
cargo build --release
```

## Optional Local Install

Install the binary into `~/.local/bin`:

```sh
mkdir -p ~/.local/bin
cp target/release/nursearch ~/.local/bin/nursearch
```

Install the desktop entry:

```sh
mkdir -p ~/.local/share/applications
cp nursearch.desktop ~/.local/share/applications/nursearch.desktop
```

The desktop entry uses:

```desktop
Exec=nursearch
```

Make sure `~/.local/bin` is in your desktop session's `PATH`.

To start NurSearch hidden at login (the AUR package does this system-wide):

```sh
mkdir -p ~/.config/autostart
cp nursearch-autostart.desktop ~/.config/autostart/nursearch.desktop
```

## Releases

Version tags publish the Arch package to the AUR:

```sh
git tag v0.2.3
git push origin v0.2.3
```

The GitHub repository needs an `AUR_SSH_PRIVATE_KEY` secret for an SSH key that
is registered with the AUR account allowed to push `nursearch.git`.

## KDE Shortcut Setup

To bind NurSearch to `Meta+Space` in KDE Plasma:

1. Open System Settings.
2. Go to Keyboard > Shortcuts.
3. Add a custom command shortcut.
4. Use `nursearch` as the command.
5. Assign `Meta+Space`.

If `Meta+Space` is already assigned to KRunner or another launcher, remove or change that binding
first.

## Desktop App Discovery

NurSearch scans `.desktop` files from:

- `/usr/share/applications`
- `/usr/local/share/applications`
- `~/.local/share/applications`
- `$XDG_DATA_DIRS/*/applications`

It includes only visible `Type=Application` entries, skips `NoDisplay=true` and `Hidden=true`,
honors `OnlyShowIn`, `NotShowIn`, and `TryExec`, and indexes `Name`, `GenericName`, `Comment`,
and `Keywords`.

App launching is delegated to GLib/GIO's desktop app launcher when possible, with a local fallback
for parsed `Exec` commands.

## Theming

The built-in theme (`crates/nursearch/src/style.css`) is always loaded. On first run NurSearch
creates `~/.config/nursearch/style.css` as a commented template; rules you add there are applied on
top and reload live on save. Everything you do not override follows the built-in theme, including
its updates. An untouched full copy of the theme written by versions up to 0.3.0 is replaced by the
template automatically; edited files are left alone.

## Settings

On first run NurSearch writes a commented `~/.config/nursearch/config.toml`. Changes apply the next
time the launcher opens (except `window.position`, which needs a restart):

```toml
[window]
position = "center"   # or "top" (upper part of the screen, via layer-shell)
width = 720
top_margin = 160      # used with position = "top"

[search]
max_results = 12

[[quicklinks]]        # "aw wayland" opens the Arch Wiki search
keyword = "aw"
name = "Arch Wiki"
url = "https://wiki.archlinux.org/index.php?search={query}"
```

A quicklink without `{query}` in its URL is a bookmark opened by its bare keyword. Plugin keywords
(`c`, `e`, `f`, `g`, `w`, …) take precedence over quicklinks with the same keyword.

## Known Limitations

- Desktop-file parsing covers common launcher fields but does not fully implement every part of the
  freedesktop.org Desktop Entry Specification.
- There is no global hotkey registration; KDE owns the shortcut binding.
- The `Abmelden` (log out) action targets KDE Plasma via `qdbus`; other actions use
  `systemctl`/`loginctl`.
