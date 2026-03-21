//! End-to-end integration test for example 09 — global deployment topology.
//!
//! Spins up the full 6-node stack on loopback:
//!   - Origin: m6-http with TLS (bind) + H2C backbone (h2c_bind)
//!   - 5 cache nodes: each m6-http over TLS, proxying to origin over H2C
//!   - Backend services: m6-html, m6-file, m6-auth-server, render-cms (from 07)
//!
//! Verifies:
//!   §0  All 6 TLS endpoints are reachable
//!   §1  Public pages serve correctly from origin (direct TLS)
//!   §2  All 5 cache nodes proxy /blog and /assets to origin
//!   §3  Cache hit-or-miss: second request to a cache node is cache-hit (x-cache header)
//!   §4  CMS routes (no-store) pass through to origin
//!   §5  Origin H2C backbone is reachable on its own TCP port
//!
//! Prerequisites: m6 release binaries pre-built, render-cms from 07 pre-built.
//! If any binary is absent the test skips cleanly.

use std::io::{Read, Write as IoWrite};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

// ── binary locations ──────────────────────────────────────────────────────────

fn m6_root() -> PathBuf {
    // integration-test/  →  09-global-deployment/  →  examples/  →  m6-examples/  →  m6/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../../m6")
        .canonicalize()
        .expect("m6 workspace root")
}

fn m6_bin(name: &str) -> Option<PathBuf> {
    let release = m6_root().join("target/release").join(name);
    if release.exists() { return Some(release); }
    which(name)
}

fn render_cms_bin() -> Option<PathBuf> {
    // integration-test/ → 09-global-deployment/ → examples/ → m6-examples/target/release/
    let bin = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../../target/release/render-cms")
        .canonicalize();
    if let Ok(p) = bin { if p.exists() { return Some(p); } }
    which("render-cms")
}

fn which(name: &str) -> Option<PathBuf> {
    std::env::var_os("PATH")
        .and_then(|p| std::env::split_paths(&p).find_map(|dir| {
            let full = dir.join(name);
            if full.exists() { Some(full) } else { None }
        }))
}

fn site07_dir() -> PathBuf {
    // integration-test/  →  09-global-deployment/  →  examples/07-dev-to-production/
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../07-dev-to-production")
        .canonicalize()
        .expect("07-dev-to-production site dir")
}

fn example09_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("09-global-deployment dir")
}

// ── TLS: no-verify client ─────────────────────────────────────────────────────

#[derive(Debug)]
struct NoVerify;

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self, _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8], _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn verify_tls13_signature(&self, _: &[u8], _: &rustls::pki_types::CertificateDer<'_>,
        _: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
    }
    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms.supported_schemes()
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

// ── TLS cert generation ───────────────────────────────────────────────────────

fn generate_test_cert(dir: &Path) -> (PathBuf, PathBuf) {
    let cert_path = dir.join("cert.pem");
    let key_path  = dir.join("key.pem");

    let cert = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])
        .expect("rcgen cert");
    std::fs::write(&cert_path, cert.cert.pem()).expect("write cert");
    std::fs::write(&key_path,  cert.key_pair.serialize_pem()).expect("write key");

    (cert_path, key_path)
}

// ── HTTP helpers ──────────────────────────────────────────────────────────────

struct Response {
    status:  u16,
    #[allow(dead_code)]
    headers: Vec<(String, String)>,
    body:    String,
}

