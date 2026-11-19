use render_admin::{bench, logs, ops, perf, system};
use bench::JobStore;
use m6_core::platform::process::raise_fd_limit;
use m6_core::prelude::*;
use m6_core::signal::{Action, Lifecycle};
use m6_core::site::finish_reload;
use m6_core::{genid_failed, genid_ok};
use ops::ServiceConfig;
use serde_json::{json, Value};
use std::io;
use std::path::PathBuf;
use std::time::Duration;

// ── Global state ──────────────────────────────────────────────────────────────

pub struct Global {
    /// m6-http JSON-lines log file to read perf/log data from.
    log_file:        PathBuf,
    /// Number of "request complete" log entries to sample for route stats.
    routes_sample:   usize,
    /// site.toml for the managed site (for config read/write/touch and the topology view).
    site_toml: PathBuf,
    /// System config m6-http starts with; the source of the bind address.
    system_toml: PathBuf,
    /// Number of perf history entries to return.
    perf_history: usize,
    /// Number of log lines to return by default.
    log_tail_default: usize,
    /// Path to the m6-bench binary.
    bench_bin: PathBuf,
    /// Default bench duration in seconds.
    bench_duration: u64,
    /// Default bench concurrency.
    bench_concurrency: u64,
    /// Named services that can be restarted (each has a PID file).
    services: Vec<ServiceConfig>,
    /// In-memory bench job store.
    bench_jobs: JobStore,
}

// Built once at start from the service's own settings. `Site` carries the parsed
// configuration, so this reads `site.user_config()` and `site.site_dir()`.
fn load_state(site: &Site) -> m6_core::Result<Global> {
    let cfg = site.user_config();
    let get = |key: &str, default: &str| -> String {
        cfg.get(key)
            .and_then(Value::as_str)
            .unwrap_or(default)
            .to_string()
    };
    let get_usize = |key: &str, default: usize| -> usize {
        cfg.get(key)
            .and_then(Value::as_u64)
            .map(|v| v as usize)
            .unwrap_or(default)
    };
    let get_u64 = |key: &str, default: u64| -> u64 {
        cfg.get(key)
            .and_then(Value::as_u64)
            .unwrap_or(default)
    };

    // Resolve paths relative to the site_dir (first CLI arg) when they are relative.
    let site_dir = site.site_dir();
    let resolve = |raw: String| -> PathBuf {
        let p = PathBuf::from(&raw);
        if p.is_absolute() { p } else { site_dir.join(p) }
    };

    let log_file = PathBuf::from(get("log_file", "/tmp/m6/m6-http.log"));
    let site_toml = resolve(get("site_toml", "site.toml"));
    let system_toml = resolve(get("system_toml", "configs/system-dev.toml"));
    let bench_bin = PathBuf::from(get("bench_bin", "m6-bench"));

    // Parse services: array of { name, pid_file } objects.
    let services: Vec<ServiceConfig> = cfg
        .get("services")
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let name = v.get("name")?.as_str()?.to_string();
                    let pid_file = v.get("pid_file")?.as_str()?.to_string();
                    Some(ServiceConfig { name, pid_file })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Global {
        log_file,
        routes_sample: get_usize("routes_sample", 10_000),
        site_toml,
        system_toml,
        perf_history: get_usize("perf_history", 60),
        log_tail_default: get_usize("log_tail_default", 100),
        bench_bin,
        bench_duration: get_u64("bench_duration_secs", 10),
        bench_concurrency: get_u64("bench_concurrency", 50),
        services,
        bench_jobs: bench::new_job_store(),
    })
}

// ── Handlers ──────────────────────────────────────────────────────────────────

fn handle_perf(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let n = req.dict()
        .get("n")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(g.perf_history);
    Ok(Response::json(perf::perf_blob(&g.log_file, n)))
}

fn handle_routes(_req: &Request, g: &Global) -> m6_core::Result<Response> {
    let n = _req.dict()
        .get("n")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(g.routes_sample);
    Ok(Response::json(perf::routes_blob(&g.log_file, n)))
}

fn handle_system(_req: &Request, _g: &Global) -> m6_core::Result<Response> {
    Ok(Response::json(system::system_blob()))
}

