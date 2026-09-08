//! Local handoff from LaTeX Workshop to the companion's build queue.
use std::io::{BufRead as _, BufReader, Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::Duration;

use crate::TexeError;

#[derive(serde::Deserialize)]
struct Endpoint {
    port: u16,
    token: String,
}

pub(super) fn build(project: Option<&Path>) -> Result<(), TexeError> {
    let context = super::project::load_project(project)?;
    let path = context.root.join(".texe/editor/build-bridge.json");
    let unavailable = || {
        TexeError::Build("open this paper folder in trusted VS Code with the texe companion enabled, then retry Build; use `texe build` for a terminal build".into())
    };
    let bytes = std::fs::read(&path).map_err(|_| unavailable())?;
    if bytes.len() > 4096 {
        return Err(unavailable());
    }
    let endpoint: Endpoint = serde_json::from_slice(&bytes).map_err(|_| unavailable())?;
    if endpoint.token.len() != 64 || !endpoint.token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(unavailable());
    }
    let mut stream = TcpStream::connect_timeout(
        &SocketAddr::from((Ipv4Addr::LOCALHOST, endpoint.port)),
        Duration::from_secs(2),
    )
    .map_err(|_| unavailable())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .map_err(|_| unavailable())?;
    writeln!(stream, "{}", serde_json::json!({"token":endpoint.token}))
        .map_err(|_| unavailable())?;
    let mut response = String::new();
    BufReader::new(stream)
        .take(4097)
        .read_line(&mut response)
        .map_err(|_| unavailable())?;
    if response.len() > 4096 {
        return Err(unavailable());
    }
    let response: serde_json::Value = serde_json::from_str(&response).map_err(|_| unavailable())?;
    if response["ok"] == true {
        Ok(())
    } else if response["cancelled"] == true {
        Err(TexeError::Prompt("cancelled".into()))
    } else {
        Err(TexeError::Build(
            response["message"]
                .as_str()
                .unwrap_or("build failed; see texe Problems and Output in VS Code")
                .into(),
        ))
    }
}
