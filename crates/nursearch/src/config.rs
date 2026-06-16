//! Theming and configuration. A built-in stylesheet is always applied as the
//! base, and a user file at `~/.config/nursearch/style.css` is overlaid on top
//! and watched for changes so edits take effect without restarting.

use gtk::gdk;
use gtk::gio;
use gtk::prelude::*;
use gtk4 as gtk;
use log::{debug, warn};
use std::fs;
use std::path::PathBuf;

/// Built-in stylesheet. Mirrored to disk on first run so it can be customized.
pub const DEFAULT_CSS: &str = include_str!("style.css");

/// `~/.config/nursearch`, honoring `XDG_CONFIG_HOME`.
pub fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        return PathBuf::from(xdg).join("nursearch");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config/nursearch")
}

/// Install the base theme plus the user overlay, returning the file monitor
/// that drives hot reloading. The monitor must be kept alive by the caller.
pub fn install_css() -> Option<gio::FileMonitor> {
    let display = gdk::Display::default()?;

    let base = gtk::CssProvider::new();
    base.load_from_data(DEFAULT_CSS);
    gtk::style_context_add_provider_for_display(
        &display,
        &base,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );

    let style_path = ensure_user_style();
    let user = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(&display, &user, gtk::STYLE_PROVIDER_PRIORITY_USER);
    load_user_style(&user, &style_path);

    let file = gio::File::for_path(&style_path);
    let monitor = file
        .monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
        .ok()?;
    monitor.connect_changed(move |_, _, _, _| {
        debug!("style.css changed; reloading theme");
        load_user_style(&user, &style_path);
    });
    Some(monitor)
}

/// Write the default stylesheet to the config dir on first run so users have a
/// starting point to edit. Returns the path either way.
fn ensure_user_style() -> PathBuf {
    let dir = config_dir();
    let path = dir.join("style.css");
    if !path.exists() {
        if let Err(err) = fs::create_dir_all(&dir).and_then(|()| fs::write(&path, DEFAULT_CSS)) {
            warn!("could not write default style.css: {err}");
        } else {
            debug!("wrote default style.css to {}", path.display());
        }
    }
    path
}

fn load_user_style(provider: &gtk::CssProvider, path: &PathBuf) {
    if path.exists() {
        provider.load_from_path(path);
    } else {
        // File removed: fall back to the built-in base only.
        provider.load_from_data("");
    }
}
