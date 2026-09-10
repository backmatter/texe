//! Subprocess construction shared by the CLI and its desktop clients.

use std::ffi::OsStr;
use std::process::Command;

/// Keep background tools from opening console windows when launched by the app.
pub(crate) fn command(program: impl AsRef<OsStr>) -> Command {
    let command = Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        let mut command = command;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
        command
    }
    #[cfg(not(windows))]
    command
}
