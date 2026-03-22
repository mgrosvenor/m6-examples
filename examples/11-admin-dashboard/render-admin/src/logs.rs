use serde_json::{json, Value};
use std::fs;
use std::path::Path;

/// Returns a JSON blob with the last `n` log lines:
/// { "lines": [ "...", "..." ], "total_lines": 1234 }
pub fn logs_blob(log_path: &Path, n: usize) -> Value {
    let text = match fs::read_to_string(log_path) {
        Ok(t) => t,
        Err(e) => {
            return json!({ "error": format!("cannot read log: {e}"), "lines": [], "total_lines": 0 })
        }
    };

    let all_lines: Vec<&str> = text.lines().collect();
    let total = all_lines.len();
    let start = total.saturating_sub(n);
    let lines: Vec<&str> = all_lines[start..].to_vec();

    json!({ "lines": lines, "total_lines": total })
}