fn handle_bench_targets(_req: &Request, g: &Global) -> m6_core::Result<Response> {
    Ok(Response::json(bench::targets_blob(&g.system_toml)))
}

fn handle_bench_start(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let body: Value = req.body_json()
        .map_err(|_| Error::BadRequest("invalid JSON body".into()))?;

    let addr = body.get("addr")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::BadRequest("missing 'addr'".into()))?
        .to_string();

    let proto = body.get("proto")
        .and_then(Value::as_str)
        .unwrap_or("all")
        .to_string();

    let duration = body.get("duration_secs")
        .and_then(Value::as_u64)
        .unwrap_or(g.bench_duration);

    let concurrency = body.get("concurrency")
        .and_then(Value::as_u64)
        .unwrap_or(g.bench_concurrency);

    let result = bench::start_bench(&g.bench_bin, &g.bench_jobs, &addr, &proto, duration, concurrency);
    Ok(Response::json(result))
}

fn handle_bench_poll(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let id = req["id"].as_str().unwrap_or("");
    match bench::poll_job(&g.bench_jobs, id) {
        Some(blob) => Ok(Response::json(blob)),
        None => Err(Error::NotFound),
    }
}

fn handle_logs(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let n = req.dict().get("n").and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok()).unwrap_or(g.log_tail_default);
    let offset = req.dict().get("offset").and_then(Value::as_str)
        .and_then(|s| s.parse::<u64>().ok());
    Ok(Response::json(logs::logs_blob(&g.log_file, n, offset)))
}

fn handle_config_read(_req: &Request, g: &Global) -> m6_core::Result<Response> {
    Ok(Response::json(ops::config_read(&g.site_toml)))
}

fn handle_config_write(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let body: Value = req.body_json()
        .map_err(|_| Error::BadRequest("invalid JSON body".into()))?;

    // Body must be a JSON object — the full config tree to write back as TOML.
    if !body.is_object() {
        return Err(Error::BadRequest("body must be a JSON object".into()));
    }

    ops::config_write(&g.site_toml, &body)
        .map_err(Error::BadRequest)?;

    Ok(Response::json(json!({ "ok": true })))
}

fn handle_config_touch(_req: &Request, g: &Global) -> m6_core::Result<Response> {
    ops::config_touch(&g.site_toml)
        .map_err(Error::BadRequest)?;
    Ok(Response::json(json!({ "ok": true })))
}

fn handle_restart(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let name = req["name"].as_str().unwrap_or("");
    let svc = g.services.iter().find(|s| s.name == name)
        .ok_or(Error::NotFound)?;
    Ok(Response::json(ops::restart_service(svc)))
}

fn handle_backends_summary(_req: &Request, g: &Global) -> m6_core::Result<Response> {
    Ok(Response::json(perf::backends_summary_blob(
        &g.log_file,
        &g.site_toml,
        &g.system_toml,
        g.routes_sample,
    )))
}

fn handle_backend_sample(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let name = req["name"].as_str().unwrap_or("");
    let n = req.dict().get("n").and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok()).unwrap_or(g.routes_sample);
    Ok(Response::json(perf::backend_stats_blob(&g.log_file, &g.site_toml, name, n)))
}

fn handle_backend_alltime(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let name = req["name"].as_str().unwrap_or("");
    Ok(Response::json(perf::backend_stats_blob(&g.log_file, &g.site_toml, name, usize::MAX)))
}

fn handle_m6http_sample(req: &Request, g: &Global) -> m6_core::Result<Response> {
    let n = req.dict().get("n").and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok()).unwrap_or(g.routes_sample);
    Ok(Response::json(perf::m6http_stats_blob(&g.log_file, n)))
}

fn handle_m6http_alltime(_req: &Request, g: &Global) -> m6_core::Result<Response> {
    Ok(Response::json(perf::m6http_stats_blob(&g.log_file, usize::MAX)))
}

// ── Main ──────────────────────────────────────────────────────────────────────

/// The name the service passes to `Site::load`: the configuration file's stem, the
/// socket's name and the `[[backend]]` name in site.toml.
const NAME: &str = "render-admin";

