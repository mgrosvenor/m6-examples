use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::path::Path;

// ── Periodic-stats history ────────────────────────────────────────────────────

/// Parse the last `n` "periodic stats" entries from the m6-http JSON-lines log.
/// Returns:
/// {
///   "history": [ { "ts", "rps_avg", "rps_peak", "latency_p50_us", "latency_p99_us",
///                  "cache_hits", "cache_misses", "cache_hit_rate",
///                  "backend_errors", "pool_members" }, ... ]
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
        .map(|v| json!({
            "ts":             v["ts"],
            "rps_avg":        v["rps_avg"],
            "rps_peak":       v["rps_peak"],
            "latency_p50_us": v["latency_p50_us"],
            "latency_p99_us": v["latency_p99_us"],
            "cache_hits":     v["cache_hits"],
            "cache_misses":   v["cache_misses"],
            "cache_hit_rate": v["cache_hit_rate"],
            "backend_errors": v["backend_errors"],
            "pool_members":   v["pool_members"],
        }))
        .collect();

    if entries.len() > n {
        entries = entries.split_off(entries.len() - n);
    }
    entries
}

// ── Per-route stats ───────────────────────────────────────────────────────────

/// Parse the last `sample` "request complete" log entries and aggregate per-route stats.
/// Returns:
/// {
///   "sample_requests": 1000,
///   "sample_window_secs": 42,       // wall time covered by the sample (null if unknown)
///   "routes": [
///     {
///       "path":         "/blog",
///       "requests":     820,
///       "cache_hits":   710,
///       "cache_misses": 110,
///       "hit_rate":     0.866,
///       "avg_latency_us": 240
///     }, ...
///   ]
/// }
pub fn routes_blob(log_path: &Path, sample: usize) -> Value {
    let text = match fs::read_to_string(log_path) {
        Ok(t) => t,
        Err(_) => return json!({ "sample_requests": 0, "sample_window_secs": null, "routes": [] }),
    };

    // Collect the last `sample` "request complete" entries.
    let entries: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|v| v.get("msg").and_then(Value::as_str) == Some("request complete"))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take(sample)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();

    let sample_requests = entries.len();

    // Determine time window from first/last ts (if present).
    let first_ts = entries.first().and_then(|v| v["ts"].as_str()).map(|s| s.to_string());
    let last_ts  = entries.last().and_then(|v| v["ts"].as_str()).map(|s| s.to_string());
    let window_secs = ts_diff_secs(first_ts.as_deref(), last_ts.as_deref());

    // Aggregate per-path.
    struct RouteStats { requests: u64, cache_hits: u64, latency_sum: u64 }
    let mut by_path: HashMap<String, RouteStats> = HashMap::new();

    for v in &entries {
        let path = v["path"].as_str().unwrap_or("/").to_string();
        let cache_hit = v["cache_hit"].as_bool().unwrap_or(false);
        let latency = v["latency_us"].as_u64().unwrap_or(0);

        let s = by_path.entry(path).or_insert(RouteStats { requests: 0, cache_hits: 0, latency_sum: 0 });
        s.requests   += 1;
        s.latency_sum += latency;
        if cache_hit { s.cache_hits += 1; }
    }

    let mut routes: Vec<Value> = by_path
        .into_iter()
        .map(|(path, s)| {
            let misses   = s.requests - s.cache_hits;
            let hit_rate = if s.requests > 0 { s.cache_hits as f64 / s.requests as f64 } else { 0.0 };
            let avg_lat  = if s.requests > 0 { s.latency_sum / s.requests } else { 0 };
            json!({
                "path":           path,
                "requests":       s.requests,
                "cache_hits":     s.cache_hits,
                "cache_misses":   misses,
                "hit_rate":       (hit_rate * 1000.0).round() / 1000.0,
                "avg_latency_us": avg_lat,
            })
        })
        .collect();

    // Sort by request count descending.
    routes.sort_by(|a, b| {
        b["requests"].as_u64().unwrap_or(0)
            .cmp(&a["requests"].as_u64().unwrap_or(0))
    });

    json!({
        "sample_requests":   sample_requests,
        "sample_window_secs": window_secs,
        "routes": routes,
    })
}

/// Parse two RFC-3339-ish timestamps and return their difference in seconds, if possible.
fn ts_diff_secs(first: Option<&str>, last: Option<&str>) -> Option<u64> {
    let parse = |s: &str| -> Option<u64> {
        // Expect "YYYY-MM-DDTHH:MM:SS" or similar — just parse up to seconds.
        let s = s.trim_end_matches('Z');
        let s = s.get(..19)?; // "YYYY-MM-DDTHH:MM:SS"
        let parts: Vec<&str> = s.splitn(2, 'T').collect();
        if parts.len() != 2 { return None; }
        let date: Vec<u32> = parts[0].splitn(3, '-').filter_map(|x| x.parse().ok()).collect();
        let time: Vec<u32> = parts[1].splitn(3, ':').filter_map(|x| x.parse().ok()).collect();
        if date.len() < 3 || time.len() < 3 { return None; }
        // Rough seconds-since-epoch (ignoring leap seconds and timezone).
        let y = date[0] as u64; let mo = date[1] as u64; let d = date[2] as u64;
        let h = time[0] as u64; let m  = time[1] as u64; let s_  = time[2] as u64;
        // Days since 1970-01-01 (approximate).
        let years_since_1970 = y.saturating_sub(1970);
        let days = years_since_1970 * 365 + years_since_1970 / 4
            + (mo.saturating_sub(1)) * 30 + d;
        Some(days * 86400 + h * 3600 + m * 60 + s_)
    };

    let t0 = parse(first?)?;
    let t1 = parse(last?)?;
    Some(t1.saturating_sub(t0))
}
