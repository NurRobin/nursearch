use crate::desktop::DesktopEntry;
use gtk4::gio;
use gtk4::gio::prelude::*;
use gtk4::glib;
use log::{debug, error, info, warn};
use std::io;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

pub fn launch(app: &DesktopEntry) -> io::Result<()> {
    debug!(
        "launch request: name={:?}, desktop_file={}, dbus_activatable={}, terminal={}, has_exec={}",
        app.name,
        app.path.display(),
        app.dbus_activatable,
        app.terminal,
        app.exec.is_some()
    );

    if app.exec.is_some() && !app.terminal && !app.dbus_activatable {
        return launch_exec(app);
    }

    if let Some(info) = gio::DesktopAppInfo::from_filename(&app.path) {
        debug!(
            "using GIO desktop launcher: name={:?}, desktop_file={}",
            app.name,
            app.path.display()
        );

        // D-Bus activated apps are started by the bus, never by us, so the PID
        // callback does not fire for them. Everything GIO spawns itself (e.g.
        // terminal apps) is moved out of the daemon's cgroup into its own scope.
        let app_id = desktop_file_id(&app.path);
        let result = info.launch_uris_as_manager(
            &[],
            gio::AppLaunchContext::NONE,
            glib::SpawnFlags::SEARCH_PATH,
            None,
            Some(&mut |_, pid| move_into_scope(&app_id, pid.0)),
        );
        return match result {
            Ok(()) => {
                info!("GIO launch accepted: {}", app.name);
                Ok(())
            }
            Err(err) => {
                error!("GIO launch failed for {}: {err}", app.name);
                Err(io::Error::other(err.to_string()))
            }
        };
    }

    if app.dbus_activatable && app.exec.is_none() {
        error!(
            "cannot launch D-Bus activatable app without GIO support: name={:?}, desktop_file={}",
            app.name,
            app.path.display()
        );
        return Err(io::Error::other(
            "D-Bus activatable desktop entry could not be loaded by GIO",
        ));
    }

    if app.terminal {
        error!(
            "cannot launch terminal app without GIO support: name={:?}, desktop_file={}",
            app.name,
            app.path.display()
        );
        return Err(io::Error::other(
            "terminal desktop entries require GIO launch support",
        ));
    }

    launch_exec(app)
}

fn launch_exec(app: &DesktopEntry) -> io::Result<()> {
    let args = match app.exec_args() {
        Ok(args) => args,
        Err(err) => {
            error!("failed to parse Exec command for {}: {err}", app.name);
            return Err(err);
        }
    };
    debug!("parsed Exec command for {}: {:?}", app.name, args);
    spawn_app(&args, &desktop_file_id(&app.path))
}

/// Start a long-running program (e.g. a System Settings page) detached from
/// the daemon, like an app. `app_id` names its systemd unit.
pub fn spawn_detached<S: AsRef<str>>(args: &[S], app_id: &str) -> io::Result<()> {
    let args: Vec<String> = args.iter().map(|arg| arg.as_ref().to_string()).collect();
    spawn_app(&args, app_id)
}

/// Open a URL or file path with its default application. `xdg-open` runs in
/// its own unit named after that application, so a browser it starts is not
/// attributed to (or killed with) the daemon.
pub fn open_uri(target: &str) -> io::Result<()> {
    let app_id = default_handler_id(target).unwrap_or_else(|| "xdg-open".to_string());
    spawn_detached(&["xdg-open", target], &app_id)
}

/// Desktop file ID of the default handler for `target`, if GIO knows one.
fn default_handler_id(target: &str) -> Option<String> {
    let file = if target.contains("://") {
        gio::File::for_uri(target)
    } else {
        gio::File::for_path(target)
    };
    let scheme = file.uri_scheme()?;
    let info = if scheme == "file" {
        file.query_default_handler(gio::Cancellable::NONE).ok()?
    } else {
        gio::AppInfo::default_for_uri_scheme(&scheme)?
    };
    let id = info.id()?;
    Some(id.trim_end_matches(".desktop").to_string())
}

