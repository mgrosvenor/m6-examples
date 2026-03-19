+++
title   = "Guide 03: Contact Form with a Custom Renderer"
date    = "2025-01-05"
summary = "Write your first custom renderer using m6-render. Handle a GET/POST contact form and send email via SMTP."
tags    = ["guide", "tutorial", "m6-render", "custom-renderer"]
cover   = "https://picsum.photos/seed/guide03/1200/500"
+++

**Guide series:**
[Prerequisites](/blog/guide-00-prerequisites) →
[Example 01](/blog/guide-01-static-site) →
[Example 02](/blog/guide-02-blog) →
**Example 03 — Contact Form** →
[Example 04](/blog/guide-04-auth) →
[Example 05](/blog/guide-05-cms) →
[Example 06](/blog/guide-06-systemd) →
[Example 07](/blog/guide-07-dev-to-production)

---

`examples/03-contact/` introduces custom renderers. `render-contact` handles GET (renders the form) and POST (sends email via SMTP). Everything else — routing, caching, TLS — stays in the m6 layer.

```bash
cd examples/03-contact
cargo build --release -p render-contact
./dev.sh
```

## What's a custom renderer?

<pre class="mermaid">
flowchart LR
    Browser --"HTTPS"--> mhttp["m6-http"]
    mhttp --"/contact GET"--> mhtml["m6-html\n(template render)"]
    mhttp --"/contact POST"--> rc["render-contact\n(SMTP send)"]
    rc --"email"--> SMTP[("SMTP server")]
</pre>

Any HTTP/1.1 server that listens on a Unix socket. m6-http discovers it via inotify, adds it to the backend pool, and routes requests to it. The m6-render library handles the socket and HTTP plumbing — you write handler functions.

## `render-contact/Cargo.toml`

```toml
[package]
name    = "render-contact"
version = "0.1.0"
edition = "2021"

[dependencies]
m6-render  = { git = "https://github.com/m6/m6", tag = "v0.1.0", features = ["smtp"] }
serde_json = "1"
```

## `render-contact/src/main.rs`

```rust
use m6_render::prelude::*;

// SmtpTransport is Send+Sync — safe to share across threads
struct Global {
    mailer: SmtpTransport,
}

fn init_global(config: &Map<String, Value>) -> Result<Global> {
    Ok(Global {
        mailer: SmtpTransport::builder(config["smtp"]["host"].as_str()?)
            .port(config["smtp"]["port"].as_u64()? as u16)
            .credentials(
                config["smtp"]["username"].as_str()?,
                config["smtp"]["password"].as_str()?,
            )
            .build()?,
    })
}

fn handle_post(req: &Request, global: &Global, _local: &mut ()) -> Result<Response> {
    let name    = req.field("name")?;
    let email   = req.field("email")?;
    let message = req.field("message")?;

    global.mailer.send(
        Message::builder()
            .from(req["smtp"]["from"].as_str()?.parse()?)
            .to(req["smtp"]["to"].as_str()?.parse()?)
            .subject(format!("Contact from {}", name))
            .body(format!("From: {} <{}>\n\n{}", name, email, message))?,
    )?;

    Response::render_with("templates/contact.html", req, json!({"sent": true, "name": name}))
}

fn main() -> Result<()> {
    App::with_global(init_global)
        .route_post("/contact", handle_post)
        // GET /contact served by the framework default (template render from config)
        .run()
}
```

`App::with_global` initialises shared state once at startup — the `SmtpTransport` connection pool lives here. Handler functions receive `&Global` (shared) and `&mut Local` (per-thread, unused here). The framework dispatches methods: `route_post` registers a POST handler; GET falls back to the config-driven template render.

## `configs/render-contact.conf`

```toml
global_params = ["data/site.json"]
secrets_file  = "/etc/m6/render-contact.toml"

[[route]]
path     = "/contact"
template = "templates/contact.html"
params   = []
cache    = "no-store"

# Development defaults — overridden by secrets_file in production
[smtp]
host     = "localhost"
port     = 1025
username = ""
password = ""
from     = "noreply@example.com"
to       = "owner@example.com"
```

`secrets_file` is merged in if the file exists. If absent (on dev machines), it's silently ignored and the `[smtp]` block below provides defaults. The file path is safe to commit — it contains no secrets itself.

Production secrets file (on the server, not in version control):

```toml
# /etc/m6/render-contact.toml
[smtp]
host     = "smtp.example.com"
port     = 587
username = "user@example.com"
password = "live-smtp-password"
```

## `templates/contact.html`

```html
{% extends "base.html" %}
{% block title %}Contact · {{ site_name }}{% endblock %}
{% block content %}
  <h1>Contact</h1>
  {% if sent %}
    <p>Message sent. Thanks, {{ name }}!</p>
  {% else %}
    <form method="post" action="/contact">
      <label>Name    <input type="text"  name="name"    required></label>
      <label>Email   <input type="email" name="email"   required></label>
      <label>Message <textarea           name="message" required></textarea></label>
      <button type="submit">Send</button>
    </form>
  {% endif %}
{% endblock %}
```

Same template handles both states — `sent` is absent on GET, `true` after a successful POST.

## New entries in `site.toml`

```toml
[[backend]]
name    = "render-contact"
sockets = "/run/m6/render-contact-*.sock"

[[route]]
path    = "/contact"
backend = "render-contact"
```

The socket name is derived from the config filename: `configs/render-contact.conf` → `/run/m6/render-contact.sock`.

## Updated `dev.sh`

```bash
./target/release/render-contact "$SITE" "$SITE/configs/render-contact.conf" &
```

Added alongside the existing m6-html, m6-file, and m6-http lines. The renderer starts, creates its socket, and m6-http discovers it via inotify.

## How App::with_global works

The framework initialises global state once on startup:

1. Loads config (renderer conf + secrets overlay)
2. Calls `init_global(&config)` — runs once, result stored in `Arc`
3. Spawns N worker threads, each calling `init_local()` (default: no-op, returns `()`)
4. Worker threads wait for connections on the Unix socket
5. For each request: route match → call handler with `(req, &global, &mut local)`

This gives you zero-overhead shared access to immutable state (connection pools, compiled regexes, loaded keys) and mutable per-thread state (database connections, buffers) without any locking on the hot path.

---

**← [Example 02 — Blog with m6-md](/blog/guide-02-blog)** | **[Example 04 — Login and Protected Pages](/blog/guide-04-auth) →**
