use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use m6_render::prelude::*;
use serde_json::{json, Value};
use std::fs;

fn main() -> Result<()> {
    App::new()
        .route_get("/contact",               handle_contact_get)
        .route_post("/contact",              handle_contact_post)
        .route_get("/cms",                   handle_dashboard)
        .route_get("/cms/new",               handle_new)
        .route_get("/cms/edit/{stem}",       handle_edit)
        .route_post("/api/drafts",           handle_create_draft)
        .route_patch("/api/drafts/{stem}",   handle_update_draft)
        .route_post("/api/publish/{stem}",   handle_publish)
        .route_post("/api/unpublish/{stem}", handle_unpublish)
        .run()
}

fn handle_contact_get(req: &Request) -> Result<Response> {
    Response::render("templates/contact.html", req)
}

fn handle_contact_post(req: &Request) -> Result<Response> {
    let name = req.field("name")?;
    Response::render_with("templates/contact.html", req, json!({
        "sent": true,
        "name": name,
    }))
}

fn handle_dashboard(req: &Request) -> Result<Response> {
    let author    = req["auth_username"].as_str().unwrap_or("");
    let drafts    = req.list_json("content/drafts/")?;
    let index     = req.read_json("data/posts.json").unwrap_or_else(|_| json!({"documents": []}));
    let published = index["documents"].as_array().cloned().unwrap_or_default();
    Response::render_with("templates/cms/dashboard.html", req, json!({
        "drafts":    drafts,
        "published": published,
        "author":    author,
    }))
}

fn handle_new(req: &Request) -> Result<Response> {
    Response::render_with("templates/cms/editor.html", req, json!({"new": true}))
}

