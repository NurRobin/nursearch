//! Bind NurSearch to the Meta key on KDE Plasma through kglobalaccel's D-Bus
//! API, the same store System Settings > Shortcuts edits.
//!
//! On first run this only happens when NurSearch has no shortcut at all, so a
//! binding the user chose later is never overwritten; `--setup-shortcut`
//! forces it. Meta is taken away from Kickoff ("Activate Application
//! Launcher"), whose other keys (Alt+F1) stay.

use gtk4::gio;
use gtk4::glib;
use gtk4::glib::prelude::*;
use log::{info, warn};
use std::fs;
use std::path::PathBuf;

/// Qt key code of a lone Meta press (`Qt::Key_Meta`).
const KEY_META: i32 = 0x0100_0022;

const NURSEARCH_ACTION: [&str; 4] = ["nursearch.desktop", "_launch", "NurSearch", "NurSearch"];
const KICKOFF_ACTION: [&str; 4] = [
    "plasmashell",
    "activate application launcher",
    "plasmashell",
    "Activate Application Launcher",
];

/// A kglobalaccel shortcut: up to four key combinations, each as the four
/// ints of a `QKeySequence`.
type Keys = Vec<Vec<i32>>;

/// First-run hook: bind Meta once if NurSearch has no shortcut yet.
pub fn ensure_on_first_run() {
    let marker = marker_path();
    if marker.exists() || !is_plasma() {
        return;
    }
    match setup(false) {
        Ok(message) => {
            info!("{message}");
            if let Some(parent) = marker.parent() {
                let _ = fs::create_dir_all(parent);
            }
            if let Err(err) = fs::write(&marker, message) {
                warn!("could not record shortcut setup: {err}");
            }
        }
        // Not marked: kglobalaccel may simply not be up yet this early.
        Err(err) => warn!("could not set up the Meta shortcut: {err}"),
    }
}

/// Bind Meta to NurSearch. Without `force`, an existing NurSearch shortcut is
/// left alone. Returns a human-readable summary.
pub fn setup(force: bool) -> Result<String, String> {
    if !is_plasma() {
        return Err("automatic shortcut setup needs KDE Plasma".to_string());
    }
    let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
        .map_err(|err| err.to_string())?;

    let current = shortcut_keys(&bus, &NURSEARCH_ACTION)?;
    if !force && current.iter().any(|keys| keys.iter().any(|&key| key != 0)) {
        return Ok("NurSearch already has a shortcut; left unchanged".to_string());
    }

    let kickoff = shortcut_keys(&bus, &KICKOFF_ACTION).unwrap_or_default();
    let (kickoff_kept, took_meta) = without_meta(&kickoff);
    if took_meta {
        set_keys(&bus, &KICKOFF_ACTION, &kickoff_kept)?;
    }

    call(
        &bus,
        "doRegister",
        &(NURSEARCH_ACTION.to_vec(),).to_variant(),
    )?;
    // Keep any other keys the user gave NurSearch (e.g. Ctrl+Alt+N).
    let (others, _) = without_meta(&current);
    let mut keys = vec![vec![KEY_META, 0, 0, 0]];
    keys.extend(
        others
            .into_iter()
            .filter(|combo| combo.iter().any(|&key| key != 0)),
    );
    set_keys(&bus, &NURSEARCH_ACTION, &keys)?;
    Ok(if took_meta {
        "Meta now opens NurSearch (removed from the Kickoff launcher)".to_string()
    } else {
        "Meta now opens NurSearch".to_string()
    })
}

/// `keys` minus any lone-Meta binding, and whether one was removed.
fn without_meta(keys: &Keys) -> (Keys, bool) {
    let kept: Keys = keys
        .iter()
        .filter(|combo| combo.first() != Some(&KEY_META) || combo.iter().skip(1).any(|&k| k != 0))
        .cloned()
        .collect();
    let removed = kept.len() != keys.len();
    (kept, removed)
}

fn shortcut_keys(bus: &gio::DBusConnection, action: &[&str; 4]) -> Result<Keys, String> {
    let reply = call(bus, "shortcutKeys", &(action.to_vec(),).to_variant())?;
    let (keys,): (Vec<(Vec<i32>,)>,) = reply
        .get()
        .ok_or_else(|| format!("unexpected shortcutKeys reply: {}", reply.type_()))?;
    Ok(keys.into_iter().map(|(combo,)| combo).collect())
}

fn set_keys(bus: &gio::DBusConnection, action: &[&str; 4], keys: &Keys) -> Result<(), String> {
    let keys: Vec<(Vec<i32>,)> = keys.iter().map(|combo| (combo.clone(),)).collect();
    call(
        bus,
        "setForeignShortcutKeys",
        &(action.to_vec(), keys).to_variant(),
    )
    .map(|_| ())
}

fn call(
    bus: &gio::DBusConnection,
    method: &str,
    parameters: &glib::Variant,
) -> Result<glib::Variant, String> {
    bus.call_sync(
        Some("org.kde.kglobalaccel"),
        "/kglobalaccel",
        "org.kde.KGlobalAccel",
        method,
        Some(parameters),
        None,
        gio::DBusCallFlags::NONE,
        3_000,
        gio::Cancellable::NONE,
    )
    .map_err(|err| format!("kglobalaccel {method}: {err}"))
}

fn is_plasma() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP")
        .map(|desktops| desktops.split(':').any(|desktop| desktop == "KDE"))
        .unwrap_or(false)
}

fn marker_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".local/share/nursearch/shortcut-configured")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removes_only_a_lone_meta_binding() {
        let alt_f1 = vec![0x0900_0030, 0, 0, 0];
        let meta_b = vec![KEY_META + 0x42, 0, 0, 0];
        let keys = vec![vec![KEY_META, 0, 0, 0], alt_f1.clone(), meta_b.clone()];

        assert_eq!(without_meta(&keys), (vec![alt_f1.clone(), meta_b], true));
        assert_eq!(without_meta(&vec![alt_f1.clone()]), (vec![alt_f1], false));
    }
}
