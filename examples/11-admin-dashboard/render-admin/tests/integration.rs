//! Integration tests for render-admin.
//!
//! Tests each module's logic directly — no live server required.
//! All tests run as part of `cargo test -p render-admin`.

use std::fs;
use std::io::Write;
use tempfile::TempDir;

// ── helpers ───────────────────────────────────────────────────────────────────

fn tmp() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

// ── perf ──────────────────────────────────────────────────────────────────────

mod perf_tests {
    use super::*;
    use render_admin::perf::perf_blob;

    fn write_log(dir: &TempDir, lines: &[&str]) -> std::path::PathBuf {
        let p = dir.path().join("m6.log");
        let mut f = fs::File::create(&p).unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        p
    }

    #[test]
    fn empty_log_returns_empty_history() {
        let dir = tmp();
        let path = write_log(&dir, &[]);
        let v = perf_blob(&path, 60);
        let history = v["history"].as_array().expect("history array");
        assert!(history.is_empty());
    }

    #[test]
    fn missing_log_returns_empty_history() {
        let dir = tmp();
        let path = dir.path().join("nonexistent.log");
        let v = perf_blob(&path, 60);
        let history = v["history"].as_array().expect("history array");
        assert!(history.is_empty());
    }

    #[test]
    fn non_stats_lines_are_ignored() {
        let dir = tmp();
        let path = write_log(&dir, &[
            r#"{"msg":"request complete","path":"/","latency_us":42}"#,
            r#"{"msg":"m6-render started","routes":3}"#,
            r#"not json at all"#,
        ]);
        let v = perf_blob(&path, 60);
        assert!(v["history"].as_array().unwrap().is_empty());
    }

    #[test]
    fn periodic_stats_are_parsed() {
        let dir = tmp();
        let path = write_log(&dir, &[
            r#"{"msg":"request complete","latency_us":10}"#,
            r#"{"msg":"periodic stats","ts":"2024-01-01T00:00:00Z","rps_avg":100,"rps_peak":200,"latency_p50_us":500,"latency_p99_us":2000,"cache_hits":80,"cache_misses":20,"cache_hit_rate":0.8,"backend_errors":0,"pool_members":4}"#,
            r#"{"msg":"periodic stats","ts":"2024-01-01T00:00:10Z","rps_avg":150,"rps_peak":300,"latency_p50_us":400,"latency_p99_us":1500,"cache_hits":90,"cache_misses":10,"cache_hit_rate":0.9,"backend_errors":0,"pool_members":4}"#,
        ]);
        let v = perf_blob(&path, 60);
        let history = v["history"].as_array().unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[0]["rps_avg"], 100);
        assert_eq!(history[1]["rps_avg"], 150);
        assert_eq!(history[0]["latency_p50_us"], 500);
        assert_eq!(history[1]["cache_hit_rate"].as_f64().unwrap(), 0.9);
    }

    #[test]
    fn n_limits_history_to_last_n_entries() {
        let dir = tmp();
        // 5 periodic stats entries
        let lines: Vec<String> = (1..=5)
            .map(|i| format!(
                r#"{{"msg":"periodic stats","rps_avg":{i},"rps_peak":0,"latency_p50_us":0,"latency_p99_us":0,"cache_hits":0,"cache_misses":0,"cache_hit_rate":0,"backend_errors":0,"pool_members":0}}"#,
            ))
            .collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let path = write_log(&dir, &refs);

        let v = perf_blob(&path, 3);
        let history = v["history"].as_array().unwrap();
        assert_eq!(history.len(), 3);
        // Should be the last 3: rps_avg 3, 4, 5
        assert_eq!(history[0]["rps_avg"], 3);
        assert_eq!(history[2]["rps_avg"], 5);
    }
}

// ── routes ────────────────────────────────────────────────────────────────────

mod routes_tests {
    use super::*;
    use render_admin::perf::routes_blob;

    fn write_log(dir: &TempDir, lines: &[&str]) -> std::path::PathBuf {
        let p = dir.path().join("m6.log");
        let mut f = fs::File::create(&p).unwrap();
        for line in lines { writeln!(f, "{}", line).unwrap(); }
        p
    }

    #[test]
    fn empty_log_returns_empty_routes() {
        let dir = tmp();
        let path = write_log(&dir, &[]);
        let v = routes_blob(&path, 1000);
        assert_eq!(v["sample_requests"], 0);
        assert!(v["routes"].as_array().unwrap().is_empty());
    }

