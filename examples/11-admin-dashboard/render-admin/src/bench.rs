use parking_lot::Mutex;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct BenchJob {
    pub id: String,
    pub target: String,
    pub status: JobStatus,
    pub output: String,
    pub started_at: u64,
    pub finished_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum JobStatus {
    Running,
    Done,
    Failed,
}

pub type JobStore = Arc<Mutex<HashMap<String, BenchJob>>>;

pub fn new_job_store() -> JobStore {
    Arc::new(Mutex::new(HashMap::new()))
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// Derive bench targets from the site.toml bind address.
/// Returns JSON blob: { "targets": [ { "id": "h1", "label": "HTTP/1.1 127.0.0.1:8080",
///                                     "addr": "127.0.0.1:8080", "proto": "h1" }, ... ] }
pub fn targets_blob(site_toml: &Path) -> Value {
    let targets = discover_targets(site_toml);
    json!({ "targets": targets })
}

pub fn discover_targets(site_toml: &Path) -> Vec<Value> {
    let text = match std::fs::read_to_string(site_toml) {
        Ok(t) => t,
        Err(_) => return vec![],
    };
    let parsed: toml::Value = match toml::from_str(&text) {
        Ok(v) => v,
        Err(_) => return vec![],
    };

    let bind = parsed
        .get("server")
        .and_then(|s| s.get("bind"))
        .and_then(|b| b.as_str())
        .unwrap_or("127.0.0.1:8443");

    // Extract port from "host:port", default to 8443
    let port = bind.rsplit(':').next().unwrap_or("8443");
    let addr = format!("127.0.0.1:{port}");

    vec![
        json!({ "id": "h1",  "label": format!("HTTP/1.1 {addr}"), "addr": addr, "proto": "h1"  }),
        json!({ "id": "h2",  "label": format!("HTTP/2   {addr}"), "addr": addr, "proto": "h2"  }),
        json!({ "id": "h3",  "label": format!("HTTP/3   {addr}"), "addr": addr, "proto": "h3"  }),
        json!({ "id": "all", "label": format!("All      {addr}"), "addr": addr, "proto": "all" }),
    ]
}

/// Start a bench job. Returns `{ "job_id": "..." }` or an error Value.
pub fn start_bench(
    bench_bin: &Path,
    jobs: &JobStore,
    addr: &str,
    proto: &str,
    duration_secs: u64,
    concurrency: u64,
) -> Value {
    let id = uuid::Uuid::new_v4().to_string();
    let job = BenchJob {
        id: id.clone(),
        target: format!("{addr} ({proto})"),
        status: JobStatus::Running,
        output: String::new(),
        started_at: now_secs(),
        finished_at: None,
    };
    jobs.lock().insert(id.clone(), job);

    let bench_bin = bench_bin.to_path_buf();
    let addr = addr.to_string();
    let proto = proto.to_string();
    let jobs_clone = Arc::clone(jobs);
    let job_id = id.clone();

    std::thread::spawn(move || {
        run_bench_job(bench_bin, jobs_clone, job_id, addr, proto, duration_secs, concurrency);
    });

    json!({ "job_id": id })
}

fn run_bench_job(
    bench_bin: PathBuf,
    jobs: JobStore,
    job_id: String,
    addr: String,
    proto: String,
    duration_secs: u64,
    concurrency: u64,
) {
    let mut cmd = Command::new(&bench_bin);
    cmd.arg("--addr").arg(&addr)
        .arg("--duration").arg(duration_secs.to_string())
        .arg("--concurrency").arg(concurrency.to_string())
        .arg("--skip-verify");

    match proto.as_str() {
        "h1" => { cmd.arg("--http11-only"); }
        "h2" => { cmd.arg("--http2-only"); }
        "h3" => { cmd.arg("--http3-only"); }
        _    => {} // "all" — run all protocols
    }

    let result = cmd.output();
    let finished = now_secs();

    let mut store = jobs.lock();
    if let Some(job) = store.get_mut(&job_id) {
        match result {
            Ok(out) => {
                job.output = String::from_utf8_lossy(&out.stdout).to_string()
                    + &String::from_utf8_lossy(&out.stderr);
                job.status = if out.status.success() {
                    JobStatus::Done
                } else {
                    JobStatus::Failed
                };
            }
            Err(e) => {
                job.output = format!("failed to start m6-bench: {e}");
                job.status = JobStatus::Failed;
            }
        }
        job.finished_at = Some(finished);
    }
}

/// Poll a bench job by id.
/// Returns: { "id": "...", "target": "...", "status": "running"|"done"|"failed",
///            "output": "...", "started_at": 0, "finished_at": null }
pub fn poll_job(jobs: &JobStore, job_id: &str) -> Option<Value> {
    let store = jobs.lock();
    store.get(job_id).map(|j| {
        json!({
            "id":          j.id,
            "target":      j.target,
            "status":      match j.status { JobStatus::Running => "running", JobStatus::Done => "done", JobStatus::Failed => "failed" },
            "output":      j.output,
            "started_at":  j.started_at,
            "finished_at": j.finished_at,
        })
    })
}
