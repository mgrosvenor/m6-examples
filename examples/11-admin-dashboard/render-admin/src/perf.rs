use serde_json::{json, Value};
use std::fs;
use std::path::Path;

/// Parse the last `n` "periodic stats" entries from the m6-http JSON-lines log.
/// Returns a JSON blob:
/// {
///   "history": [ { "ts": "...", "rps_avg": 0, "rps_peak": 0,
///                  "latency_p50_us": 0, "latency_p99_us": 0,
///                  "cache_hits": 0, "cache_misses": 0, "cache_hit_rate": 0.0,
///                  "backend_errors": 0, "pool_members": 0 }, ... ]
/// }
pub fn perf_blob(log_path: &Path, n: usize) -> Value {
    let history = read_periodic_stats(log_path, n);
    json!({ "history": history })
}

fn read_periodic_stats(log_path: &Path, n: usize) -> Vec<Value> {
    let text = match fs::read_to_string(log_path) {
        Ok(t) => t,
        Err(_) => return vec![],
    };

    let mut entries: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v.get("msg").and_then(Value::as_str) == Some("periodic stats"))
        .map(|v| {
            json!({
                "ts":              v["ts"],
                "rps_avg":         v["rps_avg"],
                "rps_peak":        v["rps_peak"],
                "latency_p50_us":  v["latency_p50_us"],
                "latency_p99_us":  v["latency_p99_us"],
                "cache_hits":      v["cache_hits"],
                "cache_misses":    v["cache_misses"],
                "cache_hit_rate":  v["cache_hit_rate"],
                "backend_errors":  v["backend_errors"],
                "pool_members":    v["pool_members"],
            })
        })
        .collect();

    // Keep only the last n entries
    if entries.len() > n {
        entries = entries.split_off(entries.len() - n);
    }
    entries
}
