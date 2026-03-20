+++
title   = "Guide 05: CMS Blog"
date    = "2025-01-03"
summary = "The full system: a public blog cached in RAM, an auth-protected CMS renderer, and a publish flow that touches site.toml to go live."
tags    = ["guide", "tutorial", "cms", "custom-renderer"]
cover   = "https://picsum.photos/seed/guide05/1200/500"
+++

**Guide series:**
[Prerequisites](/blog/guide-00-prerequisites) →
[Example 01](/blog/guide-01-static-site) →
[Example 02](/blog/guide-02-blog) →
[Example 03](/blog/guide-03-contact-form) →
[Example 04](/blog/guide-04-auth) →
**Example 05 — CMS Blog** →
[Example 06](/blog/guide-06-systemd) →
[Example 07](/blog/guide-07-dev-to-production)

---

`examples/05-cms/` is the full system. A public blog served at maximum speed from the m6-http RAM cache. An auth-protected CMS renderer for creating, editing, and publishing posts. No database — posts are JSON files on disk.

```bash
cd examples/05-cms
./setup.sh           # keys, first admin user
cargo build --release -p render-cms
./dev.sh
# CMS at https://localhost:8443/cms (login required)
# Blog at https://localhost:8443/blog (public, cached)
```

## Site directory

```
05-cms/
├── site.toml
├── configs/
│   ├── system-dev.toml
│   ├── m6-html.conf
│   ├── m6-file.conf
│   ├── m6-auth.conf
│   └── render-cms.conf
├── templates/
│   ├── base.html
│   ├── home.html
│   ├── post-index.html
│   ├── post.html
│   ├── login.html
│   ├── error.html
│   └── cms/
│       ├── dashboard.html
│       └── editor.html
├── assets/
├── content/
│   ├── posts/     ← published JSON (one file per post)
│   └── drafts/    ← draft JSON (never publicly routable)
├── keys/
└── render-cms/
    ├── Cargo.toml
    └── src/main.rs
```

## `site.toml`

```toml
[site]
name   = "My Blog"
domain = "localhost"

[auth]
backend    = "m6-auth"
public_key = "keys/auth.pub"

[[backend]]
name    = "m6-html"
sockets = "/run/m6/m6-html-*.sock"

[[backend]]
name    = "m6-file"
sockets = "/run/m6/m6-file-*.sock"

[[backend]]
name    = "m6-auth"
sockets = "/run/m6/m6-auth-*.sock"

[[backend]]
name    = "render-cms"
sockets = "/run/m6/render-cms-*.sock"

# ── Public routes — no auth, fully cached ────────────────────────

[[route]]
path    = "/"
backend = "m6-html"

[[route]]
path    = "/blog"
backend = "m6-html"

[[route_group]]
glob    = "content/posts/*.json"
path    = "/blog/{stem}"
backend = "m6-html"

[[route_group]]
glob    = "assets/**/*"
path    = "/assets/{relpath}"
backend = "m6-file"

[[route]]
path    = "/_errors"
backend = "m6-html"

# ── Auth endpoints — public ──────────────────────────────────────

[[route]]
path    = "/login"
backend = "m6-html"

[[route]]
path    = "/auth/login"
backend = "m6-auth"

[[route]]
path    = "/auth/logout"
backend = "m6-auth"

[[route]]
path    = "/auth/refresh"
backend = "m6-auth"

# ── CMS — all protected, never cached ───────────────────────────

[[route]]
path    = "/cms"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/cms/edit/{stem}"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/cms/new"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/api/drafts"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/api/drafts/{stem}"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/api/publish/{stem}"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/api/unpublish/{stem}"
backend = "render-cms"
require = "group:editors"

[[route]]
path    = "/api/posts/{stem}"
backend = "render-cms"
require = "group:editors"
```

`[[route_group]]` with `content/posts/*.json` handles routing for individual posts. New post files become routable when m6-http reloads after `site.toml` is touched on publish.

## `render-cms/Cargo.toml`

```toml
[package]
name    = "render-cms"
version = "0.1.0"
edition = "2021"

[dependencies]
m6-render  = { git = "https://github.com/m6/m6", tag = "v0.1.0", features = ["auth", "disk"] }
serde_json = "1"
```

