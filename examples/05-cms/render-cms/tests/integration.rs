//! End-to-end integration test for the example-05 CMS blog site.
//!
//! Spins up the full server stack (m6-html, m6-file, m6-auth-server,
//! render-cms, m6-http) in child processes, exercises every public page,
//! the contact form, auth, all CMS API endpoints, and the full
//! draft → publish → unpublish lifecycle, then tears everything down.
//!
//! # Prerequisites
//!
//! The m6 release binaries must be pre-built:
//!   cargo build --release   (in the m6/ workspace)
//!
//! If any required binary, certificate or socket is absent the test fails and
//! names the command that fixes it.
//!
//! The site's auth database must exist with an `admin` user in the `editors`
//! group (created by `./dev.sh` or `./setup.sh`). The test fails with
//! "auth database not found. Run ./dev.sh first" if it is absent.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use rustls::pki_types::ServerName;

// ── locate binaries ───────────────────────────────────────────────────────────

fn m6_root() -> PathBuf {
    // render-cms/  →  05-cms/  →  examples/  →  m6-examples/  →  m6/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../m6")
        .canonicalize()
        .expect("m6 workspace root")
}

fn m6_bin(name: &str) -> Option<PathBuf> {
    let release = m6_root().join("target/release").join(name);
    if release.exists() { return Some(release); }
    // Fall back to PATH
    which(name)
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).find_map(|dir| {
            let full = dir.join(name);
            if full.exists() { Some(full) } else { None }
        }))
}

fn render_cms_bin() -> PathBuf {
    // Cargo sets this env var to the path of the compiled binary.
    PathBuf::from(env!("CARGO_BIN_EXE_render-cms"))
}

fn site_dir() -> PathBuf {
    // render-cms/  →  05-cms/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("site dir")
}

/// Whether the auth database exists.
///
/// `m6-auth-server` resolves `[storage] path` against the config file's
/// directory, so this example's database is `configs/data/auth.db`. `data/` is
/// where an earlier layout expected it, and `dev.sh` checks both.
fn auth_db_exists() -> bool {
    let site = site_dir();
    site.join("configs/data/auth.db").exists() || site.join("data/auth.db").exists()
}

// ── TLS: no-verify client config ─────────────────────────────────────────────

#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self, _msg: &[u8], _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(
        &self, _msg: &[u8], _cert: &rustls::pki_types::CertificateDer<'_>,
        _dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}

fn make_tls_config() -> Arc<rustls::ClientConfig> {
    rustls::crypto::ring::default_provider().install_default().ok();
    Arc::new(
        rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify))
            .with_no_client_auth(),
    )
}

// ── HTTP/1.1 over TLS helpers ─────────────────────────────────────────────────

struct Response {
    status: u16,
    headers: Vec<(String, String)>,
    body:   String,
}

impl Response {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }
    fn set_cookies(&self) -> Vec<String> {
        self.headers.iter()
            .filter(|(k, _)| k.eq_ignore_ascii_case("set-cookie"))
            .map(|(_, v)| v.clone())
            .collect()
    }
}

