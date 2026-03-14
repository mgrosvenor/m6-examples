use m6_render::prelude::*;
use serde_json::{json, Value};
use std::fs;

fn main() -> Result<()> {
    // render-cms uses JSON files directly — no state needed.
    // A real CMS would use App::with_state(init_global, init_thread)
    // to hold a database connection per thread.
    App::new()
        .route_get("/cms",             handle_dashboard)
        .route_get("/cms/new",         handle_new)
        .route_get("/cms/edit/{stem}", handle_edit)
        .route_post("/api/drafts",           handle_create_draft)
        .route_patch("/api/drafts/{stem}",   handle_update_draft)
        .route_post("/api/publish/{stem}",   handle_publish)
        .route_post("/api/unpublish/{stem}", handle_unpublish)
        .run()
}

fn handle_dashboard(req: &Request) -> Result<Response> {
    let author    = req["auth_username"].as_str().unwrap_or("");
    let drafts    = req.list_json("content/drafts/")?;
    let published = req.list_json("content/posts/")?;
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
    let post = req.read_json(&format!("content/drafts/{}.json", stem))
        .or_else(|_| req.read_json(&format!("content/posts/{}.json", stem)))?;
    Response::render_with("templates/cms/editor.html", req, post)
}

fn handle_create_draft(req: &Request) -> Result<Response> {
    let body: Value = req.body_json()?;
    let stem  = req["title"].as_str().unwrap_or("untitled").to_slug();
    let draft = json!({
        "stem":   stem,
        "title":  body["title"],
        "body":   body["body"],
        "author": req["auth_username"],
        "date":   today_iso8601(),
    });
    req.write_json(&format!("content/drafts/{}.json", stem), &draft)?;
    Response::json_status(json!({"stem": stem}), 201)
}

fn handle_update_draft(req: &Request) -> Result<Response> {
    let stem = req["stem"].as_str().ok_or(Error::NotFound)?;
    let body: Value = req.body_json()?;
    let path = format!("content/drafts/{}.json", stem);
    let mut draft = req.read_json(&path)?;
    if let Some(t) = body.get("title") { draft["title"] = t.clone(); }
    if let Some(b) = body.get("body")  { draft["body"]  = b.clone(); }
    req.write_json(&path, &draft)?;
    Response::json(json!({"ok": true}))
}

fn handle_publish(req: &Request) -> Result<Response> {
    let stem         = req["stem"].as_str().ok_or(Error::NotFound)?;
    let draft_path   = format!("content/drafts/{}.json", stem);
    let publish_path = format!("content/posts/{}.json", stem);

    let mut post = req.read_json(&draft_path)?;
    post["stem"]         = json!(stem);
    post["path"]         = json!(format!("/blog/{}", stem));
    post["published_at"] = json!(now_iso8601());

    req.write_json_atomic(&publish_path, &post)?;
    update_index(req)?;
    req.touch_site_toml()?;
    let _ = fs::remove_file(req.site_path(&draft_path));

    Response::json(json!({"published": true, "path": post["path"]}))
}

fn handle_unpublish(req: &Request) -> Result<Response> {
    let stem         = req["stem"].as_str().ok_or(Error::NotFound)?;
    let publish_path = format!("content/posts/{}.json", stem);
    let draft_path   = format!("content/drafts/{}.json", stem);

    let mut post = req.read_json(&publish_path)?;
    post["draft"] = json!(true);
    req.write_json(&draft_path, &post)?;
    fs::remove_file(req.site_path(&publish_path))?;
    update_index(req)?;
    req.touch_site_toml()?;

    Response::json(json!({"unpublished": true}))
}

fn update_index(req: &Request) -> Result<()> {
    let posts = req.list_json("content/posts/")?;
    let index = json!({ "documents": posts });
    req.write_json_atomic("data/posts.json", &index)
}