/// Desktop file ID without the `.desktop` suffix, e.g. `org.kde.dolphin`.
fn desktop_file_id(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "app".to_string())
}

/// Unit name following the systemd XDG application convention,
/// `app-<launcher>-<ApplicationID>-<RANDOM>.<suffix>`, so system monitors
/// show the app by name. Characters systemd reserves (including `-`, the
/// field separator) are `\xNN`-escaped as `systemd-escape` does.
fn unit_name(app_id: &str, unique: &str, suffix: &str) -> String {
    let mut escaped = String::with_capacity(app_id.len());
    for (index, byte) in app_id.bytes().enumerate() {
        let keep = byte.is_ascii_alphanumeric()
            || byte == b':'
            || byte == b'_'
            || (byte == b'.' && index > 0);
        if keep {
            escaped.push(byte as char);
        } else {
            escaped.push_str(&format!("\\x{byte:02x}"));
        }
    }
    format!("app-nursearch-{escaped}-{unique}.{suffix}")
}

/// A unit-name suffix unique for this daemon's lifetime.
fn unique_suffix() -> String {
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("{:x}{count:x}", std::process::id())
}

/// Move an already running process into a new transient scope of the systemd
/// user manager, taking it out of the daemon's cgroup. Asynchronous and best
/// effort: on failure the app keeps running, just attributed to NurSearch.
fn move_into_scope(app_id: &str, pid: i32) {
    let Ok(pid) = u32::try_from(pid) else {
        return;
    };
    let bus = match gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
        Ok(bus) => bus,
        Err(err) => {
            warn!("no session bus to move pid {pid} into a scope: {err}");
            return;
        }
    };
    let name = unit_name(app_id, &pid.to_string(), "scope");
    let properties: Vec<(String, glib::Variant)> = vec![
        ("PIDs".to_string(), vec![pid].to_variant()),
        ("CollectMode".to_string(), "inactive-or-failed".to_variant()),
    ];
    let auxiliary: Vec<(String, Vec<(String, glib::Variant)>)> = Vec::new();
    let parameters = (name.as_str(), "fail", properties, auxiliary).to_variant();
    bus.call(
        Some("org.freedesktop.systemd1"),
        "/org/freedesktop/systemd1",
        "org.freedesktop.systemd1.Manager",
        "StartTransientUnit",
        Some(&parameters),
        None,
        gio::DBusCallFlags::NONE,
        5_000,
        gio::Cancellable::NONE,
        move |result| match result {
            Ok(_) => info!("moved pid {pid} into {name}"),
            Err(err) => warn!("could not move pid {pid} into its own scope: {err}"),
        },
    );
}

/// Launch an application command fully detached from the NurSearch daemon.
///
/// On a systemd user session the app is started as a transient unit via
/// `systemd-run --user`. The systemd user manager then owns the app, which
/// means:
///   * NurSearch is no longer the app's parent, so quitting or restarting the
///     daemon can't take launched apps down with it, and
///   * the app lives in its own cgroup instead of inheriting NurSearch's
///     service cgroup — without this, system monitors attribute the launched
///     app's CPU, memory, and network to NurSearch (e.g. a launched Electron
///     app's traffic showing up as constant NurSearch up/download).
///
/// Falls back to a plain detached spawn when systemd is unavailable.
fn spawn_app(args: &[String], app_id: &str) -> io::Result<()> {
    if Path::new("/run/systemd/system").exists() {
        match run_via_systemd(args, app_id) {
            Ok(()) => return Ok(()),
            Err(err) => warn!("systemd-run launch failed, spawning directly: {err}"),
        }
    }
    run_command(args)
}