fn https_request(
    addr:    &str,
    method:  &str,
    path:    &str,
    extra_headers: &[(&str, &str)],
    body:    &[u8],
    tls_cfg: Arc<rustls::ClientConfig>,
) -> Result<Response, String> {
    let tcp = TcpStream::connect(addr)
        .map_err(|e| format!("connect {addr}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();

    let server_name = ServerName::try_from("localhost".to_string())
        .map_err(|e| format!("server name: {e}"))?;
    let conn = rustls::ClientConnection::new(tls_cfg, server_name)
        .map_err(|e| format!("TLS init: {e}"))?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);

    // Build request
    let mut req = format!("{method} {path} HTTP/1.1\r\nHost: localhost\r\n");
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    if !body.is_empty() {
        req.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    req.push_str("Connection: close\r\n\r\n");

    stream.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    if !body.is_empty() {
        stream.write_all(body).map_err(|e| format!("write body: {e}"))?;
    }
    stream.flush().map_err(|e| format!("flush: {e}"))?;

    // Read response
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok(); // EOF is normal with Connection: close

    Ok(parse_response(&String::from_utf8_lossy(&raw)))
}

/// The status code from a header block's first line, or 0 if there isn't one.
fn status_of(hdr_sec: &str) -> u16 {
    hdr_sec
        .split("\r\n")
        .next()
        .unwrap_or("")
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

/// Parse a raw HTTP/1.1 response, discarding any 1xx that precedes the real one.
///
/// RFC 9110 15.2 requires a client to parse and discard any number of 1xx
/// responses before the final one. `m6-http` sends `103 Early Hints` ahead of
/// the real response on any route carrying preload hints, and this parser took
/// the FIRST `\r\n\r\n` and the FIRST status line, so it read 103 and handed the
/// actual response back as the body:
///
/// ```text
/// FAIL  GET / reachable — expected 200, got 103
/// ```
///
/// The same defect was in `09-global-deployment`'s client. Both are fixed the
/// same way; neither is a server regression. They began failing when early hints
/// started firing, which is m6 #74/#78 landing. m6 issue #86.
fn parse_response(s: &str) -> Response {
    let mut cursor = 0usize;
    let (hdr_sec, body) = loop {
        let rest = &s[cursor..];
        let split = rest.find("\r\n\r\n").unwrap_or(rest.len());
        let after = cursor + if split + 4 <= rest.len() { split + 4 } else { rest.len() };

        // A 1xx carries no body and is always followed by another response, so
        // step over it. `after < s.len()` stops this looping on a truncated
        // response that ends with an informational one.
        if (100..200).contains(&status_of(&rest[..split])) && after < s.len() {
            cursor = after;
            continue;
        }
        break (&rest[..split], &s[after..]);
    };

    let mut headers = Vec::new();
    for line in hdr_sec.split("\r\n").skip(1) {
        if let Some(colon) = line.find(':') {
            headers.push((line[..colon].trim().to_string(), line[colon+1..].trim().to_string()));
        }
    }

    Response { status: status_of(hdr_sec), headers, body: body.to_string() }
}

#[cfg(test)]
mod response_parsing {
    use super::parse_response;

    #[test]
    fn a_plain_response_is_unchanged() {
        let r = parse_response("HTTP/1.1 200 OK\r\ncontent-type: text/html\r\n\r\nhello");
        assert_eq!(r.status, 200);
        assert_eq!(r.body, "hello");
    }

    #[test]
    fn early_hints_are_skipped_and_the_real_status_is_read() {
        let raw = "HTTP/1.1 103 Early Hints\r\n\
                   Link: </assets/css/style.css>; rel=preload; as=style\r\n\
                   \r\n\
                   HTTP/1.1 200 OK\r\n\
                   content-type: text/html\r\n\
                   \r\n\
                   <!doctype html>";
        let r = parse_response(raw);
        assert_eq!(r.status, 200, "must report the FINAL status, not the 103");
        assert_eq!(r.body, "<!doctype html>");
    }

    #[test]
    fn set_cookie_on_the_final_response_survives_the_skip() {
        // This suite logs in, so the header it cares about most must come from
        // the real response rather than being lost with the discarded 103.
        let raw = "HTTP/1.1 103 Early Hints\r\n\
                   Link: </a.css>; rel=preload\r\n\
                   \r\n\
                   HTTP/1.1 302 Found\r\n\
                   set-cookie: session=abc; HttpOnly\r\n\
                   location: /cms\r\n\
                   \r\n";
        let r = parse_response(raw);
        assert_eq!(r.status, 302);
        assert!(
            r.headers.iter().any(|(k, v)| k == "set-cookie" && v.contains("session=abc")),
            "headers: {:?}",
            r.headers
        );
    }

    #[test]
    fn a_truncated_informational_response_does_not_loop() {
        let r = parse_response("HTTP/1.1 103 Early Hints\r\nLink: </a.css>\r\n\r\n");
        assert_eq!(r.status, 103);
    }

    #[test]
    fn garbage_does_not_panic() {
        assert_eq!(parse_response("").status, 0);
        assert_eq!(parse_response("not http at all").status, 0);
    }
}

fn get_with_cookie_str(addr: &str, path: &str, cookie_str: &str, tls: Arc<rustls::ClientConfig>)
    -> Response
{
    let extra: &[(&str, &str)] = if cookie_str.is_empty() { &[] }
        else { &[("Cookie", cookie_str)] };
    https_request(addr, "GET", path, extra, &[], tls)
        .unwrap_or_else(|e| panic!("GET {path}: {e}"))
}

fn post(
    addr: &str, path: &str,
    content_type: &str, body: &str,
    cookie_str: &str,
    tls: Arc<rustls::ClientConfig>,
) -> Response {
    let mut extra = vec![("Content-Type", content_type)];
    if !cookie_str.is_empty() { extra.push(("Cookie", cookie_str)); }
    https_request(addr, "POST", path, &extra, body.as_bytes(), tls)
        .unwrap_or_else(|e| panic!("POST {path}: {e}"))
}

fn patch(
    addr: &str, path: &str,
    body: &str,
    cookie_str: &str,
    tls: Arc<rustls::ClientConfig>,
) -> Response {
    let mut extra = vec![("Content-Type", "application/json")];
    if !cookie_str.is_empty() { extra.push(("Cookie", cookie_str)); }
    https_request(addr, "PATCH", path, &extra, body.as_bytes(), tls)
        .unwrap_or_else(|e| panic!("PATCH {path}: {e}"))
}

fn build_cookie_header(jar: &HashMap<String, String>) -> String {
    jar.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
}

/// Extract all Set-Cookie name=value pairs from a response into a jar.
fn harvest_cookies(resp: &Response, jar: &mut HashMap<String, String>) {
    for sc in resp.set_cookies() {
        // Set-Cookie: name=value; Path=/; HttpOnly; ...
        if let Some(pair) = sc.split(';').next() {
            if let Some(eq) = pair.find('=') {
                let name  = pair[..eq].trim().to_string();
                let value = pair[eq+1..].trim().to_string();
                jar.insert(name, value);
            }
        }
    }
}

// ── process management ────────────────────────────────────────────────────────

struct ServerStack {
    addr:       String,
    _sock_dir:  tempfile::TempDir,   // kept alive so the temp dir is not deleted while running
    processes:  Vec<Child>,
}

impl Drop for ServerStack {
    fn drop(&mut self) {
        for c in &mut self.processes {
            c.kill().ok();
            c.wait().ok();
        }
    }
}

fn wait_for_tcp(addr: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if TcpStream::connect(addr).is_ok() { return true; }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_for_socket(path: &Path, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() { return true; }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

impl ServerStack {
    /// Start the full server stack. Panics, naming the command that fixes it,
    /// when a binary, certificate, auth database or socket is missing or a
    /// service does not come up. Children already started are killed on the
    /// way out, because they live in the returned stack's `Drop`.
    fn start() -> Self {
        let site = site_dir();

        // Require all m6 binaries.
        let m6_http   = m6_bin("m6-http").expect("m6-http not found in ../m6/target/release or PATH. Run `cargo build --release` in the m6 workspace");
        let m6_html   = m6_bin("m6-html").expect("m6-html not found in ../m6/target/release or PATH. Run `cargo build --release` in the m6 workspace");
        let m6_file   = m6_bin("m6-file").expect("m6-file not found in ../m6/target/release or PATH. Run `cargo build --release` in the m6 workspace");
        let m6_auth   = m6_bin("m6-auth-server").expect("m6-auth-server not found in ../m6/target/release or PATH. Run `cargo build --release` in the m6 workspace");
        let render_cms = render_cms_bin();
        assert!(
            render_cms.exists(),
            "render-cms binary not found at {}. Run `cargo build -p render-cms`",
            render_cms.display()
        );

        // Use a unique port per process ID to avoid collisions with dev server.
        let base_port: u16 = 20000 + (std::process::id() as u16 % 10000);
        let tls_addr = format!("127.0.0.1:{base_port}");

        let sock_dir = tempfile::tempdir().expect("cannot create a temporary directory. Check that the temp dir is writable");
        let html_sock = sock_dir.path().join("m6-html.sock");
        let file_sock = sock_dir.path().join("m6-file.sock");
        let auth_sock = sock_dir.path().join("m6-auth.sock");
        let cms_sock  = sock_dir.path().join("render-cms.sock");

        // Write a system.toml that points at the dev TLS keys.
        let tls_cert = site.join("keys/dev.pem");
        let tls_key  = site.join("keys/dev-key.pem");
        if !tls_cert.exists() || !tls_key.exists() {
            panic!("TLS certs not found at keys/dev.pem. Run ./dev.sh first");
        }

        let sys_toml = sock_dir.path().join("system.toml");
        std::fs::write(&sys_toml, format!(
            "[server]\nbind     = \"{tls_addr}\"\ntls_cert = \"{}\"\ntls_key  = \"{}\"\n\n\
             [auth]\nbackend    = \"m6-auth\"\npublic_key = \"keys/auth.pub\"\n",
            tls_cert.display(), tls_key.display(),
        )).expect("cannot write system.toml in the temporary directory. Check that the temp dir is writable");

        // Write a site.toml that points sockets at the temp sock_dir.
        let site_toml = sock_dir.path().join("site.toml");
        std::fs::copy(site.join("site.toml"), &site_toml)
            .expect("cannot copy site.toml. Check examples/05-cms/site.toml exists");
        // Patch socket glob paths in the copy.
        let original = std::fs::read_to_string(&site_toml)
            .expect("cannot read the copied site.toml. Check that the temp dir is writable");
        let patched = original
            .replace("/tmp/m6/m6-html*.sock",    &html_sock.display().to_string())
            .replace("/tmp/m6/m6-file*.sock",    &file_sock.display().to_string())
            .replace("/tmp/m6/m6-auth*.sock",    &auth_sock.display().to_string())
            .replace("/tmp/m6/render-cms*.sock", &cms_sock.display().to_string());
        std::fs::write(&site_toml, &patched)
            .expect("cannot write the patched site.toml. Check that the temp dir is writable");

        // The auth DB is created by dev.sh / setup.sh.
        if !auth_db_exists() {
            panic!("auth database not found. Run ./dev.sh first");
        }

        // The stack owns its children from the first spawn, so a panic below
        // kills what was already started instead of leaking it.
        let mut stack = ServerStack {
            addr: tls_addr.clone(),
            _sock_dir: sock_dir,
            processes: Vec::new(),
        };
        let sock_path = stack._sock_dir.path().to_path_buf();

        // m6-html
        stack.processes.push(Command::new(&m6_html)
            .arg(&site)
            .arg(site.join("configs/m6-html.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &html_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("cannot start m6-html. Run `cargo build --release` in the m6 workspace and `cargo build -p render-cms`"));

        // m6-file
        stack.processes.push(Command::new(&m6_file)
            .arg(&site)
            .arg(site.join("configs/m6-file.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &file_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("cannot start m6-file. Run `cargo build --release` in the m6 workspace and `cargo build -p render-cms`"));

        // m6-auth-server
        stack.processes.push(Command::new(&m6_auth)
            .arg(&site)
            .arg(site.join("configs/m6-auth.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &auth_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("cannot start m6-auth-server. Run `cargo build --release` in the m6 workspace and `cargo build -p render-cms`"));

        // render-cms
        stack.processes.push(Command::new(&render_cms)
            .arg(&site)
            .arg(site.join("configs/render-cms.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &cms_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("cannot start render-cms. Run `cargo build --release` in the m6 workspace and `cargo build -p render-cms`"));

        // Wait for all backend sockets.
        let timeout = Duration::from_secs(15);
        for sock in [&html_sock, &file_sock, &cms_sock] {
            if !wait_for_socket(sock, timeout) {
                panic!(
                    "socket {} did not appear. Run ./dev.sh first, then check the service logs",
                    sock.display()
                );
            }
        }
        if !wait_for_socket(&auth_sock, timeout) {
            panic!("auth socket did not appear. Run ./dev.sh first");
        }

        // Create symlinks in sock_dir so m6-http can resolve relative paths
        // (keys/, templates/, data/, assets/, configs/, content/) against sock_dir.
        for subdir in &["templates", "data", "assets", "configs", "keys", "content"] {
            let src = site.join(subdir);
            let dst = sock_path.join(subdir);
            if src.exists() {
                std::os::unix::fs::symlink(&src, &dst).ok();
            }
        }

        // m6-http — pass sock_dir as site_dir; it has site.toml + symlinked subdirs
        stack.processes.push(Command::new(&m6_http)
            .arg(&sock_path)
            .arg(&sys_toml)
            .arg("--log-level").arg("warn")
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().expect("cannot start m6-http. Run `cargo build --release` in the m6 workspace and `cargo build -p render-cms`"));

        if !wait_for_tcp(&tls_addr, Duration::from_secs(15)) {
            panic!("m6-http did not become ready on {tls_addr}. Run ./dev.sh first");
        }

        stack
    }
}

// ── assertion helpers ─────────────────────────────────────────────────────────

macro_rules! check_eq {
    ($label:expr, $got:expr, $want:expr) => {{
        let got  = $got;
        let want = $want;
        assert_eq!(got, want, "FAIL  {} — expected {}, got {}", $label, want, got);
        eprintln!("  PASS  {} ({})", $label, got);
    }};
}

macro_rules! check_contains {
    ($label:expr, $body:expr, $needle:expr) => {{
        let body   = &$body;
        let needle = $needle;
        assert!(body.contains(needle),
            "FAIL  {} — body does not contain {:?}\nbody: {}",
            $label, needle, &body[..body.len().min(500)]);
        eprintln!("  PASS  {} (contains {:?})", $label, needle);
    }};
}

macro_rules! check_status {
    // Accept any one of a set of expected codes.
    ($label:expr, $got:expr, $($want:expr),+) => {{
        let got = $got;
        let ok = false $(|| got == $want)+;
        assert!(ok, "FAIL  {} — got {}, expected one of [{}]",
            $label, got, stringify!($($want),+));
        eprintln!("  PASS  {} ({})", $label, got);
    }};
}

// ── the test ──────────────────────────────────────────────────────────────────

#[test]
fn blog_site_end_to_end() {
    let stack = ServerStack::start();

    let addr = &stack.addr;
    let tls  = make_tls_config();
    let mut cookies: HashMap<String, String> = HashMap::new();

    // ── §0 Server reachability ─────────────────────────────────────────────

    eprintln!("\n── §0 Server reachability ──");
    let r = get_with_cookie_str(addr, "/", "", Arc::clone(&tls));
    check_eq!("GET / reachable", r.status, 200);

    // ── §1 Public pages ───────────────────────────────────────────────────

    eprintln!("\n── §1 Public pages ──");

    let r = get_with_cookie_str(addr, "/", "", Arc::clone(&tls));
    check_eq!("GET / → 200", r.status, 200);
    check_contains!("GET / → site name", r.body, "m6");
    check_contains!("GET / → Recent Posts", r.body, "Recent Posts");

    let r = get_with_cookie_str(addr, "/about", "", Arc::clone(&tls));
    check_eq!("GET /about → 200", r.status, 200);

    let r = get_with_cookie_str(addr, "/blog", "", Arc::clone(&tls));
    check_eq!("GET /blog → 200", r.status, 200);
    check_contains!("GET /blog → lists posts", r.body, "quick-start");

    let r = get_with_cookie_str(addr, "/blog/quick-start", "", Arc::clone(&tls));
    check_eq!("GET /blog/quick-start → 200", r.status, 200);
    check_contains!("GET /blog/quick-start → title", r.body, "Quick Start");

    let r = get_with_cookie_str(addr, "/blog/architecture", "", Arc::clone(&tls));
    check_eq!("GET /blog/architecture → 200", r.status, 200);

    let r = get_with_cookie_str(addr, "/blog/nonexistent-post-xyz", "", Arc::clone(&tls));
    check_eq!("GET /blog/nonexistent → 200 (not-found page)", r.status, 200);
    check_contains!("GET /blog/nonexistent → not-found message", r.body, "not found");

    // ── §2 Static assets ──────────────────────────────────────────────────

    eprintln!("\n── §2 Static assets ──");

    let r = get_with_cookie_str(addr, "/assets/style.css", "", Arc::clone(&tls));
    check_eq!("GET /assets/style.css → 200", r.status, 200);
    let ct = r.header("content-type").unwrap_or("");
    assert!(ct.contains("text/css"),
        "FAIL  style.css content-type — got {:?}", ct);
    eprintln!("  PASS  style.css content-type is CSS ({ct})");

    let r = get_with_cookie_str(addr, "/assets/easymde.min.js", "", Arc::clone(&tls));
    check_eq!("GET /assets/easymde.min.js → 200", r.status, 200);

    let r = get_with_cookie_str(addr, "/assets/nonexistent.css", "", Arc::clone(&tls));
    check_eq!("GET /assets/nonexistent.css → 404", r.status, 404);

    // ── §3 Contact form ───────────────────────────────────────────────────

    eprintln!("\n── §3 Contact form ──");

    let r = get_with_cookie_str(addr, "/contact", "", Arc::clone(&tls));
    check_eq!("GET /contact → 200", r.status, 200);
    check_contains!("GET /contact → has <form>", r.body, "<form");
    check_contains!("GET /contact → has name field", r.body, "name");

    let r = post(addr, "/contact", "application/x-www-form-urlencoded",
        "name=Alice&email=alice%40example.com&message=Hello", "", Arc::clone(&tls));
    check_eq!("POST /contact → 200", r.status, 200);

    let r = post(addr, "/contact", "application/x-www-form-urlencoded",
        "name=TestUser&email=test%40example.com&message=Test+message", "", Arc::clone(&tls));
    check_eq!("POST /contact → 200 (name echo)", r.status, 200);
    check_contains!("POST /contact → echoes name", r.body, "TestUser");

    // ── §4 Login page ─────────────────────────────────────────────────────

    eprintln!("\n── §4 Login page ──");

    let r = get_with_cookie_str(addr, "/login", "", Arc::clone(&tls));
    check_eq!("GET /login → 200", r.status, 200);
    check_contains!("GET /login → has <form>", r.body, "<form");
    check_contains!("GET /login → has password field", r.body, "password");

    // ── §5 Auth endpoints ─────────────────────────────────────────────────

    eprintln!("\n── §5 Auth endpoints ──");

    // POST /auth/login → 302 redirect + Set-Cookie
    let r = post(addr, "/auth/login", "application/x-www-form-urlencoded",
        "username=admin&password=admin", "", Arc::clone(&tls));
    check_status!("POST /auth/login → 302", r.status, 302u16);
    harvest_cookies(&r, &mut cookies);
    check_eq!("POST /auth/login sets an auth cookie", !cookies.is_empty(), true);

    let cookie_str = build_cookie_header(&cookies);

    // ── §6 Protected CMS pages ────────────────────────────────────────────

    eprintln!("\n── §6 Protected CMS pages ──");

    // Without auth
    let r = get_with_cookie_str(addr, "/cms", "", Arc::clone(&tls));
    check_status!("GET /cms without auth → 401/302/403", r.status, 401u16, 302u16, 403u16);

    let r = get_with_cookie_str(addr, "/cms", &cookie_str, Arc::clone(&tls));
    check_eq!("GET /cms with auth → 200", r.status, 200);
    check_contains!("GET /cms → shows dashboard", r.body, "draft");

    let r = get_with_cookie_str(addr, "/cms/new", &cookie_str, Arc::clone(&tls));
    check_eq!("GET /cms/new with auth → 200", r.status, 200);
    check_contains!("GET /cms/new → has editor", r.body, "title");

    // ── §7 CMS API: draft lifecycle ───────────────────────────────────────

    eprintln!("\n── §7 CMS API: draft → publish → unpublish lifecycle ──");

    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
    let title  = format!("Test Post {ts}");
    let draft_body = format!(
        r#"{{"title":"{title}","body":"Hello world","summary":"A test post","tags":["test"],"date":"2026-03-21"}}"#
    );

    let r = post(addr, "/api/drafts", "application/json",
        &draft_body, &cookie_str, Arc::clone(&tls));
    check_eq!("POST /api/drafts → 201", r.status, 201);

    let stem: String = serde_json::from_str::<serde_json::Value>(&r.body)
        .ok()
        .and_then(|v| v["stem"].as_str().map(str::to_string))
        .unwrap_or_else(|| {
            panic!("FAIL  could not extract stem from: {}", &r.body[..r.body.len().min(200)])
        });
    eprintln!("  PASS  draft stem: {stem}");

    let r = get_with_cookie_str(addr, &format!("/cms/edit/{stem}"), &cookie_str, Arc::clone(&tls));
    check_eq!(&format!("GET /cms/edit/{stem} → 200"), r.status, 200);

    let r = patch(addr, &format!("/api/drafts/{stem}"),
        r#"{"summary":"Updated summary"}"#, &cookie_str, Arc::clone(&tls));
    check_eq!(&format!("PATCH /api/drafts/{stem} → 200"), r.status, 200);

    let r = post(addr, &format!("/api/publish/{stem}"),
        "application/json", "", &cookie_str, Arc::clone(&tls));
    check_eq!(&format!("POST /api/publish/{stem} → 200"), r.status, 200);
    check_contains!("publish → published=true", r.body, "published");

    // Give m6-http a moment to pick up the new content.
    std::thread::sleep(Duration::from_millis(300));
    let r = get_with_cookie_str(addr, &format!("/blog/{stem}"), "", Arc::clone(&tls));
    check_eq!(&format!("GET /blog/{stem} → 200 after publish"), r.status, 200);

    let r = post(addr, &format!("/api/unpublish/{stem}"),
        "application/json", "", &cookie_str, Arc::clone(&tls));
    check_eq!(&format!("POST /api/unpublish/{stem} → 200"), r.status, 200);
    check_contains!("unpublish → unpublished=true", r.body, "unpublished");

    // Clean up test artefacts from the site directory.
    let site = site_dir();
    std::fs::remove_file(site.join(format!("content/drafts/{stem}.json"))).ok();
    std::fs::remove_file(site.join(format!("content/posts/{stem}.json"))).ok();

    // ── §8 Auth: logout ───────────────────────────────────────────────────

    eprintln!("\n── §8 Auth logout ──");

    let r = post(addr, "/auth/logout", "application/x-www-form-urlencoded",
        "", &cookie_str, Arc::clone(&tls));
    check_status!("POST /auth/logout → 200/302/204", r.status, 200u16, 302u16, 204u16);

    // ── §9 Error handling ─────────────────────────────────────────────────

    eprintln!("\n── §9 Error handling ──");

    let r = get_with_cookie_str(addr, "/this-path-does-not-exist", "", Arc::clone(&tls));
    check_eq!("GET /nonexistent → 404", r.status, 404);

    let r = get_with_cookie_str(addr, "/blog/this-post-does-not-exist-xyz", "", Arc::clone(&tls));
    check_eq!("GET /blog/nonexistent → 200 (not-found page)", r.status, 200);
    check_contains!("GET /blog/nonexistent → not-found message", r.body, "not found");

    eprintln!("\n  All assertions passed.");
}