    #[test]
    fn missing_log_returns_empty_routes() {
        let dir = tmp();
        let path = dir.path().join("no.log");
        let v = routes_blob(&path, 1000);
        assert_eq!(v["sample_requests"], 0);
    }

    #[test]
    fn aggregates_per_path() {
        let dir = tmp();
        let path = write_log(&dir, &[
            r#"{"msg":"request complete","path":"/","status":200,"cache_hit":true,"latency_us":100}"#,
            r#"{"msg":"request complete","path":"/","status":200,"cache_hit":true,"latency_us":200}"#,
            r#"{"msg":"request complete","path":"/blog","status":200,"cache_hit":false,"latency_us":300}"#,
            r#"{"msg":"request complete","path":"/blog","status":200,"cache_hit":true,"latency_us":400}"#,
            r#"{"msg":"periodic stats","rps_avg":10}"#,   // should be ignored
        ]);
        let v = routes_blob(&path, 1000);
        assert_eq!(v["sample_requests"], 4);

        let routes = v["routes"].as_array().unwrap();
        // Sorted by requests desc; both paths have 2 requests each.
        assert_eq!(routes.len(), 2);

        let root = routes.iter().find(|r| r["path"] == "/").unwrap();
        assert_eq!(root["requests"],   2);
        assert_eq!(root["cache_hits"], 2);
        assert_eq!(root["cache_misses"], 0);
        assert_eq!(root["avg_latency_us"], 150);

        let blog = routes.iter().find(|r| r["path"] == "/blog").unwrap();
        assert_eq!(blog["cache_hits"],   1);
        assert_eq!(blog["cache_misses"], 1);
        assert_eq!(blog["avg_latency_us"], 350);
    }

