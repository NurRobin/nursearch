use crate::desktop::DesktopEntry;
use gtk4::gio;
use gtk4::gio::prelude::*;
use std::io;
use std::process::Command;

pub fn launch(app: &DesktopEntry) -> io::Result<()> {
    if let Some(info) = gio::DesktopAppInfo::from_filename(&app.path) {
        return info
            .launch(&[], gio::AppLaunchContext::NONE)
            .map_err(|err| io::Error::other(err.to_string()));
    }

    if app.dbus_activatable && app.exec.is_none() {
        return Err(io::Error::other(
            "D-Bus activatable desktop entry could not be loaded by GIO",
        ));
    }

    if app.terminal {
        return Err(io::Error::other(
            "terminal desktop entries require GIO launch support",
        ));
    }

    let args = app.exec_args()?;

    let Some((program, rest)) = args.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "desktop entry has an empty Exec command",
        ));
    };

    Command::new(program).args(rest).spawn().map(|_| ())
}
