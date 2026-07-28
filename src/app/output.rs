use std::io::Write as _;
use std::path::PathBuf;

use serde::Serialize;

use crate::TexeError;

pub(crate) fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    if bytes < 1024_u64.pow(2) {
        return format!("{} KiB", (bytes + 512) / 1024);
    }
    if bytes < 1024_u64.pow(3) {
        let divisor = 1024_u64.pow(2);
        return format!("{} MiB", (bytes + divisor / 2) / divisor);
    }
    let divisor = 1024_u64.pow(3);
    let rounded_tenths = (u128::from(bytes) * 10 + u128::from(divisor / 2)) / u128::from(divisor);
    format!("{}.{} GiB", rounded_tenths / 10, rounded_tenths % 10)
}

pub(crate) fn human_count(count: usize, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

pub(super) fn print_json<T: Serialize>(report: &T) -> Result<(), TexeError> {
    println!(
        "{}",
        serde_json::to_string_pretty(report).map_err(|source| {
            TexeError::Json {
                path: PathBuf::from("<stdout>"),
                source,
            }
        })?
    );
    Ok(())
}

pub(super) fn print_json_line<T: Serialize>(event: &T) -> Result<(), TexeError> {
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    serde_json::to_writer(&mut stdout, event).map_err(|source| TexeError::Json {
        path: PathBuf::from("<stdout>"),
        source,
    })?;
    stdout
        .write_all(b"\n")
        .and_then(|()| stdout.flush())
        .map_err(|source| TexeError::Io {
            path: PathBuf::from("<stdout>"),
            source,
        })
}
