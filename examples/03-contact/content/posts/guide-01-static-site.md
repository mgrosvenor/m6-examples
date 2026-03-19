+++
title   = "Guide 01: Static Site"
date    = "2025-01-07"
summary = "The base case: templates, assets, and hand-authored JSON. No build step, no custom renderers. Three processes, one config each."
tags    = ["guide", "tutorial"]
cover   = "https://picsum.photos/seed/guide01/1200/500"
+++

**Guide series:**
[Prerequisites](/blog/guide-00-prerequisites) →
**Example 01 — Static Site** →
[Example 02](/blog/guide-02-blog) →
[Example 03](/blog/guide-03-contact-form) →
[Example 04](/blog/guide-04-auth) →
[Example 05](/blog/guide-05-cms) →
[Example 06](/blog/guide-06-systemd) →
[Example 07](/blog/guide-07-dev-to-production)

---

`examples/01-static/` is the base case. Templates, assets, and hand-authored JSON. No build step, no custom renderers. Three processes, one config each.

```bash
cd examples/01-static
./dev.sh
# open https://localhost:8443
```

## Site directory

```
01-static/
├── site.toml
├── configs/
│   ├── m6-html.conf
│   └── m6-file.conf
├── templates/
│   ├── base.html
│   ├── home.html
│   ├── page.html
│   └── error.html
├── assets/
│   └── style.css
└── data/
    └── site.json
```

No binaries. No generated files. Everything you see is what you deploy.

## `site.toml`

<pre class="mermaid">
flowchart LR
    Browser --"HTTPS :8443"--> mhttp["m6-http"]
    mhttp --"/assets/*"--> mfile["m6-file\n(m6-file.conf)"]
    mhttp --"/, /about"--> mhtml["m6-html\n(m6-html.conf)"]
</pre>

```toml
[site]
name   = "My Site"
domain = "localhost"

[server]
bind     = "127.0.0.1:8443"
tls_cert = "../../localhost.pem"
tls_key  = "../../localhost-key.pem"

[errors]
mode = "internal"

[log]
level  = "info"
format = "text"

[[backend]]
name    = "m6-html"
sockets = "/run/m6/m6-html-*.sock"

[[backend]]
name    = "m6-file"
sockets = "/run/m6/m6-file-*.sock"

[[route]]
path    = "/"
backend = "m6-html"

[[route]]
path    = "/about"
backend = "m6-html"

[[route]]
path    = "/_errors"
backend = "m6-html"

[[route_group]]
glob    = "assets/**/*"
path    = "/assets/{relpath}"
backend = "m6-file"
```

`site.toml` declares backends and routes. m6-http reads it. m6-html and m6-file have their own configs (below) that further describe how to handle each route on their side.

The `[[route_group]]` glob expands at startup: every file under `assets/` becomes a routable URL. `assets/style.css` → `/assets/style.css`. No individual route declarations needed.

## `configs/m6-html.conf`

```toml
global_params = ["data/site.json"]

[[route]]
path     = "/"
template = "templates/home.html"
params   = []

[[route]]
path     = "/about"
template = "templates/page.html"
params   = ["data/about.json"]

[[route]]
path     = "/_errors"
template = "templates/error.html"
params   = []
cache    = "no-store"
```

`global_params` are loaded for every request and merged into the template context first. Route `params` are loaded next and overlay globals. The template sees the merged result.

`cache = "no-store"` on the error route ensures m6-http never caches a 404 page and then serves it for a valid path.

## `configs/m6-file.conf`

```toml
[[route]]
path = "/assets/{relpath}"
root = "assets/"
```

`GET /assets/css/main.css` → `root = "assets/"`, `relpath = "css/main.css"` → resolves to `assets/css/main.css` relative to the site directory. Path traversal is blocked: `..` sequences return 404, symlinks outside root are not followed.

All file responses are `Cache-Control: public` — m6-http caches them as pre-compressed byte blobs. After the first request, CSS and JS are served directly from RAM at the cost of a socket write. m6-file is not contacted again until the file changes on disk.

## `data/site.json`

```json
{
  "nav": [
    { "label": "Home",  "path": "/" },
    { "label": "About", "path": "/about" }
  ]
}
```

This file is the `global_params` source — merged into every template's context. Add any site-wide variables here: `site_name`, analytics IDs, feature flags, whatever every page needs.

## `templates/base.html`

```html
<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>{% block title %}{{ site_name }}{% endblock %}</title>
  <link rel="stylesheet" href="/assets/style.css">
</head>
<body>
  <nav>
    {% for item in nav %}
      <a href="{{ item.path }}">{{ item.label }}</a>
    {% endfor %}
  </nav>
  <main>{% block content %}{% endblock %}</main>
</body>
</html>
```

Tera template inheritance: `{% extends "templates/base.html" %}` in child templates, `{% block content %}` to fill the slot. Works like Jinja2 with one important difference: `{% set %}` at the top level of a child template is silently discarded — you must set variables inside the block where they're used.

## `templates/error.html`

```html
{% extends "base.html" %}
{% block title %}{{ query.status }} · {{ site_name }}{% endblock %}
{% block content %}
  <h1>{{ query.status }} — {{ query.reason }}</h1>
  <p>The page <code>{{ query.path }}</code> could not be found.</p>
  <a href="/">Return home</a>
{% endblock %}
```

`query` is a built-in key that m6-html populates with the parsed query string from the URL. For error pages, m6-http calls `/_errors?status=404&from=/missing-path` — so `query.status` and `query.from` are available in the template.

## `dev.sh`

```bash
#!/bin/bash
set -e
SITE="$(cd "$(dirname "$0")" && pwd)"
trap 'kill $(jobs -p) 2>/dev/null' EXIT

m6-html "$SITE" "$SITE/configs/m6-html.conf" &
m6-file "$SITE" "$SITE/configs/m6-file.conf" &
m6-http "$SITE" &

echo "Running at https://localhost:8443"
wait
```

Three background processes. `trap` kills them all on `Ctrl-C`. `set -e` aborts if any binary isn't found.

## Hot reload

Change anything in `data/site.json` or a template, then:

```bash
touch site.toml
```

m6-http watches `site.toml` via inotify. Touching it triggers a full config and cache reload — no restart. Your changes are live in milliseconds.

---

**← [Prerequisites](/blog/guide-00-prerequisites)** | **[Example 02 — Blog with m6-md](/blog/guide-02-blog) →**