## `render-cms/src/main.rs`

```rust
use m6_render::prelude::*;
use serde_json::{json, Value};
use std::fs;

fn main() -> Result<()> {
    App::new()
        .route_get("/cms",             handle_dashboard)
        .route_get("/cms/new",         handle_new)
        .route_get("/cms/edit/{stem}", handle_edit)
        .route_post("/api/drafts",           handle_create_draft)
        .route_patch("/api/drafts/{stem}",   handle_update_draft)
        .route_post("/api/publish/{stem}",   handle_publish)
        .route_post("/api/unpublish/{stem}", handle_unpublish)
        .route_delete("/api/drafts/{stem}",  handle_delete_draft)
        .route_delete("/api/posts/{stem}",   handle_delete_post)
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
    req.touch_site_toml()?;   // triggers m6-http route reload
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
```

## CMS API

| Method | Path | Action |
|---|---|---|
| `POST` | `/api/drafts` | Create draft → returns `{"stem":"..."}` |
| `PATCH` | `/api/drafts/{stem}` | Update draft fields |
| `DELETE` | `/api/drafts/{stem}` | Delete draft permanently |
| `POST` | `/api/publish/{stem}` | Publish draft → moves to posts, touches site.toml |
| `POST` | `/api/unpublish/{stem}` | Move published post back to drafts |
| `DELETE` | `/api/posts/{stem}` | Delete published post permanently |

The dashboard shows **Edit / Delete** for drafts and **View / Edit / Unpublish / Delete** for published posts. The editor defaults the date field to today for new posts.

## The publish flow

1. Editor saves draft → `POST /api/drafts` or `PATCH /api/drafts/{stem}` writes `content/drafts/{stem}.json`
2. Editor clicks **Publish** → `POST /api/publish/{stem}`:
   - render-cms renders markdown → HTML, writes `content/posts/{stem}.json`
   - render-cms rebuilds `data/posts.json` index
   - render-cms touches `site.toml` — m6-http reloads routes, `/blog/{stem}` becomes live
   - render-cms deletes `content/drafts/{stem}.json`
3. Browser redirects to `/blog/{stem}`
4. m6-http: route now live, cache miss → m6-html renders → m6-http caches
5. Every subsequent request to `/blog/{stem}`: cache hit — RAM only

<pre class="mermaid">
sequenceDiagram
    participant Editor
    participant rcms as render-cms
    participant Disk
    participant mhttp as m6-http
    participant mhtml as m6-html
    participant Browser

    Editor->>rcms: PATCH /api/drafts/my-post (save draft)
    Editor->>rcms: POST /api/publish/my-post
    rcms->>Disk: write content/posts/my-post.json
    rcms->>Disk: touch site.toml
    Disk-->>mhttp: inotify: route reload + cache evict
    rcms->>Browser: redirect /blog/my-post
    Browser->>mhttp: GET /blog/my-post
    mhttp->>mhtml: cache miss, render
    mhtml->>mhttp: response
    mhttp->>Browser: page (now cached)
</pre>

## Performance profile

| Request type | Latency |
|---|---|
| Public post (cached) | < 1 ms — RAM only |
| Public post (cache miss) | ~5 ms — m6-html reads JSON, renders, caches |
| CMS dashboard | ~10 ms — JWT verified locally, render-cms reads posts + drafts directory |
| Publish | ~5 ms — one atomic file write, one inotify event |

The public blog path never touches disk after the first request.

## `dev.sh`

```bash
#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
trap 'kill $(jobs -p) 2>/dev/null' EXIT

m6-html    "$SITE" "$SITE/configs/m6-html.conf" &
m6-file    "$SITE" "$SITE/configs/m6-file.conf" &
m6-auth    "$SITE" "$SITE/configs/m6-auth.conf" &
"$SITE/target/release/render-cms" "$SITE" "$SITE/configs/render-cms.conf" &
m6-http    "$SITE" "$SITE/configs/system-dev.toml" &

echo "Running at https://localhost:8443"
echo "CMS at    https://localhost:8443/cms"
wait
```

---

**← [Example 04 — Login and Protected Pages](/blog/guide-04-auth)** | **[Example 06 — Production with systemd](/blog/guide-06-systemd) →**