/// The code routes, in the order `route` below matches them.
const ROUTES: &[CodeRoute] = &[
    // Performance tab: last N "periodic stats" entries from the m6-http log
    CodeRoute { pattern: "/api/admin/perf",                       method: RouteMethod::Get },
    // Routes tab: per-route request counts, cache hit rates, average latency
    CodeRoute { pattern: "/api/admin/routes",                     method: RouteMethod::Get },
    // System tab: CPU, RAM, disk, uptime
    CodeRoute { pattern: "/api/admin/system",                     method: RouteMethod::Get },
    // Bench tab: list targets
    CodeRoute { pattern: "/api/admin/bench",                      method: RouteMethod::Get },
    // Bench tab: start a job, answers { job_id }
    CodeRoute { pattern: "/api/admin/bench",                      method: RouteMethod::Post },
    // Bench tab: poll a job by id
    CodeRoute { pattern: "/api/admin/bench/{id}",                 method: RouteMethod::Get },
    // Logs tab: last N log lines
    CodeRoute { pattern: "/api/admin/logs",                       method: RouteMethod::Get },
    // Config: read site.toml
    CodeRoute { pattern: "/api/admin/config",                     method: RouteMethod::Get },
    // Config: write site.toml (validates the TOML first)
    CodeRoute { pattern: "/api/admin/config",                     method: RouteMethod::Put },
    // Config: touch site.toml (the file only: m6-http reads it at start)
    CodeRoute { pattern: "/api/admin/config/touch",               method: RouteMethod::Post },
    // Cache flush: an alias for touch
    CodeRoute { pattern: "/api/admin/cache/flush",                method: RouteMethod::Post },
    // Services: restart by name with SIGTERM
    CodeRoute { pattern: "/api/admin/services/{name}/restart",    method: RouteMethod::Post },
    // Backends: topology summary, then per-backend sample and all-time detail
    CodeRoute { pattern: "/api/admin/backends",                   method: RouteMethod::Get },
    CodeRoute { pattern: "/api/admin/backends/{name}/sample",     method: RouteMethod::Get },
    CodeRoute { pattern: "/api/admin/backends/{name}/alltime",    method: RouteMethod::Get },
    CodeRoute { pattern: "/api/admin/m6http/sample",              method: RouteMethod::Get },
    CodeRoute { pattern: "/api/admin/m6http/alltime",             method: RouteMethod::Get },
];

// ── the service's own event loop ─────────────────────────────────────────────
//
// m6-core supplies the parts (the reactor, the signal descriptor, the
// connection slots, the read deadlines, routing, the shutdown lines). The
// service writes the loop that ties them together, so nothing here is hidden
// in a framework. docs/m6-app-anatomy.md in m6 walks through it step by step.
//
// start: blocking and allocation are fine. run: nothing blocks. stop: blocking
// is fine again.

const LISTENER: Token = Token(usize::MAX);
const SIGNALS: Token = Token(usize::MAX - 1);
/// The log's stdout, registered only while it holds bytes a full pipe refused.
const LOG: Token = Token(usize::MAX - 2);
/// The longest the loop waits when nothing happens, and the log's flush tick.
const TICK: Duration = Duration::from_secs(1);