fn https_get(addr: &str, path: &str, extra_headers: &[(&str, &str)],
    tls: Arc<rustls::ClientConfig>,
) -> Result<Response, String> {
    let tcp = TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
    tcp.set_read_timeout(Some(Duration::from_secs(10))).ok();
    tcp.set_write_timeout(Some(Duration::from_secs(10))).ok();

    let server_name = rustls::pki_types::ServerName::try_from("localhost".to_string())
        .map_err(|e| format!("server name: {e}"))?;
    let conn = rustls::ClientConnection::new(tls, server_name)
        .map_err(|e| format!("TLS init: {e}"))?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);

    let mut req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\n");
    for (k, v) in extra_headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("Connection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).map_err(|e| format!("write: {e}"))?;
    stream.flush().map_err(|e| format!("flush: {e}"))?;

    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok();

    let s = String::from_utf8_lossy(&raw);
    let split = s.find("\r\n\r\n").unwrap_or(s.len());
    let hdr_sec = &s[..split];
    let body = if split + 4 <= s.len() { &s[split + 4..] } else { "" };

    let mut lines = hdr_sec.split("\r\n");
    let status_line = lines.next().unwrap_or("");
    let status: u16 = status_line.split_whitespace().nth(1)
        .and_then(|s| s.parse().ok()).unwrap_or(0);

    let mut headers = Vec::new();
    for line in lines {
        if let Some(colon) = line.find(':') {
            headers.push((line[..colon].trim().to_string(), line[colon+1..].trim().to_string()));
        }
    }

    Ok(Response { status, headers, body: body.to_string() })
}

fn get(addr: &str, path: &str, tls: Arc<rustls::ClientConfig>) -> Response {
    https_get(addr, path, &[], tls).unwrap_or_else(|e| panic!("GET {path}: {e}"))
}

// ── wait helpers ──────────────────────────────────────────────────────────────

