//! Loopback-only PDF.js viewer for `texe watch --view`.

use std::fs;
use std::io::{Cursor, Read as _, Write as _};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use crate::TexeError;

const PDFJS_ARCHIVE: &[u8] = include_bytes!("../assets/pdfjs/pdfjs-5.7.284-dist.zip");

const TEXE_BRIDGE_JS: &str = r#"
const POLL_INTERVAL_MS = 700;
const RELOAD_TIMEOUT_MS = 15_000;

let generation;
let reloadInProgress = false;

function nextFrame() {
  return new Promise(resolve => requestAnimationFrame(resolve));
}

function waitForEvent(eventBus, name, timeoutMs) {
  return new Promise(resolve => {
    const timeout = setTimeout(() => resolve(false), timeoutMs);
    eventBus.on(name, () => {
      clearTimeout(timeout);
      resolve(true);
    }, { once: true });
  });
}

function captureView(pdfViewer) {
  return {
    pageNumber: pdfViewer.currentPageNumber,
    scaleValue: pdfViewer.currentScaleValue,
    scrollLeft: pdfViewer.container.scrollLeft,
    scrollTop: pdfViewer.container.scrollTop,
    pagesRotation: pdfViewer.pagesRotation,
    scrollMode: pdfViewer.scrollMode,
    spreadMode: pdfViewer.spreadMode,
  };
}

async function restoreView(pdfViewer, view) {
  pdfViewer.pagesRotation = view.pagesRotation;
  pdfViewer.scrollMode = view.scrollMode;
  pdfViewer.spreadMode = view.spreadMode;
  pdfViewer.currentScaleValue = view.scaleValue;
  pdfViewer.currentPageNumber = Math.min(view.pageNumber, pdfViewer.pagesCount);

  // PDF.js updates scale and page layout asynchronously. Restore the exact
  // viewport only after those layout updates have settled.
  await nextFrame();
  await nextFrame();
  pdfViewer.container.scrollTo(view.scrollLeft, view.scrollTop);
}

async function reloadPdf(app, nextGeneration) {
  const view = captureView(app.pdfViewer);
  const pagesLoaded = waitForEvent(
    app.eventBus,
    "pagesloaded",
    RELOAD_TIMEOUT_MS,
  );

  await app.open({ url: `/paper.pdf?v=${nextGeneration}` });
  await pagesLoaded;
  await restoreView(app.pdfViewer, view);
}

async function readGeneration() {
  const response = await fetch("/status", { cache: "no-store" });
  if (!response.ok) {
    throw new Error(`viewer status returned ${response.status}`);
  }
  return Number((await response.json()).generation);
}

async function poll(app) {
  if (reloadInProgress) {
    return;
  }

  try {
    const nextGeneration = await readGeneration();
    if (generation === undefined) {
      generation = nextGeneration;
      document.documentElement.dataset.texeGeneration = String(generation);
      return;
    }
    if (nextGeneration === 0 || nextGeneration === generation) {
      return;
    }

    reloadInProgress = true;
    await reloadPdf(app, nextGeneration);
    generation = nextGeneration;
    document.documentElement.dataset.texeGeneration = String(generation);
  } catch (error) {
    console.warn("texe could not refresh the PDF; it will retry", error);
  } finally {
    reloadInProgress = false;
  }
}

async function start() {
  const app = window.PDFViewerApplication;
  await app.initializedPromise;
  await poll(app);
  setInterval(() => poll(app), POLL_INTERVAL_MS);
}

if (window.PDFViewerApplication) {
  start();
} else {
  document.addEventListener("webviewerloaded", start, { once: true });
}
"#;

struct Shared {
    pdf: Mutex<PathBuf>,
    generation: AtomicU64,
    stop: AtomicBool,
}