/// Start `args` as a transient `systemd --user` unit. Waits for `systemd-run`
/// itself (a fast D-Bus registration that exits once the user manager has
/// taken ownership) so the helper is reaped rather than left as a zombie, and
/// so a registration failure can fall back to a direct spawn.
fn run_via_systemd(args: &[String], app_id: &str) -> io::Result<()> {
    let unit = unit_name(app_id, &unique_suffix(), "service");
    let status = Command::new("systemd-run")
        .args(["--user", "--collect", "--quiet"])
        .arg(format!("--unit={unit}"))
        .arg("--")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;

    if status.success() {
        info!("launched via systemd-run: {args:?}");
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "systemd-run exited with {status}"
        )))
    }
}

/// Synthesize a `Ctrl+V` paste into whatever window currently holds focus.
///
/// The text has already been placed on the clipboard by the caller, which then
/// hides the launcher so focus returns to the previous window; this presses the
/// paste shortcut there. It tries the input-synthesis tools commonly available
/// on Wayland and X11 in turn and uses the first one that is installed. This is
/// best-effort: if none is present the user can still paste manually, so a
/// missing tool is reported but not fatal.
pub fn paste_into_focused() -> io::Result<()> {
    // Most-preferred first: wtype (Wayland), ydotool (Wayland, uinput),
    // xdotool (X11/XWayland). ydotool uses Linux input keycodes (29 = LeftCtrl,
    // 47 = V) as `code:state` press/release pairs.
    let candidates: [&[&str]; 3] = [
        &["wtype", "-M", "ctrl", "-k", "v", "-m", "ctrl"],
        &["ydotool", "key", "29:1", "47:1", "47:0", "29:0"],
        &["xdotool", "key", "--clearmodifiers", "ctrl+v"],
    ];

    let mut last_err = None;
    for argv in candidates {
        // A missing binary fails synchronously at spawn (ENOENT), so a spawn
        // error means "tool not installed" — fall through and try the next one.
        match run_command(argv) {
            Ok(()) => {
                debug!("synthesized paste via {:?}", argv[0]);
                return Ok(());
            }
            Err(err) => last_err = Some(err),
        }
    }
    Err(last_err
        .unwrap_or_else(|| io::Error::other("no paste tool (wtype/ydotool/xdotool) available")))
}

/// Spawn a short-lived helper command (system actions, paste helpers, and the
/// fallback when systemd is unavailable), discarding its standard streams. It
/// stays a child of the daemon; long-running programs go through [`spawn_app`].
pub fn run_command<S: AsRef<str>>(args: &[S]) -> io::Result<()> {
    let Some((program, rest)) = args.split_first() else {
        error!("cannot run an empty command");
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "empty command"));
    };
    let program = program.as_ref();
    let rest: Vec<&str> = rest.iter().map(AsRef::as_ref).collect();

    debug!("spawning command: program={program:?}, args={rest:?}");
    let mut command = Command::new(program);
    command
        .args(&rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);

    match command.spawn() {
        Ok(mut child) => {
            info!("command spawned: pid={}, program={program:?}", child.id());
            // Reap it, or every finished command lingers as a zombie for the
            // daemon's whole lifetime. Commands run here are short-lived.
            std::thread::spawn(move || child.wait());
            Ok(())
        }
        Err(err) => {
            error!("command failed: program={program:?}, args={rest:?}, error={err}");
            Err(err)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::unit_name;

    #[test]
    fn unit_names_escape_reserved_characters() {
        assert_eq!(
            unit_name("org.kde.dolphin", "1f", "scope"),
            "app-nursearch-org.kde.dolphin-1f.scope"
        );
        assert_eq!(
            unit_name("t3code-nightly", "2a", "service"),
            "app-nursearch-t3code\\x2dnightly-2a.service"
        );
        assert_eq!(
            unit_name(".hidden app", "1", "scope"),
            "app-nursearch-\\x2ehidden\\x20app-1.scope"
        );
    }
}
