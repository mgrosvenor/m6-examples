use render_admin::{bench, logs, ops, perf, system};
use bench::JobStore;
use m6_render::{App, Error, Request, Response};
use ops::ServiceConfig;
use serde_json::{json, Value};
use std::path::PathBuf;

// ── Global state ──────────────────────────────────────────────────────────────

pub struct Global {
    /// m6-http JSON-lines log file to read perf/log data from.
    log_file:        PathBuf,
    /// Number of "request complete" log entries to sample for route stats.
    routes_sample:   usize,
    /// site.toml for the managed site (for config read/write/touch and bench targets).
    site_toml: PathBuf,
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

fn init_global(cfg: &serde_json::Map<String, Value>) -> m6_render::Result<Global> {
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
    let site_dir = PathBuf::from(std::env::args().nth(1).unwrap_or_default());
    let resolve = |raw: String| -> PathBuf {
        let p = PathBuf::from(&raw);
        if p.is_absolute() { p } else { site_dir.join(p) }
    };

    let log_file = PathBuf::from(get("log_file", "/tmp/m6/m6-http.log"));
    let site_toml = resolve(get("site_toml", "site.toml"));
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

fn handle_perf(req: &Request, g: &Global) -> m6_render::Result<Response> {
    let n = req.dict()
        .get("n")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(g.perf_history);
    Ok(Response::json(perf::perf_blob(&g.log_file, n)))
}

fn handle_routes(_req: &Request, g: &Global) -> m6_render::Result<Response> {
    let n = _req.dict()
        .get("n")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(g.routes_sample);
    Ok(Response::json(perf::routes_blob(&g.log_file, n)))
}

fn handle_system(_req: &Request, _g: &Global) -> m6_render::Result<Response> {
    Ok(Response::json(system::system_blob()))
}

fn handle_bench_targets(_req: &Request, g: &Global) -> m6_render::Result<Response> {
    Ok(Response::json(bench::targets_blob(&g.site_toml)))
}

fn handle_bench_start(req: &Request, g: &Global) -> m6_render::Result<Response> {
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

fn handle_bench_poll(req: &Request, g: &Global) -> m6_render::Result<Response> {
    let id = req["id"].as_str().unwrap_or("");
    match bench::poll_job(&g.bench_jobs, id) {
        Some(blob) => Ok(Response::json(blob)),
        None => Err(Error::NotFound),
    }
}

fn handle_logs(req: &Request, g: &Global) -> m6_render::Result<Response> {
    let n = req.dict()
        .get("n")
        .and_then(Value::as_str)
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(g.log_tail_default);
    Ok(Response::json(logs::logs_blob(&g.log_file, n)))
}

fn handle_config_read(_req: &Request, g: &Global) -> m6_render::Result<Response> {
    Ok(Response::json(ops::config_read(&g.site_toml)))
}

fn handle_config_write(req: &Request, g: &Global) -> m6_render::Result<Response> {
    let body: Value = req.body_json()
        .map_err(|_| Error::BadRequest("invalid JSON body".into()))?;

    // Body must be a JSON object — the full config tree to write back as TOML.
    if !body.is_object() {
        return Err(Error::BadRequest("body must be a JSON object".into()));
    }

    ops::config_write(&g.site_toml, &body)
        .map_err(|e| Error::BadRequest(e))?;

    Ok(Response::json(json!({ "ok": true })))
}

fn handle_config_touch(_req: &Request, g: &Global) -> m6_render::Result<Response> {
    ops::config_touch(&g.site_toml)
        .map_err(|e| Error::BadRequest(e))?;
    Ok(Response::json(json!({ "ok": true })))
}

fn handle_restart(req: &Request, g: &Global) -> m6_render::Result<Response> {
    let name = req["name"].as_str().unwrap_or("");
    let svc = g.services.iter().find(|s| s.name == name)
        .ok_or(Error::NotFound)?;
    Ok(Response::json(ops::restart_service(svc)))
}

// ── Main ──────────────────────────────────────────────────────────────────────

fn main() {
    App::with_global(init_global)
        // Performance tab: last N "periodic stats" entries from m6-http log
        .route_get("/api/admin/perf",           handle_perf)
        // Routes tab: per-route request counts, cache hit rates, avg latency
        .route_get("/api/admin/routes",         handle_routes)
        // System tab: CPU, RAM, disk, uptime
        .route_get("/api/admin/system",          handle_system)
        // Bench tab: list targets
        .route_get("/api/admin/bench",           handle_bench_targets)
        // Bench tab: start a job → { job_id }
        .route_post("/api/admin/bench",          handle_bench_start)
        // Bench tab: poll a job by id
        .route_get("/api/admin/bench/{id}",      handle_bench_poll)
        // Logs tab: last N log lines
        .route_get("/api/admin/logs",            handle_logs)
        // Config: read site.toml
        .route_get("/api/admin/config",          handle_config_read)
        // Config: write site.toml (validates TOML first)
        .route_put("/api/admin/config",          handle_config_write)
        // Config: touch site.toml → triggers m6-http hot reload
        .route_post("/api/admin/config/touch",   handle_config_touch)
        // Cache flush: alias for touch
        .route_post("/api/admin/cache/flush",    handle_config_touch)
        // Services: restart by name via SIGTERM
        .route_post("/api/admin/services/{name}/restart", handle_restart)
        .run()
        .unwrap_or_else(|e| {
            eprintln!("render-admin error: {e}");
            std::process::exit(1);
        });
}