pub(crate) struct Viewer {
    address: SocketAddr,
    shared: Arc<Shared>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Viewer {
    pub(crate) fn start(pdf: &Path) -> Result<Self, TexeError> {
        let listener =
            TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).map_err(|source| TexeError::Io {
                path: PathBuf::from("127.0.0.1"),
                source,
            })?;
        let address = listener.local_addr().map_err(|source| TexeError::Io {
            path: PathBuf::from("127.0.0.1"),
            source,
        })?;
        listener
            .set_nonblocking(true)
            .map_err(|source| TexeError::Io {
                path: PathBuf::from(address.to_string()),
                source,
            })?;
        let shared = Arc::new(Shared {
            pdf: Mutex::new(pdf.to_path_buf()),
            generation: AtomicU64::new(u64::from(pdf.is_file())),
            stop: AtomicBool::new(false),
        });
        let server = Arc::clone(&shared);
        let thread = thread::spawn(move || serve(&listener, &server));
        Ok(Self {
            address,
            shared,
            thread: Some(thread),
        })
    }

    pub(crate) fn url(&self) -> String {
        format!("http://{}/web/viewer.html?file=%2Fpaper.pdf", self.address)
    }

    pub(crate) fn notify_success(&self) {
        self.shared.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub(crate) fn set_pdf(&self, pdf: &Path) {
        *self
            .shared
            .pdf
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = pdf.to_path_buf();
    }

    pub(crate) fn open_browser(&self) -> Result<bool, TexeError> {
        let url = self.url();
        let mut command = if cfg!(target_os = "macos") {
            let mut command = Command::new("open");
            command.arg(&url);
            command
        } else if cfg!(target_os = "windows") {
            let mut command = Command::new("cmd");
            command.args(["/C", "start", "", &url]);
            command
        } else {
            let mut command = Command::new("xdg-open");
            command.arg(&url);
            command
        };
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        match command.spawn() {
            Ok(_) => Ok(true),
            Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(source) => Err(TexeError::Spawn {
                tool: PathBuf::from(if cfg!(target_os = "macos") {
                    "open"
                } else if cfg!(target_os = "windows") {
                    "cmd"
                } else {
                    "xdg-open"
                }),
                source,
            }),
        }
    }
}

impl Drop for Viewer {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn serve(listener: &TcpListener, shared: &Shared) {
    while !shared.stop.load(Ordering::SeqCst) {
        match listener.accept() {
            Ok((mut stream, peer)) if peer.ip().is_loopback() => {
                let _ = respond(&mut stream, shared);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(30));
            }
            Err(_) => return,
        }
    }
}

fn respond(stream: &mut TcpStream, shared: &Shared) -> std::io::Result<()> {
    stream.set_nonblocking(false)?;
    stream.set_read_timeout(Some(Duration::from_secs(2)))?;
    stream.set_write_timeout(Some(Duration::from_secs(2)))?;
    let mut request = [0_u8; 8192];
    let mut length = 0;
    while length < request.len() {
        let read = stream.read(&mut request[length..])?;
        if read == 0 {
            break;
        }
        length += read;
        if request[..length]
            .windows(4)
            .any(|window| window == b"\r\n\r\n")
        {
            break;
        }
    }
    let line = String::from_utf8_lossy(&request[..length])
        .lines()
        .next()
        .unwrap_or_default()
        .to_string();
    let path = line
        .strip_prefix("GET ")
        .and_then(|line| line.split_whitespace().next())
        .and_then(|path| path.split('?').next());

    match path {
        Some("/") => send_redirect(stream, "/web/viewer.html?file=%2Fpaper.pdf"),
        Some("/web/viewer.html") => {
            let html = bundled_pdfjs_asset("web/viewer.html")?
                .ok_or_else(|| std::io::Error::other("PDF.js viewer.html is missing"))?;
            let html = String::from_utf8(html)
                .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
            let html = html
                .replace(
                    "<title>PDF.js viewer</title>",
                    "<link rel=\"icon\" href=\"data:,\" />\n    <title>texe PDF viewer</title>",
                )
                .replace(
                    "  </body>",
                    "    <script src=\"/texe-bridge.js\" type=\"module\"></script>\n  </body>",
                );
            send(
                stream,
                "200 OK",
                "text/html; charset=utf-8",
                html.as_bytes(),
            )
        }
        Some("/texe-bridge.js") => send(
            stream,
            "200 OK",
            "text/javascript; charset=utf-8",
            TEXE_BRIDGE_JS.as_bytes(),
        ),
        Some("/status") => {
            let body = format!(
                "{{\"schema\":\"texe.viewer-status/v1\",\"generation\":{}}}",
                shared.generation.load(Ordering::SeqCst)
            );
            send(
                stream,
                "200 OK",
                "application/json; charset=utf-8",
                body.as_bytes(),
            )
        }
        Some("/paper.pdf") => {
            let path = shared
                .pdf
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !path.is_file() {
                return not_found(stream);
            }
            let pdf = fs::read(path)?;
            send(stream, "200 OK", "application/pdf", &pdf)
        }
        Some("/pdfjs-license.txt") => {
            let license = bundled_pdfjs_asset("LICENSE")?
                .ok_or_else(|| std::io::Error::other("PDF.js license is missing"))?;
            send(stream, "200 OK", "text/plain; charset=utf-8", &license)
        }
        Some(path) => match pdfjs_asset_name(path) {
            Some(name) => match bundled_pdfjs_asset(&name)? {
                Some(asset) => send(stream, "200 OK", content_type(&name), &asset),
                None => not_found(stream),
            },
            None => not_found(stream),
        },
        None => not_found(stream),
    }
}

fn pdfjs_asset_name(path: &str) -> Option<String> {
    if path.contains('\\')
        || path
            .split('/')
            .any(|component| component == "." || component == "..")
    {
        return None;
    }

    let exact = [
        "/web/viewer.css",
        "/web/viewer.mjs",
        "/web/debugger.css",
        "/web/debugger.mjs",
        "/build/pdf.mjs",
        "/build/pdf.worker.mjs",
        "/build/pdf.sandbox.mjs",
    ];
    let prefixes = [
        "/web/images/",
        "/web/locale/",
        "/web/cmaps/",
        "/web/standard_fonts/",
        "/web/wasm/",
        "/web/iccs/",
    ];
    if exact.contains(&path) || prefixes.iter().any(|prefix| path.starts_with(prefix)) {
        return Some(path.trim_start_matches('/').to_string());
    }
    None
}

fn bundled_pdfjs_asset(name: &str) -> std::io::Result<Option<Vec<u8>>> {
    let cursor = Cursor::new(PDFJS_ARCHIVE);
    let mut archive = zip::ZipArchive::new(cursor).map_err(std::io::Error::other)?;
    let mut file = match archive.by_name(name) {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(error) => return Err(std::io::Error::other(error)),
    };
    let mut body = Vec::with_capacity(usize::try_from(file.size()).unwrap_or_default());
    file.read_to_end(&mut body)?;
    Ok(Some(body))
}

fn content_type(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|extension| extension.to_str())
    {
        Some("css") => "text/css; charset=utf-8",
        Some("js" | "mjs") => "text/javascript; charset=utf-8",
        Some("json") => "application/json; charset=utf-8",
        Some("ftl" | "properties") => "text/plain; charset=utf-8",
        Some("svg") => "image/svg+xml",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("ttf") => "font/ttf",
        Some("otf") => "font/otf",
        Some("woff") => "font/woff",
        Some("woff2") => "font/woff2",
        Some("wasm") => "application/wasm",
        Some("icc") => "application/vnd.iccprofile",
        _ => "application/octet-stream",
    }
}

