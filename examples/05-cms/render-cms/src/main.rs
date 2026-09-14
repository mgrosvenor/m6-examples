use base64::{Engine as _, engine::general_purpose::STANDARD as B64};
use m6_core::prelude::*;
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
        .route_delete("/api/drafts/{stem}",  handle_delete_draft)
        .route_delete("/api/posts/{stem}",   handle_delete_post)
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
    // `unwrap_or_default`, not `?`. A FRESH CLONE HAS NO content/drafts/: git does
    // not track empty directories, so the dashboard 500'd on a brand-new checkout
    // until somebody happened to create a draft. `list_json` returns an error for a
    // directory that does not exist, and the `?` turned that into a 500 on the one
    // page a new reader opens first.
    //
    // No drafts and no drafts directory are the same thing to a reader, so they
    // should render the same page. `cms_owned_stems` below already treats a missing
    // directory as owning nothing; this call site was simply missed.
    let drafts    = req.list_json("content/drafts/").unwrap_or_default();
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

/// Every stem this CMS owns, published or not, taken from the file names in both
/// content directories rather than from the files' contents.
///
/// `content/posts/{stem}.json` and `content/drafts/{stem}.json` are named by
/// stem by construction, so the name is the reliable fact. Reading `["stem"]`
/// out of the JSON would also work for published posts but not for a draft that
/// has never been published, and a missing field would silently drop the entry
/// from this set, which is the failure this function exists to prevent.
fn cms_owned_stems(req: &Request) -> std::collections::HashSet<String> {
    let mut stems = std::collections::HashSet::new();
    for dir in ["content/posts/", "content/drafts/"] {
        let path = req.site_path(dir);
        let Ok(entries) = fs::read_dir(&path) else {
            continue; // a directory that does not exist owns nothing
        };
        for entry in entries.flatten() {
            let p = entry.path();
            if p.extension().and_then(|e| e.to_str()) == Some("json") {
                if let Some(stem) = p.file_stem().and_then(|s| s.to_str()) {
                    stems.insert(stem.to_string());
                }
            }
        }
    }
    stems
}

/// Rebuild `data/posts.json` from the CMS's published posts plus whatever
/// m6-md put there from markdown.
///
/// ## Why ownership is not inferred from the surviving files
///
/// This used to work out which stems belonged to the CMS by reading
/// `content/posts/`, the published directory, and preserving every index entry
/// whose stem was not in it. **That made unpublishing do nothing at all.**
///
/// `handle_unpublish` deletes `content/posts/{stem}.json` and then calls this.
/// By that point the stem is no longer in the published directory, so the old
/// code classified it as markdown-sourced and carefully preserved the entry it
/// was supposed to be removing. The API answered `{"unpublished": true}`, the
/// post stayed in the index, stayed listed on `/blog`, and stayed readable at
/// its own URL. Nothing reported a problem, and the example's own test suite
/// passed it, because the test only checked that the response said "unpublished"
/// and never asked whether the post had gone.
///
/// A post's owner cannot be deduced from where it currently is, so ownership
/// comes from `cms_owned_stems`, which counts drafts as CMS-owned too. An
/// unpublished post has a draft file, so it is owned, so the stale index entry
/// is dropped and not re-added.
fn update_index(req: &Request) -> Result<()> {
    let cms_posts = req.list_json("content/posts/")?;
    let owned = cms_owned_stems(req);

    // Preserve existing index entries this CMS does not own: the .md-sourced
    // posts m6-md indexes.
    let existing = req.read_json("data/posts.json")
        .unwrap_or_else(|_| json!({"documents": []}));
    let md_posts: Vec<Value> = existing["documents"].as_array()
        .map(|docs| docs.iter()
            .filter(|d| !owned.contains(d["stem"].as_str().unwrap_or("")))
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

fn handle_delete_draft(req: &Request) -> Result<Response> {
    let stem = req["stem"].as_str().ok_or(Error::NotFound)?;
    let path = req.site_path(&format!("content/drafts/{}.json", stem));
    if path.exists() {
        fs::remove_file(&path).map_err(|e| Error::Other(e.into()))?;
    }
    Ok(Response::json(json!({"deleted": true})))
}

fn handle_delete_post(req: &Request) -> Result<Response> {
    let stem = req["stem"].as_str().ok_or(Error::NotFound)?;
    let path = req.site_path(&format!("content/posts/{}.json", stem));
    if path.exists() {
        fs::remove_file(&path).map_err(|e| Error::Other(e.into()))?;
        update_index(req)?;
        req.touch("site.toml")?;
    }
    Ok(Response::json(json!({"deleted": true})))
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
