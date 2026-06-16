# NurSearch

NurSearch is a small local GTK4 app launcher for Linux desktops. It is meant to be a fast,
privacy-friendly launcher in the spirit of KRunner, Alfred, or Raycast, but without cloud APIs,
AI services, or a background account dependency.

The initial MVP targets CachyOS / Arch Linux with KDE Plasma on Wayland.

## Features

- Undecorated GTK4 search window
- Discovers visible `.desktop` applications from common system and user application directories
- Fuzzy app search with exact, prefix, substring, and character-order matching
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
Exec=~/.local/bin/nursearch
```

## KDE Shortcut Setup

To bind NurSearch to `Meta+Space` in KDE Plasma:

1. Open System Settings.
2. Go to Keyboard > Shortcuts.
3. Add a custom command shortcut.
4. Use `~/.local/bin/nursearch` as the command.
5. Assign `Meta+Space`.

If `Meta+Space` is already assigned to KRunner or another launcher, remove or change that binding
first.

## Desktop App Discovery

NurSearch scans `.desktop` files from:

- `/usr/share/applications`
- `/usr/local/share/applications`
- `~/.local/share/applications`
- `$XDG_DATA_DIRS/*/applications`

It includes only visible `Type=Application` entries and skips `NoDisplay=true` and `Hidden=true`.
The `Exec` command is cleaned by removing common desktop placeholders such as `%u`, `%U`, `%f`,
`%F`, `%i`, `%c`, and `%k`.

## Known Limitations

- Desktop-file parsing covers the MVP fields only and does not fully implement the freedesktop.org
  Desktop Entry Specification.
- Icons are parsed but not rendered yet.
- Results are refreshed from an in-memory app list loaded at startup.
- There is no daemon mode or global hotkey registration; KDE owns the shortcut binding.
- Launch errors are currently printed to stderr instead of shown in the UI.
