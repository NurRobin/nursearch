# NurSearch

NurSearch is a small local GTK4 app launcher for Linux desktops. It is meant to be a fast,
privacy-friendly launcher in the spirit of KRunner, Alfred, or Raycast, but without cloud APIs,
AI services, or a background account dependency.

The initial MVP targets CachyOS / Arch Linux with KDE Plasma on Wayland.

## Features

- Undecorated GTK4 search window
- Discovers visible `.desktop` applications from common system and user application directories
- Fuzzy app search with exact, prefix, substring, character-order, keyword, generic-name, and
  comment matching
- Launch with `Enter` or by activating a selected result row
- `Escape` closes the launcher
- Up/down arrow navigation
- Local SQLite launch history at `~/.local/share/nursearch/history.sqlite`
- Global usage ranking and query-specific learning

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

## Known Limitations

- Desktop-file parsing covers common launcher fields but does not fully implement every part of the
  freedesktop.org Desktop Entry Specification.
- Results are refreshed from an in-memory app list loaded at startup.
- There is no daemon mode or global hotkey registration; KDE owns the shortcut binding.
