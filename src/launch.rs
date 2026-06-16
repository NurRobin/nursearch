use crate::desktop::DesktopEntry;
use std::io;
use std::process::Command;

pub fn launch(app: &DesktopEntry) -> io::Result<()> {
    let args = shell_words::split(&app.exec)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err.to_string()))?;

    let Some((program, rest)) = args.split_first() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "desktop entry has an empty Exec command",
        ));
    };

    Command::new(program).args(rest).spawn().map(|_| ())
}
