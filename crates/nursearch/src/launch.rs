use crate::desktop::DesktopEntry;
use gtk4::gio;
use gtk4::gio::prelude::*;
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

        return match info.launch(&[], gio::AppLaunchContext::NONE) {
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
    spawn_app(&args)
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
fn spawn_app(args: &[String]) -> io::Result<()> {
    if Path::new("/run/systemd/system").exists() {
        match run_via_systemd(args) {
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
fn run_via_systemd(args: &[String]) -> io::Result<()> {
    let status = Command::new("systemd-run")
        .args(["--user", "--collect", "--quiet", "--"])
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

/// Spawn a detached command, discarding its standard streams. Shared by the
/// desktop-entry Exec fallback and by built-in system actions.
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
        Ok(child) => {
            info!("command spawned: pid={}, program={program:?}", child.id());
            Ok(())
        }
        Err(err) => {
            error!("command failed: program={program:?}, args={rest:?}, error={err}");
            Err(err)
        }
    }
}