fn main() -> io::Result<()> {
    // Block the termination signals before anything can start a thread.
    m6_core::signal::block();

    // ── start ────────────────────────────────────────────────────────────
    let mut site = Site::load(NAME, ROUTES, &[], &TeraFactory);
    // State is a plain value in `main`: one thread owns it, so it needs no lock.
    // A start that cannot build it prints why and exits 2.
    let state = load_state(&site).unwrap_or_else(|e| fail("loading the configuration", e));
    let read_timeout = site.read_timeout();
    let ceiling = raise_fd_limit().unwrap_or_else(|e| fail("raising the open file limit", e));
    let table = site.limits().get(&m6_core::limits::CONNECTIONS);
    let ceiling = ceiling.min(table.max());
    let initial = table.start().min(ceiling);

    let mut poll = Poll::new().unwrap_or_else(|e| fail("creating the poll", e));
    // The signal descriptor opens before the socket does: the socket is how a
    // caller knows the service is ready, and a stop sent right after must find
    // the handler armed.
    let mut signals = Signals::new().unwrap_or_else(|e| fail("opening the signal descriptor", e));
    let mut listener = listen_unix(site.socket_path(), site.socket_mode())
        .unwrap_or_else(|e| fail(&format!("binding {}", site.socket_path().display()), e));
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;
    poll.registry()
        .register_fd(signals.raw_fd(), SIGNALS, Interest::READABLE)?;

    let mut lifecycle = Lifecycle::new(site.name(), Some(site.socket_path().to_path_buf()));
    lifecycle.started();

    // The connection table and its buffers follow the `[limits]` keys.
    let read_limit = site.limits().get(&m6_core::limits::REQUEST_BODY).clone();
    let write_limit = site.limits().get(&m6_core::limits::RESPONSE_BUFFER).clone();
    let mut conns = Slots::new(initial, ceiling, move || {
        ServerConn::with_limits(&read_limit, &write_limit)
    });
    // `read_timeout_s = 0` means no deadline, which is the zero duration.
    let mut deadlines = Timeouts::new(initial, read_timeout.unwrap_or(Duration::ZERO));
    let mut events = Events::with_capacity(256);
    let mut accepting = true;
    let mut last_tick = m6_core::platform::now();

    // ── run ──────────────────────────────────────────────────────────────
    'run: loop {
        // An interrupted wait is not an error: wait again.
        match poll.poll(
            &mut events,
            Some(deadlines.timeout(m6_core::platform::now(), TICK)),
        ) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        let now = m6_core::platform::now();
        for ev in &events {
            match ev.token() {
                // New connections: a free slot for each, registered, with a
                // read deadline.
                LISTENER => {
                    while let Some(taken) = conns.take() {
                        if let Some(new_len) = taken.grew_to {
                            deadlines.grow(new_len);
                        }
                        let mut stream = match listener.accept() {
                            Ok((stream, _)) => stream,
                            Err(e) => {
                                conns.free(taken.key);
                                match m6_core::platform::net::accept_error(&e) {
                                    m6_core::platform::net::Accept::Drained => break,
                                    m6_core::platform::net::Accept::SkipOne => continue,
                                    m6_core::platform::net::Accept::Stop => {
                                        eprintln!("{NAME}: accept error: {e}");
                                        break;
                                    }
                                }
                            }
                        };
                        poll.registry().register(
                            &mut stream,
                            Token(taken.key),
                            Interest::READABLE,
                        )?;
                        if read_timeout.is_some() {
                            deadlines.touch(Token(taken.key), now);
                        }
                        conns.get_mut(taken.key).attach(stream);
                    }
                    // At the ceiling, stop accepting: new clients wait in the
                    // kernel's backlog.
                    if conns.is_full() {
                        poll.registry().deregister(&mut listener)?;
                        accepting = false;
                    }
                }
                SIGNALS => {
                    let mut stop = false;
                    while let Some(signal) = signals.read() {
                        if lifecycle.on_signal(signal) == Action::Stop {
                            stop = true;
                        }
                    }
                    if stop {
                        break 'run;
                    }
                }
                LOG => m6_core::log::flush(),
                // One connection's turn, then wait for what it still needs,
                // push its deadline back, or close it.
                Token(k) => {
                    let conn = conns.get_mut(k);
                    let was = conn.interest();
                    match turn(conn, &mut site, &state) {
                        Ok(true) => {
                            if conn.interest() != was {
                                let interest = conn.interest();
                                let stream = conn
                                    .stream()
                                    .expect("every registered token has an attached connection");
                                poll.registry().reregister(stream, Token(k), interest)?;
                            }
                            if read_timeout.is_some() {
                                deadlines.touch(Token(k), now);
                            }
                        }
                        _ => m6_core::h1::close_conn(&poll, &mut conns, &mut deadlines, k)?,
                    }
                }
            }
        }

        // A full stdout holds its bytes. Watch it for writability only while
        // it does, so the loop never waits on the log.
        if let Err(e) = m6_core::log::watch(poll.registry(), LOG) {
            m6_core::log::error!(error = %e, "log descriptor registration failed");
        }
        // A client that sent nothing before its deadline is closed unanswered.
        while let Some(Token(k)) = deadlines.expired(m6_core::platform::now()) {
            m6_core::h1::close_conn(&poll, &mut conns, &mut deadlines, k)?;
        }
        // A slot came free while the table was full: accept again.
        if !accepting && !conns.is_full() {
            poll.registry()
                .register(&mut listener, LISTENER, Interest::READABLE)?;
            accepting = true;
        }
        if m6_core::platform::now() - last_tick >= TICK {
            m6_core::log::tick(m6_core::platform::wall());
            last_tick = m6_core::platform::now();
        }
    }

    // ── stop ─────────────────────────────────────────────────────────────
    // Connections still open are dropped: a stop does not wait for a request
    // in flight. `complete` removes the socket and logs the line.
    lifecycle.complete();
    m6_core::log::stop()?;
    Ok(())
}