fn send_redirect(stream: &mut TcpStream, location: &str) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\
         Connection: close\r\n\r\n"
    )
}

fn not_found(stream: &mut TcpStream) -> std::io::Result<()> {
    send(
        stream,
        "404 Not Found",
        "text/plain; charset=utf-8",
        b"not found",
    )
}

fn send(
    stream: &mut TcpStream,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()> {
    write!(
        stream,
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nContent-Security-Policy: default-src 'self'; \
         connect-src 'self'; img-src 'self' data: blob:; font-src 'self' data:; \
         object-src 'none'; frame-src 'self' blob:; worker-src 'self' blob:; \
         script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; \
         frame-ancestors 'none'\r\n\
         Referrer-Policy: no-referrer\r\n\
         X-Content-Type-Options: nosniff\r\n\
         X-Frame-Options: DENY\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )?;
    stream.write_all(body)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::io::{Read as _, Write as _};
    use std::net::{Ipv4Addr, SocketAddr, TcpStream};
    use std::thread;
    use std::time::Duration;

    use crate::viewer::{PDFJS_ARCHIVE, Viewer, bundled_pdfjs_asset};
    use sha2::{Digest as _, Sha256};
    use std::net::IpAddr;

    fn transient_socket_error(error: &std::io::Error) -> bool {
        matches!(
            error.kind(),
            std::io::ErrorKind::BrokenPipe
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionReset
        )
    }

    fn get(address: SocketAddr, path: &str) -> Vec<u8> {
        for _ in 0..5 {
            let mut stream = TcpStream::connect(address).expect("connect");
            if let Err(error) = write!(
                stream,
                "GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"
            ) {
                if transient_socket_error(&error) {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                panic!("request: {error}");
            }
            let mut response = Vec::new();
            match stream.read_to_end(&mut response) {
                Ok(_) => return response,
                Err(error) if transient_socket_error(&error) => {
                    thread::sleep(Duration::from_millis(20));
                }
                Err(error) => panic!("response: {error}"),
            }
        }
        panic!("viewer repeatedly reset the test connection")
    }

    fn response_text(address: SocketAddr, path: &str) -> String {
        String::from_utf8(get(address, path)).expect("text response")
    }

    #[test]
    fn pinned_pdfjs_distribution_has_the_expected_digest_and_license() {
        assert_eq!(
            hex::encode(Sha256::digest(PDFJS_ARCHIVE)),
            "6d1b81252d76358df5831567d7d551f40ebae0cd8e0a554694bc4df0d3db8715"
        );
        let license = bundled_pdfjs_asset("LICENSE")
            .expect("archive")
            .expect("license");
        assert!(String::from_utf8_lossy(&license).contains("Apache License"));
        assert!(
            bundled_pdfjs_asset("web/viewer.mjs")
                .expect("archive")
                .is_some()
        );
    }

    #[test]
    fn viewer_is_loopback_only_and_serves_no_project_source() {
        let directory = tempfile::tempdir().expect("project");
        fs::write(directory.path().join("main.pdf"), b"%PDF-test").expect("pdf");
        fs::write(directory.path().join("main.tex"), b"private source").expect("source");
        let viewer = Viewer::start(&directory.path().join("main.pdf")).expect("viewer");
        assert_eq!(viewer.address.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));

        let root = response_text(viewer.address, "/");
        assert!(root.starts_with("HTTP/1.1 302 Found"));
        assert!(root.contains("Location: /web/viewer.html?file=%2Fpaper.pdf"));
        assert!(response_text(viewer.address, "/paper.pdf").contains("%PDF-test"));

        let source = response_text(viewer.address, "/main.tex");
        assert!(source.starts_with("HTTP/1.1 404 Not Found"));
        assert!(!source.contains("private source"));
        assert!(
            response_text(viewer.address, "/web/../LICENSE").starts_with("HTTP/1.1 404 Not Found")
        );
        assert!(
            response_text(viewer.address, "/build/pdf.mjs.map")
                .starts_with("HTTP/1.1 404 Not Found")
        );
    }

    #[test]
    fn viewer_waits_for_a_complete_http_request() {
        let directory = tempfile::tempdir().expect("project");
        let viewer = Viewer::start(&directory.path().join("main.pdf")).expect("viewer");
        let mut stream = TcpStream::connect(viewer.address).expect("connect");
        stream.write_all(b"GE").expect("partial request");
        thread::sleep(Duration::from_millis(80));
        stream
            .write_all(b"T / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n")
            .expect("complete request");
        let mut bytes = Vec::new();
        if let Err(error) = stream.read_to_end(&mut bytes) {
            assert!(
                transient_socket_error(&error) && !bytes.is_empty(),
                "response: {error}"
            );
        }
        let response = String::from_utf8(bytes).expect("text response");
        assert!(response.starts_with("HTTP/1.1 302 Found"));
    }

    #[test]
    fn official_viewer_and_texe_state_bridge_are_served_locally() {
        let directory = tempfile::tempdir().expect("project");
        let viewer = Viewer::start(&directory.path().join("main.pdf")).expect("viewer");

        let html = response_text(viewer.address, "/web/viewer.html?file=%2Fpaper.pdf");
        assert!(html.starts_with("HTTP/1.1 200 OK"));
        assert!(html.contains("<title>texe PDF viewer</title>"));
        assert!(html.contains("rel=\"icon\" href=\"data:,\""));
        assert!(html.contains("id=\"zoomInButton\""));
        assert!(html.contains("id=\"findbar\""));
        assert!(html.contains("src=\"/texe-bridge.js\""));
        assert!(!html.contains("src=\"http"));
        assert!(html.contains("href=\"viewer.css\""));
        assert!(!html.contains("rel=\"stylesheet\" href=\"http"));

        let viewer_module = response_text(viewer.address, "/web/viewer.mjs");
        assert!(viewer_module.starts_with("HTTP/1.1 200 OK"));
        let worker = response_text(viewer.address, "/build/pdf.worker.mjs");
        assert!(worker.starts_with("HTTP/1.1 200 OK"));
        let locale = response_text(viewer.address, "/web/locale/locale.json");
        assert!(locale.starts_with("HTTP/1.1 200 OK"));

        let bridge = response_text(viewer.address, "/texe-bridge.js");
        assert!(bridge.contains("captureView"));
        assert!(bridge.contains("restoreView"));
        assert!(bridge.contains("currentPageNumber"));
        assert!(bridge.contains("currentScaleValue"));
        assert!(bridge.contains("scrollTop"));
    }

    #[test]
    fn successful_build_notification_advances_the_generation() {
        let directory = tempfile::tempdir().expect("project");
        let viewer = Viewer::start(&directory.path().join("main.pdf")).expect("viewer");
        let before = get(viewer.address, "/status");
        viewer.notify_success();
        let after = get(viewer.address, "/status");
        assert_ne!(before, after);
    }

    #[test]
    fn viewer_can_follow_a_changed_manifest_entry() {
        let directory = tempfile::tempdir().expect("project");
        let first = directory.path().join("main.pdf");
        let second = directory.path().join("revised.pdf");
        fs::write(&first, b"%PDF-first").expect("first PDF");
        fs::write(&second, b"%PDF-second").expect("second PDF");
        let viewer = Viewer::start(&first).expect("viewer");

        assert!(response_text(viewer.address, "/paper.pdf").contains("%PDF-first"));
        viewer.set_pdf(&second);
        viewer.notify_success();
        assert!(response_text(viewer.address, "/paper.pdf").contains("%PDF-second"));
    }

    #[test]
    fn dropping_the_viewer_closes_its_listening_port() {
        let directory = tempfile::tempdir().expect("project");
        let viewer = Viewer::start(&directory.path().join("main.pdf")).expect("viewer");
        let address = viewer.address;
        drop(viewer);
        assert!(TcpStream::connect_timeout(&address, Duration::from_millis(200)).is_err());
    }
}
