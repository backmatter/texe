//! Native target selection for embedded managed-toolchain recipes.

use crate::TexeError;

pub(super) fn current_target() -> Result<&'static str, TexeError> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => Ok("x86_64-linux"),
        ("windows", "x86_64") => Ok("windows"),
        ("macos", "aarch64") => Ok("universal-darwin"),
        (os, arch) => Err(TexeError::Toolchain(format!(
            "managed toolchains support Linux x86-64, Windows x86-64, and macOS Apple Silicon; \
             this binary is {os}-{arch}"
        ))),
    }
}