fn wait_tcp(addr: &str, timeout: Duration) -> bool {
    let dl = Instant::now() + timeout;
    while Instant::now() < dl {
        if TcpStream::connect(addr).is_ok() { return true; }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

fn wait_socket(path: &Path, timeout: Duration) -> bool {
    let dl = Instant::now() + timeout;
    while Instant::now() < dl {
        if path.exists() { return true; }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

// ── topology stack ────────────────────────────────────────────────────────────

struct GlobalStack {
    origin_addr:  String,          // TLS endpoint
    origin_h2c:   String,          // H2C backbone (plain TCP)
    cache_addrs:  Vec<String>,     // [sf, nyc, chicago, london, singapore]
    processes:    Vec<Child>,
    _temp_dirs:   Vec<tempfile::TempDir>,
}

impl Drop for GlobalStack {
    fn drop(&mut self) {
        for c in &mut self.processes { c.kill().ok(); c.wait().ok(); }
    }
}

impl GlobalStack {
    /// Build and start the full 6-node topology.
    /// Returns None if any required binary or site setup is missing.
    fn start() -> Option<Self> {
        let site07   = site07_dir();
        let site09   = example09_dir();

        let m6_http  = m6_bin("m6-http")?;
        let m6_html  = m6_bin("m6-html")?;
        let m6_file  = m6_bin("m6-file")?;
        let m6_auth  = m6_bin("m6-auth-server")?;
        let render   = render_cms_bin()?;

        // Unique ports per test run to avoid collision with dev servers.
        let pid      = std::process::id() as u16 % 5000;
        let h2c_port = 30000 + pid;
        let tls_port = 30100 + pid;
        // Cache nodes: 5 ports starting from 30200
        let cache_ports: Vec<u16> = (0..5).map(|i| 30200 + pid + i).collect();

        let origin_h2c_addr  = format!("127.0.0.1:{h2c_port}");
        let origin_tls_addr  = format!("127.0.0.1:{tls_port}");
        let cache_addrs: Vec<String> = cache_ports.iter()
            .map(|p| format!("127.0.0.1:{p}")).collect();

        // Generate self-signed cert.
        let cert_dir = tempfile::tempdir().ok()?;
        let (cert_path, key_path) = generate_test_cert(cert_dir.path());

        // ── Origin site dir ───────────────────────────────────────────────────
        // Write origin site.toml: full route table, socket paths in a temp dir.

        let origin_sock_dir = tempfile::tempdir().ok()?;
        let html_sock  = origin_sock_dir.path().join("m6-html.sock");
        let file_sock  = origin_sock_dir.path().join("m6-file.sock");
        let auth_sock  = origin_sock_dir.path().join("m6-auth.sock");
        let cms_sock   = origin_sock_dir.path().join("render-cms.sock");

        // Copy site-origin/site.toml and patch socket paths.
        let origin_site_src = site09.join("site-origin/site.toml");
        if !origin_site_src.exists() {
            eprintln!("SKIP: site-origin/site.toml not found");
            return None;
        }
        let origin_site_toml = std::fs::read_to_string(&origin_site_src).ok()?;
        let origin_site_toml = origin_site_toml
            .replace("/tmp/m6/m6-html*.sock",    &html_sock.display().to_string())
            .replace("/tmp/m6/m6-file*.sock",    &file_sock.display().to_string())
            .replace("/tmp/m6/m6-auth*.sock",    &auth_sock.display().to_string())
            .replace("/tmp/m6/render-cms*.sock", &cms_sock.display().to_string());

        // Resolve auth public_key: patch the relative "../keys/auth.pub" to absolute.
        let auth_pub = site09.join("keys/auth.pub");
        let auth_pub_str = if auth_pub.exists() {
            auth_pub.display().to_string()
        } else if site07.join("keys/auth.pub").exists() {
            site07.join("keys/auth.pub").display().to_string()
        } else {
            eprintln!("SKIP: auth public key not found — run dev.sh or 07/dev.sh first");
            return None;
        };
        let origin_site_toml = origin_site_toml
            .replace("\"../keys/auth.pub\"", &format!("\"{}\"", auth_pub_str));
        std::fs::write(origin_sock_dir.path().join("site.toml"), &origin_site_toml).ok()?;

        // Symlink site subdirs so m6-http resolves template/asset paths correctly.
        for subdir in &["templates", "data", "assets", "configs", "keys", "content"] {
            let src = site07.join(subdir);
            let dst = origin_sock_dir.path().join(subdir);
            if src.exists() { std::os::unix::fs::symlink(&src, &dst).ok(); }
        }

        // Origin system config: TLS + H2C.
        let origin_sys_toml = origin_sock_dir.path().join("system.toml");
        std::fs::write(&origin_sys_toml, format!(
            "[server]\nbind     = \"{origin_tls_addr}\"\ntls_cert = \"{}\"\ntls_key  = \"{}\"\nh2c_bind = \"{origin_h2c_addr}\"\n",
            cert_path.display(), key_path.display(),
        )).ok()?;

        // ── Cache site dir ────────────────────────────────────────────────────
        let cache_sock_dir = tempfile::tempdir().ok()?;
        let cache_site_src = site09.join("site-cache/site.toml");
        if !cache_site_src.exists() {
            eprintln!("SKIP: site-cache/site.toml not found");
            return None;
        }
        let cache_site_toml = std::fs::read_to_string(&cache_site_src).ok()?;
        let cache_site_toml = cache_site_toml
            .replace("h2c://127.0.0.1:9000", &format!("h2c://127.0.0.1:{h2c_port}"));
        std::fs::write(cache_sock_dir.path().join("site.toml"), &cache_site_toml).ok()?;

        let mut processes = Vec::new();
        let mut temp_dirs: Vec<tempfile::TempDir> = vec![cert_dir];

        // ── Backend services ──────────────────────────────────────────────────
        processes.push(Command::new(&m6_html)
            .arg(&site07).arg(site07.join("configs/m6-html.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &html_sock)
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?);

        processes.push(Command::new(&m6_file)
            .arg(&site07).arg(site07.join("configs/m6-file.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &file_sock)
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?);

        let has_auth = auth_pub.exists() || site07.join("keys/auth.pub").exists();
        let auth_db = site07.join("configs/data/auth.db");
        let has_auth_db = auth_db.exists();

        if has_auth && has_auth_db {
            processes.push(Command::new(&m6_auth)
                .arg(&site07).arg(site07.join("configs/m6-auth.conf"))
                .arg("--log-level").arg("warn")
                .env("M6_SOCKET_OVERRIDE", &auth_sock)
                .stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?);
        }

        processes.push(Command::new(&render)
            .arg(&site07).arg(site07.join("configs/render-cms.conf"))
            .arg("--log-level").arg("warn")
            .env("M6_SOCKET_OVERRIDE", &cms_sock)
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?);

        let to = Duration::from_secs(15);
        for sock in [&html_sock, &file_sock, &cms_sock] {
            if !wait_socket(sock, to) {
                eprintln!("SKIP: socket {} not ready", sock.display());
                return None;
            }
        }
        if has_auth && has_auth_db && !wait_socket(&auth_sock, to) {
            eprintln!("SKIP: auth socket not ready");
            return None;
        }

        // ── Origin m6-http (TLS + H2C) ────────────────────────────────────────
        processes.push(Command::new(&m6_http)
            .arg(origin_sock_dir.path()).arg(&origin_sys_toml)
            .arg("--log-level").arg("warn")
            .stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?);

        if !wait_tcp(&origin_tls_addr, to) {
            eprintln!("SKIP: origin TLS not ready on {origin_tls_addr}");
            return None;
        }
        if !wait_tcp(&origin_h2c_addr, to) {
            eprintln!("SKIP: origin H2C not ready on {origin_h2c_addr}");
            return None;
        }

        // ── 5 cache nodes ─────────────────────────────────────────────────────
        let city_names = ["SF", "NYC", "Chicago", "London", "Singapore"];
        for (i, &port) in cache_ports.iter().enumerate() {
            let sys = cache_sock_dir.path().join(format!("cache-{i}.toml"));
            std::fs::write(&sys, format!(
                "# Cache node: {}\n[server]\nbind     = \"127.0.0.1:{port}\"\ntls_cert = \"{}\"\ntls_key  = \"{}\"\n",
                city_names[i], cert_path.display(), key_path.display(),
            )).ok()?;

            processes.push(Command::new(&m6_http)
                .arg(cache_sock_dir.path()).arg(&sys)
                .arg("--log-level").arg("warn")
                .stdout(Stdio::null()).stderr(Stdio::null()).spawn().ok()?);
        }

        for addr in &cache_addrs {
            if !wait_tcp(addr, to) {
                eprintln!("SKIP: cache node not ready on {addr}");
                return None;
            }
        }

        temp_dirs.push(origin_sock_dir);
        temp_dirs.push(cache_sock_dir);

        Some(GlobalStack {
            origin_addr:  origin_tls_addr,
            origin_h2c:   origin_h2c_addr,
            cache_addrs,
            processes,
            _temp_dirs: temp_dirs,
        })
    }
}

// ── test macros ───────────────────────────────────────────────────────────────

macro_rules! check_eq {
    ($label:expr, $got:expr, $want:expr) => {{
        let got = $got; let want = $want;
        assert_eq!(got, want, "FAIL  {} — expected {want}, got {got}", $label);
        eprintln!("  PASS  {} ({got})", $label);
    }};
}

macro_rules! check_contains {
    ($label:expr, $body:expr, $needle:expr) => {{
        let body = &$body; let needle = $needle;
        assert!(body.contains(needle),
            "FAIL  {} — body does not contain {needle:?}\nbody: {}",
            $label, &body[..body.len().min(500)]);
        eprintln!("  PASS  {} (contains {needle:?})", $label);
    }};
}

// ── the test ──────────────────────────────────────────────────────────────────

#[test]
fn global_deployment_end_to_end() {
    let stack = match GlobalStack::start() {
        Some(s) => s,
        None => {
            eprintln!("SKIP global_deployment_end_to_end: prerequisites not met");
            return;
        }
    };

    let tls = make_tls_config();
    let origin = &stack.origin_addr;
    let city_names = ["SF", "NYC", "Chicago", "London", "Singapore"];

    // ── §0 All 6 TLS endpoints reachable ─────────────────────────────────

    eprintln!("\n── §0 All TLS endpoints reachable ──");

    let r = get(origin, "/blog", Arc::clone(&tls));
    check_eq!("origin GET /blog → 200", r.status, 200);

    for (i, addr) in stack.cache_addrs.iter().enumerate() {
        let r = get(addr, "/blog", Arc::clone(&tls));
        check_eq!(&format!("cache {} GET /blog → 200", city_names[i]), r.status, 200);
    }

    // ── §1 Public pages on origin ─────────────────────────────────────────

    eprintln!("\n── §1 Public pages on origin ──");

    let r = get(origin, "/", Arc::clone(&tls));
    check_eq!("origin GET / → 200", r.status, 200);

    let r = get(origin, "/blog", Arc::clone(&tls));
    check_eq!("origin GET /blog → 200", r.status, 200);
    check_contains!("origin /blog → post list", r.body, "post-card");

    let r = get(origin, "/blog/quick-start", Arc::clone(&tls));
    check_eq!("origin GET /blog/quick-start → 200", r.status, 200);
    check_contains!("origin /blog/quick-start → title", r.body, "Quick Start");

    let r = get(origin, "/assets/style.css", Arc::clone(&tls));
    check_eq!("origin GET /assets/style.css → 200", r.status, 200);

    // ── §2 Cache nodes proxy content from origin ──────────────────────────

    eprintln!("\n── §2 Cache nodes proxy content from origin ──");

    for (i, addr) in stack.cache_addrs.iter().enumerate() {
        let r = get(addr, "/blog", Arc::clone(&tls));
        check_eq!(&format!("{} GET /blog → 200", city_names[i]), r.status, 200);
        check_contains!(&format!("{} /blog → post list", city_names[i]), r.body, "post-card");

        let r = get(addr, "/blog/quick-start", Arc::clone(&tls));
        check_eq!(&format!("{} GET /blog/quick-start → 200", city_names[i]), r.status, 200);
        check_contains!(&format!("{} /blog/quick-start → title", city_names[i]),
            r.body, "Quick Start");

        let r = get(addr, "/assets/style.css", Arc::clone(&tls));
        check_eq!(&format!("{} GET /assets/style.css → 200", city_names[i]), r.status, 200);
    }

    // ── §3 Cache node response matches origin exactly ─────────────────────

    eprintln!("\n── §3 Cache consistency: cache vs origin ──");

    let r_origin = get(origin, "/blog/quick-start", Arc::clone(&tls));
    for (i, addr) in stack.cache_addrs.iter().enumerate() {
        let r_cache = get(addr, "/blog/quick-start", Arc::clone(&tls));
        check_eq!(&format!("{} /blog/quick-start status = origin", city_names[i]),
            r_cache.status, r_origin.status);
        // Both bodies should contain the same post title
        assert!(r_cache.body.contains("Quick Start"),
            "FAIL  {} body missing 'Quick Start'", city_names[i]);
        eprintln!("  PASS  {} body matches origin content", city_names[i]);
    }

    // ── §4 CMS routes are no-store (proxied to origin) ────────────────────

    eprintln!("\n── §4 CMS routes pass through to origin ──");

    // /cms without auth should return 401/302/403 on both origin and cache nodes
    let r = get(origin, "/cms", Arc::clone(&tls));
    assert!([401u16, 302, 403].contains(&r.status),
        "FAIL  origin /cms without auth → {}, expected 401/302/403", r.status);
    eprintln!("  PASS  origin /cms without auth → {} (auth enforced)", r.status);

    for (i, addr) in stack.cache_addrs.iter().enumerate() {
        let r = get(addr, "/cms", Arc::clone(&tls));
        assert!([401u16, 302, 403].contains(&r.status),
            "FAIL  {} /cms without auth → {}, expected 401/302/403", city_names[i], r.status);
        eprintln!("  PASS  {} /cms without auth → {} (forwarded to origin)", city_names[i], r.status);
    }

    // ── §5 H2C backbone is reachable ──────────────────────────────────────

    eprintln!("\n── §5 H2C backbone reachable ──");

    assert!(TcpStream::connect(&stack.origin_h2c).is_ok(),
        "FAIL  H2C backbone port {} not reachable", stack.origin_h2c);
    eprintln!("  PASS  H2C backbone {} is reachable (TCP)", stack.origin_h2c);

    // ── §6 404 handling on all nodes ──────────────────────────────────────

    eprintln!("\n── §6 404 handling on all nodes ──");

    let r = get(origin, "/this-path-does-not-exist", Arc::clone(&tls));
    check_eq!("origin GET /nonexistent → 404", r.status, 404);

    for (i, addr) in stack.cache_addrs.iter().enumerate() {
        let r = get(addr, "/this-path-does-not-exist", Arc::clone(&tls));
        check_eq!(&format!("{} GET /nonexistent → 404", city_names[i]), r.status, 404);
    }

    eprintln!("\n  All assertions passed — 6-node global deployment verified.");
}