/// One connection's turn: write what it owes, read, dispatch, write, and
/// report whether the connection stays open.
fn turn(conn: &mut ServerConn, site: &mut Site, state: &Global) -> io::Result<bool> {
    loop {
        // Write what is owed before reading anything.
        if conn.interest() == Interest::WRITABLE {
            match conn.flush()? {
                Flush::Done => {}
                Flush::More => return Ok(true),
                Flush::Close => return Ok(false),
            }
        }
        let mut heads = head_slots();
        let (response, idle) = match conn.read(&mut heads)? {
            Read::Request(head) => {
                let response = match site.handle(&head) {
                    // A page, a 404 or an error, already finished.
                    Handled::Done(response) => response,
                    // One of ROUTES: run its handler, then `finish` adds the
                    // site-wide parts (templates, Cache-Control, CSRF, flash).
                    Handled::Code(index, req) => {
                        let response = route(index, &req, state)
                            .unwrap_or_else(|e| m6_core::response::error_to_response(&e));
                        site.finish(&req, response)
                    }
                    // `POST /genid`: read config, templates and params again
                    // through the start path, and keep the old site if that
                    // fails.
                    Handled::Reload(reload) => {
                        let response = match site.reload(ROUTES, &[], &TeraFactory) {
                            Ok(next) => {
                                *site = next;
                                genid_ok()
                            }
                            Err(e) => genid_failed(&e),
                        };
                        finish_reload(&reload, response)
                    }
                    Handled::Named(..) => unreachable!("{NAME} registers no named handlers"),
                };
                (Some(response), false)
            }
            Read::More => (None, true),
            Read::Closed => return Ok(false),
        };
        if let Some(response) = response {
            conn.respond_response(response);
        }
        match conn.flush()? {
            // Answered and kept alive: a pipelined request may already be
            // buffered, so read again before waiting.
            Flush::Done if !idle => {}
            Flush::Done | Flush::More => return Ok(true),
            Flush::Close => return Ok(false),
        }
    }
}

/// A failure at start is printed and exits 2, the code for a bad start.
fn fail(what: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("{NAME}: {what}: {error}");
    std::process::exit(2)
}

/// The handler for one route: the arm's number is the route's place in `ROUTES`.
fn route(index: usize, req: &Request, g: &Global) -> m6_core::Result<Response> {
    match index {
        0 => handle_perf(req, g),
        1 => handle_routes(req, g),
        2 => handle_system(req, g),
        3 => handle_bench_targets(req, g),
        4 => handle_bench_start(req, g),
        5 => handle_bench_poll(req, g),
        6 => handle_logs(req, g),
        7 => handle_config_read(req, g),
        8 => handle_config_write(req, g),
        9 => handle_config_touch(req, g),
        10 => handle_config_touch(req, g),
        11 => handle_restart(req, g),
        12 => handle_backends_summary(req, g),
        13 => handle_backend_sample(req, g),
        14 => handle_backend_alltime(req, g),
        15 => handle_m6http_sample(req, g),
        16 => handle_m6http_alltime(req, g),
        _ => unreachable!("route {index} is not in ROUTES"),
    }
}
