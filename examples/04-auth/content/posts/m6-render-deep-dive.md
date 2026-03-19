+++
title   = "m6-html and m6-render: Templates, Params, and Custom Renderers"
date    = "2025-02-28"
summary = "The renderer framework that makes m6-html a one-liner — and how to use the same library to write custom backends in Rust."
tags    = ["m6-html", "m6-render", "templates", "internals"]
cover   = "https://picsum.photos/seed/m6render/1200/500"
+++

m6-html's entire application logic is a single function call:

```rust
fn main() -> Result<()> {
    App::new().run()
}
```

Everything else — route matching, params loading, key merging, template rendering, compression, error handling, logging — is the m6-render framework. Understanding the framework tells you everything about how m6-html works, and also how to write your own custom renderer for anything m6-html can't do on its own.

## How m6-html renders a request

<pre class="mermaid">
flowchart LR
    Req["Request"] --> PathMatch["Path\nmatching"]
    PathMatch --> ParamsLoad["Load params\nfiles"]
    ParamsLoad --> Merge["Merge global\nparams"]
    Merge --> Render["Render Tera\ntemplate"]
    Render --> Compress["Compress\n(br/gzip)"]
    Compress --> Resp["Response"]
</pre>

1. **Path matching** — the incoming path is matched against the routes declared in `m6-html.conf`. The first match wins. Parameterised paths extract named values (e.g. `{stem}` from `/blog/hello-world`).

2. **Params loading** — each matched route lists `params` files. Path param values substitute into file paths: `/blog/{stem}` with `stem=hello-world` loads `content/posts/hello-world.json`. All params files are loaded and merged into a single JSON object.

3. **Global params merge** — the `global_params` list at the top of the config (typically `data/site.json`) is merged in first. Route params overlay global params. Path params (from the URL) are added as top-level keys.

4. **Template render** — the merged JSON object is passed to Tera as the context. The template specified in the route config is rendered.

5. **Compression** — the rendered response is compressed per `Accept-Encoding` and per-MIME compression config. Brotli and gzip are supported.

6. **Response** — status, headers, body sent back over the Unix socket.

## Config

```toml
# configs/m6-html.conf

global_params = ["data/site.json"]

[[route]]
path     = "/"
template = "templates/home.html"
params   = ["data/home.json"]

[[route]]
path     = "/blog/{stem}"
template = "templates/post.html"
params   = ["data/posts.json"]

[[route]]
path     = "/_errors"
template = "templates/error.html"
params   = []
cache    = "no-store"
```

The `cache` key on a route sets the `Cache-Control` header on responses. Default is `public` for rendered routes. Use `no-store` for the error route — you don't want a cached 404 page to be served for a valid path.

## Tera templates

m6-html uses [Tera](https://keats.github.io/tera/), a template engine inspired by Jinja2. Template inheritance via `{% extends %}` and `{% block %}` works exactly as you'd expect.

One Tera behaviour worth knowing: `{% set %}` in a child template is silently discarded unless it's inside a `{% block %}`. If you need a derived variable in multiple blocks, set it independently in each block. This differs from Jinja2.

```html
{% block title %}
  {%- set post = documents | filter(attribute="stem", value=stem) | first -%}
  {{ post.title }} · {{ site_name }}
{% endblock %}

{% block content %}
  {% set post = documents | filter(attribute="stem", value=stem) | first %}
  <h1>{{ post.title }}</h1>
  {{ post.body | safe }}
{% endblock %}
```

The `safe` filter marks the value as trusted HTML — necessary for rendered Markdown bodies.

## Custom renderers with m6-render

Any route that requires logic beyond "render this template with this data" needs a custom renderer. The m6-render library handles all the socket/HTTP plumbing; you write handler functions.

A minimal custom renderer:

```rust
use m6_render::{App, Request, Response};

fn handle(req: &Request) -> Response {
    let name = req.path_param("name").unwrap_or("world");
    Response::html(200, format!("<h1>Hello, {name}!</h1>"))
}

fn main() -> anyhow::Result<()> {
    App::new()
        .route("/hello/{name}", handle)
        .run()
}
```

The `Request` type gives you access to path params, query params, headers, and the request body. The `Response` type covers the common cases: HTML, JSON, redirect, file. For streaming or complex cases, you can construct a raw response.

The socket naming convention is the same as m6-html: the config filename determines the socket path. `configs/my-renderer.conf` → `/run/m6/my-renderer.sock`. Declare it as a backend in `site.toml`:

```toml
[[backend]]
name    = "my-renderer"
sockets = "/run/m6/my-renderer*.sock"
```

And route to it:

```toml
[[route]]
path    = "/api/greet/{name}"
backend = "my-renderer"
```

## m6-file

m6-file serves files from the filesystem. It shares the socket naming convention and CLI interface with m6-html, but doesn't use the m6-render library internally — there are no templates, no params, no thread pool.

Configuration is straightforward:

```toml
[[route]]
path = "/assets/{relpath}"
root = "assets/"
```

`GET /assets/css/main.css` → resolves to `assets/css/main.css` relative to the site directory. Path traversal is blocked: resolved paths must stay within `root`, `..` sequences return 404, and symlinks outside root are not followed.

All file responses are `Cache-Control: public`, which means m6-http caches them. In practice, your CSS and JS are served from memory after the first request.

## Compression

Both m6-html and m6-file compress responses per `Accept-Encoding` and per-MIME config. The defaults are sensible — text, HTML, CSS, JS compressed; images and fonts not. Override per MIME type:

```toml
[compression]
"text/html"  = { brotli = 6, gzip = 6 }
"image/jpeg" = { brotli = 0, gzip = 0 }
```

Compression level 0 means disabled for that MIME type. The compression decision is made once; subsequent requests for the same path and encoding are served from the m6-http cache.

---

**← [m6-http Deep Dive](/blog/m6-http-deep-dive)** | **[Guide: Prerequisites](/blog/guide-00-prerequisites) →**
