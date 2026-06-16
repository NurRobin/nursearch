use crate::desktop::DesktopEntry;
use gtk4::gio;
use gtk4::gio::prelude::*;
use log::{debug, error, info};
use std::io;
use std::os::unix::process::CommandExt;
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

    let Some((program, rest)) = args.split_first() else {
        error!("desktop entry has empty Exec command: {}", app.name);
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "desktop entry has an empty Exec command",
        ));
    };

    info!(
        "using Exec launcher: name={}, desktop_file={}, program={:?}",
        app.name,
        app.path.display(),
        program
    );
    debug!(
        "spawning fallback command: program={:?}, args={:?}",
        program, rest
    );
    let mut command = Command::new(program);
    command
        .args(rest)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0);

    match command.spawn() {
        Ok(child) => {
            info!(
                "fallback command spawned: name={}, pid={}, program={:?}",
                app.name,
                child.id(),
                program
            );
            Ok(())
        }
        Err(err) => {
            error!(
                "fallback command failed: name={}, program={:?}, args={:?}, error={err}",
                app.name, program, rest
            );
            Err(err)
        }
    }
}
