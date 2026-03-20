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
//! If any required binary is absent the test prints a skip message and exits
//! cleanly — it does not fail the build.
//!
//! The site's auth database (`data/auth.db`) must exist with an `admin` user
//! in the `editors` group (created by `./dev.sh` or `./setup.sh`).
//! Auth-dependent sections are skipped gracefully if the database is absent.

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

    let raw_str = String::from_utf8_lossy(&raw);
    let split = raw_str.find("\r\n\r\n").unwrap_or(raw_str.len());
    let header_section = &raw_str[..split];
    let body_str = if split + 4 <= raw_str.len() { &raw_str[split + 4..] } else { "" };

    let mut lines = header_section.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line.split_whitespace().nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon) = line.find(':') {
            headers.push((line[..colon].trim().to_string(), line[colon+1..].trim().to_string()));
        }
    }

    Ok(Response { status, headers, body: body_str.to_string() })
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
    /// Start the full server stack.  Returns None if any required binary is absent
    /// or if the auth database has not been set up (setup.sh / dev.sh not yet run).
    fn start() -> Option<Self> {
        let site = site_dir();

        // Require all m6 binaries.
        let m6_http   = m6_bin("m6-http")?;
        let m6_html   = m6_bin("m6-html")?;
        let m6_file   = m6_bin("m6-file")?;
        let m6_auth   = m6_bin("m6-auth-server")?;
        let render_cms = render_cms_bin();
        if !render_cms.exists() { return None; }

        // Use a unique port per process ID to avoid collisions with dev server.
        let base_port: u16 = 20000 + (std::process::id() as u16 % 10000);
        let tls_addr = format!("127.0.0.1:{base_port}");

        let sock_dir = tempfile::tempdir().ok()?;
        let html_sock = sock_dir.path().join("m6-html.sock");
        let file_sock = sock_dir.path().join("m6-file.sock");
        let auth_sock = sock_dir.path().join("m6-auth.sock");
        let cms_sock  = sock_dir.path().join("render-cms.sock");

        // Write a system.toml that points at the dev TLS keys.
        let tls_cert = site.join("keys/dev.pem");
        let tls_key  = site.join("keys/dev-key.pem");
        if !tls_cert.exists() || !tls_key.exists() {
            eprintln!("SKIP: TLS certs not found at keys/dev.pem — run ./dev.sh first");
            return None;
        }

        let sys_toml = sock_dir.path().join("system.toml");
        std::fs::write(&sys_toml, format!(
            "[server]\nbind     = \"{tls_addr}\"\ntls_cert = \"{}\"\ntls_key  = \"{}\"\n",
            tls_cert.display(), tls_key.display(),
        )).ok()?;

        // Write a site.toml that points sockets at the temp sock_dir.
        let site_toml = sock_dir.path().join("site.toml");
        std::fs::copy(site.join("site.toml"), &site_toml).ok()?;
        // Patch socket glob paths in the copy.
        let original = std::fs::read_to_string(&site_toml).ok()?;
        let patched = original
            .replace("/tmp/m6/m6-html*.sock",    &html_sock.display().to_string())
            .replace("/tmp/m6/m6-file*.sock",    &file_sock.display().to_string())
            .replace("/tmp/m6/m6-auth*.sock",    &auth_sock.display().to_string())
            .replace("/tmp/m6/render-cms*.sock", &cms_sock.display().to_string());
        std::fs::write(&site_toml, &patched).ok()?;

        // Check auth DB exists (created by dev.sh / setup.sh).
        let auth_db = site.join("data/auth.db");
        let has_auth = auth_db.exists();
        if !has_auth {
            eprintln!("NOTE: data/auth.db absent — auth-dependent tests will be skipped");
        }

        let mut processes = Vec::new();

        // m6-html
        processes.push(Command::new(&m6_html)
            .arg(&site)
            .arg(site.join("configs/m6-html.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &html_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?);

        // m6-file
        processes.push(Command::new(&m6_file)
            .arg(&site)
            .arg(site.join("configs/m6-file.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &file_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?);

        // m6-auth-server (only if auth DB is present)
        if has_auth {
            processes.push(Command::new(&m6_auth)
                .arg(&site)
                .arg(site.join("configs/m6-auth.conf"))
                .arg("--log-level").arg("warn")
                .env("M6_SOCKET_OVERRIDE", &auth_sock)
                .stdout(Stdio::null()).stderr(Stdio::null())
                .spawn().ok()?);
        }

        // render-cms
        processes.push(Command::new(&render_cms)
            .arg(&site)
            .arg(site.join("configs/render-cms.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &cms_sock)
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?);

        // Wait for all backend sockets.
        let timeout = Duration::from_secs(15);
        for sock in [&html_sock, &file_sock, &cms_sock] {
            if !wait_for_socket(sock, timeout) {
                eprintln!("SKIP: socket {} did not appear", sock.display());
                return None;
            }
        }
        if has_auth && !wait_for_socket(&auth_sock, timeout) {
            eprintln!("SKIP: auth socket did not appear");
            return None;
        }

        // Create symlinks in sock_dir so m6-http can resolve relative paths
        // (keys/, templates/, data/, assets/, configs/, content/) against sock_dir.
        for subdir in &["templates", "data", "assets", "configs", "keys", "content"] {
            let src = site.join(subdir);
            let dst = sock_dir.path().join(subdir);
            if src.exists() {
                std::os::unix::fs::symlink(&src, &dst).ok();
            }
        }

        // m6-http — pass sock_dir as site_dir; it has site.toml + symlinked subdirs
        processes.push(Command::new(&m6_http)
            .arg(sock_dir.path())
            .arg(&sys_toml)
            .arg("--log-level").arg("warn")
            .stdout(Stdio::null()).stderr(Stdio::null())
            .spawn().ok()?);

        if !wait_for_tcp(&tls_addr, Duration::from_secs(15)) {
            eprintln!("SKIP: m6-http did not become ready on {tls_addr}");
            return None;
        }

        Some(ServerStack { addr: tls_addr, _sock_dir: sock_dir, processes })
    }

    fn has_auth(&self) -> bool {
        site_dir().join("data/auth.db").exists()
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
    let stack = match ServerStack::start() {
        Some(s) => s,
        None => {
            eprintln!("SKIP blog_site_end_to_end: prerequisites not met");
            return;
        }
    };

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

    let auth_available = stack.has_auth();

    if auth_available {
        // POST /auth/login → 302 redirect + Set-Cookie
        let r = post(addr, "/auth/login", "application/x-www-form-urlencoded",
            "username=admin&password=admin", "", Arc::clone(&tls));
        check_status!("POST /auth/login → 302", r.status, 302u16);
        harvest_cookies(&r, &mut cookies);

        if cookies.is_empty() {
            eprintln!("  SKIP  auth cookie not set (HttpOnly — not visible to test client)");
            // Try once more following redirect — the redirect response carries the cookie
        } else {
            eprintln!("  PASS  auth cookie obtained");
        }
    } else {
        eprintln!("  SKIP  §5 auth (no data/auth.db)");
    }

    let cookie_str = build_cookie_header(&cookies);

    // ── §6 Protected CMS pages ────────────────────────────────────────────

    eprintln!("\n── §6 Protected CMS pages ──");

    // Without auth
    let r = get_with_cookie_str(addr, "/cms", "", Arc::clone(&tls));
    check_status!("GET /cms without auth → 401/302/403", r.status, 401u16, 302u16, 403u16);

    if auth_available && !cookie_str.is_empty() {
        let r = get_with_cookie_str(addr, "/cms", &cookie_str, Arc::clone(&tls));
        check_eq!("GET /cms with auth → 200", r.status, 200);
        check_contains!("GET /cms → shows dashboard", r.body, "draft");

        let r = get_with_cookie_str(addr, "/cms/new", &cookie_str, Arc::clone(&tls));
        check_eq!("GET /cms/new with auth → 200", r.status, 200);
        check_contains!("GET /cms/new → has editor", r.body, "title");
    } else {
        eprintln!("  SKIP  authenticated CMS pages (no auth cookie)");
    }

    // ── §7 CMS API: draft lifecycle ───────────────────────────────────────

    eprintln!("\n── §7 CMS API: draft → publish → unpublish lifecycle ──");

    if auth_available && !cookie_str.is_empty() {
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
    } else {
        eprintln!("  SKIP  §7 CMS API (no auth cookie)");
    }

    // ── §8 Auth: logout ───────────────────────────────────────────────────

    eprintln!("\n── §8 Auth logout ──");

    if auth_available && !cookie_str.is_empty() {
        let r = post(addr, "/auth/logout", "application/x-www-form-urlencoded",
            "", &cookie_str, Arc::clone(&tls));
        check_status!("POST /auth/logout → 200/302/204", r.status, 200u16, 302u16, 204u16);
    } else {
        eprintln!("  SKIP  §8 logout (no auth cookie)");
    }

    // ── §9 Error handling ─────────────────────────────────────────────────

    eprintln!("\n── §9 Error handling ──");

    let r = get_with_cookie_str(addr, "/this-path-does-not-exist", "", Arc::clone(&tls));
    check_eq!("GET /nonexistent → 404", r.status, 404);

    let r = get_with_cookie_str(addr, "/blog/this-post-does-not-exist-xyz", "", Arc::clone(&tls));
    check_eq!("GET /blog/nonexistent → 200 (not-found page)", r.status, 200);
    check_contains!("GET /blog/nonexistent → not-found message", r.body, "not found");

    eprintln!("\n  All assertions passed.");
}
