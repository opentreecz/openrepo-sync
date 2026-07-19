//! Minimal blocking HTTP server for exercising RepoClient and sync logic
//! against canned responses, without any extra dev-dependencies.

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

/// One canned reply: status code, body bytes, and extra response headers.
pub struct MockResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

impl MockResponse {
    pub fn json(status: u16, body: &str) -> Self {
        Self {
            status,
            body: body.as_bytes().to_vec(),
            headers: vec![("Content-Type".to_string(), "application/json".to_string())],
        }
    }

    pub fn bytes(status: u16, body: Vec<u8>, headers: &[(&str, &str)]) -> Self {
        Self {
            status,
            body,
            headers: headers
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
}

/// Serves the given responses to sequential connections, recording each
/// request's head (request line + headers) for later assertions.
pub struct MockServer {
    pub url: String,
    requests: Arc<Mutex<Vec<String>>>,
    handle: Option<JoinHandle<()>>,
}

impl MockServer {
    pub fn start(responses: Vec<MockResponse>) -> Self {
        Self::start_with(|_| responses)
    }

    /// Like `start`, but the response set may reference the server's own URL
    /// (e.g. for pagination "next" links).
    pub fn start_with(responses: impl FnOnce(&str) -> Vec<MockResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock server");
        let addr = listener.local_addr().expect("mock server addr");
        let url = format!("http://{}", addr);
        let responses = responses(&url);
        let requests: Arc<Mutex<Vec<String>>> = Arc::default();
        let recorded = Arc::clone(&requests);

        let handle = std::thread::spawn(move || {
            for resp in responses {
                let Ok((mut stream, _)) = listener.accept() else {
                    return;
                };
                let raw = read_request(&mut stream);
                recorded
                    .lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&raw).into_owned());
                let mut head = format!(
                    "HTTP/1.1 {} MOCK\r\nContent-Length: {}\r\nConnection: close\r\n",
                    resp.status,
                    resp.body.len(),
                );
                for (name, value) in &resp.headers {
                    head.push_str(&format!("{}: {}\r\n", name, value));
                }
                head.push_str("\r\n");
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(&resp.body);
            }
        });

        Self {
            url,
            requests,
            handle: Some(handle),
        }
    }

    /// Request heads (request line + headers + body) in arrival order.
    /// Joins the server thread, so call only after all requests were made.
    pub fn requests(mut self) -> Vec<String> {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        let requests = self.requests.lock().unwrap();
        requests.clone()
    }
}

/// Read one HTTP request: headers, then as many body bytes as Content-Length
/// declares (uploads are multipart with a known length).
fn read_request(stream: &mut std::net::TcpStream) -> Vec<u8> {
    let mut data = Vec::new();
    let mut buf = [0u8; 16384];
    while let Ok(n) = stream.read(&mut buf) {
        if n == 0 {
            break;
        }
        data.extend_from_slice(&buf[..n]);
        if let Some(header_end) = find_subsequence(&data, b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&data[..header_end]).into_owned();
            let content_length = head
                .lines()
                .find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .map(String::from)
                })
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(0);
            let body_received = data.len() - (header_end + 4);
            if body_received >= content_length {
                break;
            }
        }
    }
    data
}

fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// True when dpkg-deb can run on this machine (present on Debian/Ubuntu,
/// including the CI runners; tests that need it skip themselves otherwise).
pub fn dpkg_deb_available() -> bool {
    std::process::Command::new("dpkg-deb")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a minimal valid .deb with the given version and return its path.
pub fn build_minimal_deb(dir: &std::path::Path, version: &str) -> std::path::PathBuf {
    let pkg_root = dir.join("pkgroot");
    std::fs::create_dir_all(pkg_root.join("DEBIAN")).unwrap();
    std::fs::write(
        pkg_root.join("DEBIAN/control"),
        format!(
            "Package: testpkg\nVersion: {}\nArchitecture: all\nMaintainer: test <t@example.com>\nDescription: test package\n",
            version
        ),
    )
    .unwrap();
    let deb_path = dir.join("testpkg.deb");
    let status = std::process::Command::new("dpkg-deb")
        .args([
            "--root-owner-group",
            "--build",
            &pkg_root.to_string_lossy(),
            &deb_path.to_string_lossy(),
        ])
        .status()
        .unwrap();
    assert!(status.success(), "dpkg-deb --build failed");
    deb_path
}