fn handle_edit(req: &Request) -> Result<Response> {
    let stem = req["stem"].as_str().ok_or(Error::NotFound)?;
    let mut post = req.read_json(&format!("content/drafts/{}.json", stem))
        .or_else(|_| req.read_json(&format!("content/posts/{}.json", stem)))
        .or_else(|_| {
            let index = req.read_json("data/posts.json")?;
            index["documents"].as_array()
                .and_then(|docs| docs.iter().find(|d| d["stem"].as_str() == Some(stem)).cloned())
                .ok_or(Error::NotFound)
        })?;

    // Prefer raw markdown body for editing
    if let Some(md) = post.get("body_md").and_then(|v| v.as_str()) {
        post["body"] = json!(md);
    } else if post["body"].as_str().map(|b| b.trim_start().starts_with('<')).unwrap_or(false) {
        // Load raw .md source for markdown-sourced posts
        let md_path = req.site_path(&format!("content/posts/{}.md", stem));
        if let Ok(raw) = std::fs::read_to_string(&md_path) {
            post["body"] = json!(strip_frontmatter(&raw));
        }
    }

    // Normalise tags to a comma-separated string for the editor input
    let tags_str = match post.get("tags") {
        Some(Value::Array(arr)) => arr.iter()
            .filter_map(|t| t.as_str())
            .collect::<Vec<_>>()
            .join(", "),
        Some(Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    post["tags_str"] = json!(tags_str);

    // Base64-encode so </textarea> in content can't break the hidden textarea transport
    let body_b64 = B64.encode(post["body"].as_str().unwrap_or(""));
    let tags_b64 = B64.encode(&tags_str);
    post["body_b64"] = json!(body_b64);
    post["tags_b64"] = json!(tags_b64);

    Response::render_with("templates/cms/editor.html", req, post)
}

fn handle_create_draft(req: &Request) -> Result<Response> {
    let body: Value = req.body_json()?;
    let stem = slugify(body["title"].as_str().unwrap_or("untitled"));
    let mut draft = json!({
        "stem":   stem,
        "author": req["auth_username"],
        "date":   today_iso8601(),
    });
    for field in &["title", "body", "summary", "tags", "date", "cover"] {
        if let Some(v) = body.get(field) { draft[field] = v.clone(); }
    }
    req.write_json(&format!("content/drafts/{}.json", stem), &draft)?;
    Ok(Response::json_status(json!({"stem": stem}), 201))
}

fn handle_update_draft(req: &Request) -> Result<Response> {
    let stem = req["stem"].as_str().ok_or(Error::NotFound)?;
    let body: Value = req.body_json()?;
    let path = format!("content/drafts/{}.json", stem);
    let mut draft = req.read_json(&path).unwrap_or_else(|_| json!({"stem": stem}));
    for field in &["title", "body", "summary", "tags", "date", "cover"] {
        if let Some(v) = body.get(field) { draft[field] = v.clone(); }
    }
    req.write_json(&path, &draft)?;
    Ok(Response::json(json!({"ok": true})))
}

fn handle_publish(req: &Request) -> Result<Response> {
    let stem         = req["stem"].as_str().ok_or(Error::NotFound)?;
    let draft_path   = format!("content/drafts/{}.json", stem);
    let publish_path = format!("content/posts/{}.json", stem);

    let mut post = req.read_json(&draft_path)?;

    // Render markdown body to HTML for the blog; preserve raw markdown in body_md
    if let Some(md) = post["body"].as_str() {
        let html = comrak::markdown_to_html(md, &comrak::Options::default());
        post["body_md"] = post["body"].clone();
        post["body"]    = json!(html);
    }

    post["stem"]         = json!(stem);
    post["path"]         = json!(format!("/blog/{}", stem));
    post["published_at"] = json!(now_iso8601());

    req.write_json_atomic(&publish_path, &post)?;
    update_index(req)?;
    req.touch("site.toml")?;
    let _ = fs::remove_file(req.site_path(&draft_path));

    Ok(Response::json(json!({"published": true, "path": post["path"]})))
}

fn handle_unpublish(req: &Request) -> Result<Response> {
    let stem         = req["stem"].as_str().ok_or(Error::NotFound)?;
    let publish_path = format!("content/posts/{}.json", stem);
    let draft_path   = format!("content/drafts/{}.json", stem);

    let mut post = req.read_json(&publish_path)?;
    post["draft"] = json!(true);
    req.write_json(&draft_path, &post)?;
    let _ = fs::remove_file(req.site_path(&publish_path));
    update_index(req)?;
    req.touch("site.toml")?;

    Ok(Response::json(json!({"unpublished": true})))
}

fn update_index(req: &Request) -> Result<()> {
    // CMS-managed posts (.json files written by the publish handler).
    let cms_posts = req.list_json("content/posts/")?;
    let cms_stems: std::collections::HashSet<&str> = cms_posts.iter()
        .filter_map(|p| p["stem"].as_str())
        .collect();

    // Preserve existing index entries that are NOT managed by the CMS
    // (i.e. .md-sourced posts indexed by m6-md).
    let existing = req.read_json("data/posts.json")
        .unwrap_or_else(|_| json!({"documents": []}));
    let md_posts: Vec<Value> = existing["documents"].as_array()
        .map(|docs| docs.iter()
            .filter(|d| !cms_stems.contains(d["stem"].as_str().unwrap_or("")))
            .cloned()
            .collect())
        .unwrap_or_default();

    // Merge and sort by date descending.
    let mut all: Vec<Value> = md_posts.into_iter().chain(cms_posts).collect();
    all.sort_by(|a, b| {
        b["date"].as_str().unwrap_or("").cmp(a["date"].as_str().unwrap_or(""))
    });

    req.write_json_atomic("data/posts.json", &json!({ "documents": all }))
}

/// Strip YAML (`---`) or TOML (`+++`) frontmatter from a markdown file,
/// returning only the body text.
fn strip_frontmatter(text: &str) -> &str {
    for delim in &["---", "+++"] {
        if let Some(rest) = text.strip_prefix(delim) {
            let nl_delim = format!("\n{}", delim);
            if let Some(end) = rest.find(nl_delim.as_str()) {
                return rest[end + nl_delim.len()..].trim_start_matches('\n');
            }
        }
    }
    text
}