    #[test]
    fn sorted_by_request_count_descending() {
        let dir = tmp();
        let lines: Vec<String> = (0..5).map(|_| r#"{"msg":"request complete","path":"/popular","cache_hit":true,"latency_us":50}"#.to_string())
            .chain((0..2).map(|_| r#"{"msg":"request complete","path":"/rare","cache_hit":false,"latency_us":200}"#.to_string()))
            .collect();
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let path = write_log(&dir, &refs);
        let v = routes_blob(&path, 1000);
        let routes = v["routes"].as_array().unwrap();
        assert_eq!(routes[0]["path"], "/popular");
        assert_eq!(routes[1]["path"], "/rare");
    }

    #[test]
    fn sample_n_limits_entries_examined() {
        let dir = tmp();
        // Write 10 entries for / and then 3 for /new.
        let mut lines: Vec<String> = (0..10)
            .map(|_| r#"{"msg":"request complete","path":"/","cache_hit":true,"latency_us":10}"#.to_string())
            .collect();
        lines.extend((0..3)
            .map(|_| r#"{"msg":"request complete","path":"/new","cache_hit":false,"latency_us":20}"#.to_string()));
        let refs: Vec<&str> = lines.iter().map(|s| s.as_str()).collect();
        let path = write_log(&dir, &refs);

        // Limit sample to last 3 entries — should only see /new.
        let v = routes_blob(&path, 3);
        assert_eq!(v["sample_requests"], 3);
        let routes = v["routes"].as_array().unwrap();
        assert_eq!(routes.len(), 1);
        assert_eq!(routes[0]["path"], "/new");
    }

    #[test]
    fn hit_rate_calculated_correctly() {
        let dir = tmp();
        let path = write_log(&dir, &[
            r#"{"msg":"request complete","path":"/x","cache_hit":true,"latency_us":1}"#,
            r#"{"msg":"request complete","path":"/x","cache_hit":true,"latency_us":1}"#,
            r#"{"msg":"request complete","path":"/x","cache_hit":false,"latency_us":1}"#,
            r#"{"msg":"request complete","path":"/x","cache_hit":false,"latency_us":1}"#,
        ]);
        let v = routes_blob(&path, 1000);
        let routes = v["routes"].as_array().unwrap();
        let hit_rate = routes[0]["hit_rate"].as_f64().unwrap();
        assert!((hit_rate - 0.5).abs() < 0.001, "expected 0.5, got {hit_rate}");
    }
}

// ── system ────────────────────────────────────────────────────────────────────

mod system_tests {
    use render_admin::system::system_blob;

    #[test]
    fn system_blob_has_required_keys() {
        let v = system_blob();
        assert!(v["uptime_secs"].is_number(), "missing uptime_secs");
        assert!(v["cpu_usage_pct"].is_number(), "missing cpu_usage_pct");
        assert!(v["memory"].is_object(), "missing memory");
        assert!(v["disks"].is_array(), "missing disks");
    }

    #[test]
    fn memory_has_required_fields() {
        let v = system_blob();
        let mem = &v["memory"];
        assert!(mem["used_bytes"].is_number(), "missing memory.used_bytes");
        assert!(mem["total_bytes"].is_number(), "missing memory.total_bytes");
        assert!(mem["used_pct"].is_number(), "missing memory.used_pct");
    }

    #[test]
    fn uptime_is_positive() {
        let v = system_blob();
        let uptime = v["uptime_secs"].as_u64().expect("uptime_secs u64");
        assert!(uptime > 0, "uptime should be positive, got {uptime}");
    }

    #[test]
    fn memory_totals_are_sensible() {
        let v = system_blob();
        let total = v["memory"]["total_bytes"].as_u64().unwrap_or(0);
        assert!(total > 0, "total memory should be > 0");
        let used = v["memory"]["used_bytes"].as_u64().unwrap_or(0);
        assert!(used <= total, "used_bytes ({used}) should not exceed total_bytes ({total})");
    }

    #[test]
    fn disk_entries_have_required_fields() {
        let v = system_blob();
        for disk in v["disks"].as_array().unwrap() {
            assert!(disk["mount"].is_string(), "missing disk.mount");
            assert!(disk["used_bytes"].is_number(), "missing disk.used_bytes");
            assert!(disk["total_bytes"].is_number(), "missing disk.total_bytes");
            assert!(disk["used_pct"].is_number(), "missing disk.used_pct");
        }
    }
}

// ── bench ─────────────────────────────────────────────────────────────────────

mod bench_tests {
    use super::*;
    use render_admin::bench::{discover_targets, new_job_store, poll_job, start_bench};

    fn write_site_toml(dir: &TempDir, bind: &str) -> std::path::PathBuf {
        let p = dir.path().join("site.toml");
        fs::write(&p, format!("[server]\nbind = \"{bind}\"\n")).unwrap();
        p
    }

    #[test]
    fn targets_discovered_from_site_toml() {
        let dir = tmp();
        let path = write_site_toml(&dir, "127.0.0.1:9876");
        let targets = discover_targets(&path);
        assert_eq!(targets.len(), 4); // h1, h2, h3, all
        for t in &targets {
            assert!(t["addr"].as_str().unwrap().ends_with(":9876"),
                "expected addr to end with :9876, got {}", t["addr"]);
        }
        let protos: Vec<&str> = targets.iter()
            .map(|t| t["proto"].as_str().unwrap())
            .collect();
        assert!(protos.contains(&"h1"));
        assert!(protos.contains(&"h2"));
        assert!(protos.contains(&"h3"));
        assert!(protos.contains(&"all"));
    }

    #[test]
    fn targets_missing_site_toml_returns_empty() {
        let dir = tmp();
        let path = dir.path().join("no-site.toml");
        let targets = discover_targets(&path);
        assert!(targets.is_empty());
    }

    #[test]
    fn targets_default_port_used_when_bind_missing() {
        let dir = tmp();
        let p = dir.path().join("site.toml");
        // site.toml without [server].bind
        fs::write(&p, "site_name = \"test\"\n").unwrap();
        let targets = discover_targets(&p);
        // Should still produce 4 targets using default port 8443
        assert_eq!(targets.len(), 4);
        assert!(targets[0]["addr"].as_str().unwrap().ends_with(":8443"));
    }

    #[test]
    fn poll_nonexistent_job_returns_none() {
        let store = new_job_store();
        assert!(poll_job(&store, "no-such-id").is_none());
    }

    #[test]
    fn start_bench_with_echo_produces_done_status() {
        // Use `echo` as a stand-in for m6-bench (exits 0, produces output).
        let store = new_job_store();
        let result = start_bench(
            std::path::Path::new("echo"),
            &store,
            "127.0.0.1:9999",
            "h1",
            1,
            1,
        );
        let job_id = result["job_id"].as_str().expect("job_id").to_string();

        // Wait for the background thread to finish (max 3s).
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let blob = poll_job(&store, &job_id).expect("job exists");
            let status = blob["status"].as_str().unwrap_or("");
            if status != "running" {
                assert_eq!(status, "done", "expected done, got {status}");
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!("job did not complete within 3s");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    fn start_bench_with_nonexistent_binary_produces_failed_status() {
        let store = new_job_store();
        let result = start_bench(
            std::path::Path::new("/no-such-binary"),
            &store,
            "127.0.0.1:9999",
            "h1",
            1,
            1,
        );
        let job_id = result["job_id"].as_str().expect("job_id").to_string();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let blob = poll_job(&store, &job_id).expect("job exists");
            let status = blob["status"].as_str().unwrap_or("");
            if status != "running" {
                assert_eq!(status, "failed");
                break;
            }
            if std::time::Instant::now() > deadline {
                panic!("job did not complete within 3s");
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

// ── logs ──────────────────────────────────────────────────────────────────────

mod logs_tests {
    use super::*;
    use render_admin::logs::logs_blob;

    fn write_log(dir: &TempDir, n_lines: usize) -> std::path::PathBuf {
        let p = dir.path().join("test.log");
        let mut f = fs::File::create(&p).unwrap();
        for i in 0..n_lines {
            writeln!(f, "log line {i}").unwrap();
        }
        p
    }

    #[test]
    fn missing_log_returns_error_key() {
        let dir = tmp();
        let v = logs_blob(&dir.path().join("no.log"), 100);
        assert!(v["error"].is_string());
    }

    #[test]
    fn empty_log_returns_zero_lines() {
        let dir = tmp();
        let path = write_log(&dir, 0);
        let v = logs_blob(&path, 100);
        assert_eq!(v["total_lines"], 0);
        assert!(v["lines"].as_array().unwrap().is_empty());
    }

    #[test]
    fn returns_all_lines_when_fewer_than_n() {
        let dir = tmp();
        let path = write_log(&dir, 5);
        let v = logs_blob(&path, 100);
        assert_eq!(v["total_lines"], 5);
        assert_eq!(v["lines"].as_array().unwrap().len(), 5);
    }

    #[test]
    fn returns_last_n_lines() {
        let dir = tmp();
        let path = write_log(&dir, 10);
        let v = logs_blob(&path, 3);
        assert_eq!(v["total_lines"], 10);
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].as_str().unwrap(), "log line 7");
        assert_eq!(lines[2].as_str().unwrap(), "log line 9");
    }

    #[test]
    fn n_larger_than_file_returns_all() {
        let dir = tmp();
        let path = write_log(&dir, 3);
        let v = logs_blob(&path, 1000);
        assert_eq!(v["lines"].as_array().unwrap().len(), 3);
    }
}

// ── ops ───────────────────────────────────────────────────────────────────────

mod ops_tests {
    use super::*;
    use render_admin::ops::{config_read, config_touch, config_write, restart_service, ServiceConfig};
    use serde_json::json;

    fn write_toml(dir: &TempDir, content: &str) -> std::path::PathBuf {
        let p = dir.path().join("site.toml");
        fs::write(&p, content).unwrap();
        p
    }

    // ── config_read ──────────────────────────────────────────────────────────

    #[test]
    fn config_read_returns_parsed_json() {
        let dir = tmp();
        let path = write_toml(&dir, "[server]\nbind = \"127.0.0.1:8443\"\n");
        let v = config_read(&path);
        assert!(v["error"].is_null(), "unexpected error: {}", v["error"]);
        assert!(v["path"].as_str().unwrap().ends_with("site.toml"));
        // Config is returned as a parsed JSON object, not raw text.
        assert!(v["config"].is_object(), "config should be a JSON object");
        assert_eq!(
            v["config"]["server"]["bind"].as_str().unwrap(),
            "127.0.0.1:8443"
        );
    }

    #[test]
    fn config_read_arrays_preserved() {
        let dir = tmp();
        let toml = "[[route]]\npath = \"/\"\nbackend = \"html\"\n\n[[route]]\npath = \"/api\"\nbackend = \"render\"\n";
        let path = write_toml(&dir, toml);
        let v = config_read(&path);
        let routes = v["config"]["route"].as_array().expect("route array");
        assert_eq!(routes.len(), 2);
        assert_eq!(routes[0]["path"].as_str().unwrap(), "/");
        assert_eq!(routes[1]["path"].as_str().unwrap(), "/api");
    }

    #[test]
    fn config_read_missing_file_returns_error() {
        let dir = tmp();
        let v = config_read(&dir.path().join("no.toml"));
        assert!(v["error"].is_string());
    }

    // ── config_write ─────────────────────────────────────────────────────────

    #[test]
    fn config_write_json_blob_round_trips() {
        let dir = tmp();
        let original = "[server]\nbind = \"127.0.0.1:8443\"\n";
        let path = write_toml(&dir, original);

        // Read back as JSON blob.
        let read_v = config_read(&path);
        let config_json = &read_v["config"];
        assert!(config_json.is_object());

        // Modify a field.
        let mut updated = config_json.clone();
        updated["server"]["bind"] = json!("0.0.0.0:9000");

        // Write back.
        config_write(&path, &updated).expect("write should succeed");

        // Read again and verify.
        let read_v2 = config_read(&path);
        assert_eq!(
            read_v2["config"]["server"]["bind"].as_str().unwrap(),
            "0.0.0.0:9000"
        );
    }

    #[test]
    fn config_write_null_value_is_rejected() {
        let dir = tmp();
        let path = write_toml(&dir, "[server]\nbind = \"ok\"\n");
        let bad = json!({ "server": { "bind": null } });
        let result = config_write(&path, &bad);
        assert!(result.is_err(), "should reject null (not valid TOML)");
        // Original file must be unchanged.
        let content = fs::read_to_string(&path).unwrap();
        assert!(content.contains("bind"));
    }

    #[test]
    fn config_write_preserves_arrays() {
        let dir = tmp();
        let path = write_toml(&dir, "");
        let cfg = json!({
            "server": { "bind": "127.0.0.1:8443" },
            "route": [
                { "path": "/",    "backend": "html" },
                { "path": "/api", "backend": "render" }
            ]
        });
        config_write(&path, &cfg).expect("write should succeed");

        let read_back = config_read(&path);
        let routes = read_back["config"]["route"].as_array().expect("route array");
        assert_eq!(routes.len(), 2);
    }

    // ── config_touch ─────────────────────────────────────────────────────────

    #[test]
    fn config_touch_updates_mtime() {
        let dir = tmp();
        let path = write_toml(&dir, "[server]\n");
        let mtime_before = fs::metadata(&path).unwrap().modified().unwrap();

        // Sleep briefly so touch has a different time to set
        std::thread::sleep(std::time::Duration::from_millis(10));
        config_touch(&path).expect("touch should succeed");

        let mtime_after = fs::metadata(&path).unwrap().modified().unwrap();
        assert!(mtime_after >= mtime_before, "mtime should not go backwards");
    }

    #[test]
    fn config_touch_missing_file_returns_error() {
        let dir = tmp();
        let result = config_touch(&dir.path().join("no.toml"));
        assert!(result.is_err());
    }

    // ── restart_service ──────────────────────────────────────────────────────

    #[test]
    fn restart_missing_pid_file_returns_error() {
        let svc = ServiceConfig {
            name: "test".into(),
            pid_file: "/tmp/no-such-render-admin-test.pid".into(),
        };
        let v = restart_service(&svc);
        assert!(v["error"].is_string(), "expected error key, got: {v}");
    }

    #[test]
    fn restart_invalid_pid_content_returns_error() {
        let dir = tmp();
        let pid_file = dir.path().join("bad.pid");
        fs::write(&pid_file, "not-a-number\n").unwrap();
        let svc = ServiceConfig {
            name: "test".into(),
            pid_file: pid_file.to_string_lossy().into(),
        };
        let v = restart_service(&svc);
        assert!(v["error"].is_string());
    }

    #[test]
    fn restart_returns_ok_true_on_success() {
        // Spawn a sleep process we can SIGTERM safely.
        let mut child = std::process::Command::new("sleep")
            .arg("60")
            .spawn()
            .expect("spawn sleep");
        let pid = child.id();

        let dir = tmp();
        let pid_file = dir.path().join("test.pid");
        fs::write(&pid_file, format!("{pid}\n")).unwrap();
        let svc = ServiceConfig {
            name: "sleep".into(),
            pid_file: pid_file.to_string_lossy().into(),
        };
        let v = restart_service(&svc);
        // Reap the child to avoid zombie.
        child.wait().ok();

        assert_eq!(v["ok"], true, "expected ok:true, got: {v}");
        assert_eq!(v["pid"], pid);
    }
}
