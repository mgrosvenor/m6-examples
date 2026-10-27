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

[errors]
mode             = "internal"
verbose_fallback = true

[log]
level  = "info"
format = "text"

[[backend]]
name    = "m6-html"
sockets = "/tmp/m6/m6-html*.sock"

[[backend]]
name    = "m6-file"
sockets = "/tmp/m6/m6-file*.sock"

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

`site.toml` declares backends and routes. m6-http reads it. It holds nothing that differs between machines, so it has no `[server]` block. m6-html and m6-file have their own configs (below) that further describe how to handle each route on their side.

The `[[route_group]]` glob expands at startup: every file under `assets/` becomes a routable URL. `assets/style.css` → `/assets/style.css`. No individual route declarations needed.

## `configs/system-dev.toml`

```toml
[server]
bind     = "127.0.0.1:8443"
tls_cert = "keys/dev.pem"
tls_key  = "keys/dev-key.pem"
```

The system config holds the state of the machine that runs m6-http: the address to listen on and the TLS certificate. It is the second argument to m6-http (see `dev.sh` below). Relative paths resolve against the site directory, and `dev.sh` issues `keys/dev.pem` and `keys/dev-key.pem` on first run. A production machine has its own system config with its own bind address and certificate paths, and `site.toml` stays the same.

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
# dev.sh — start the 01-static stack for local development.
set -e

SITE="$(cd "$(dirname "$0")" && pwd)"
M6="${M6:-$(cd "$SITE/../../.." && pwd)/m6}"
export PATH="$M6/target/release:$PATH"

# ── TLS certs ─────────────────────────────────────────────────────────────────
mkdir -p "$SITE/keys"
if [ ! -f "$SITE/keys/dev.pem" ]; then
    if command -v mkcert &>/dev/null; then
        mkcert -install 2>/dev/null || true
        mkcert -key-file "$SITE/keys/dev-key.pem" \
               -cert-file "$SITE/keys/dev.pem" \
               localhost 127.0.0.1
    elif command -v openssl &>/dev/null; then
        # No mkcert: fall back to a self-signed certificate.
        #
        # The only thing mkcert adds is a local CA your browser already trusts,
        # which matters to a person clicking around and not to `curl -k` or to a
        # test suite. This used to be a hard `exit 1`, and m6's build host has no
        # mkcert -- the CI stage that runs this example only worked because rsync
        # happened to carry a developer's own untracked keys/ directory. A check
        # that depends on an untracked file is not a check.
        echo "mkcert not found; generating a self-signed certificate with openssl."
        echo "Your browser will warn about it. For one it trusts: brew install mkcert"
        openssl req -x509 -newkey rsa:2048 -sha256 -days 365 -nodes \
            -keyout "$SITE/keys/dev-key.pem" -out "$SITE/keys/dev.pem" \
            -subj "/CN=localhost" -addext "subjectAltName=DNS:localhost,IP:127.0.0.1" 2>/dev/null \
            || { echo "ERROR: openssl could not write a certificate." >&2; exit 1; }
    else
        echo "ERROR: neither mkcert nor openssl found. Install one:" >&2
        echo "  mkcert   - certificate your browser trusts (brew install mkcert)" >&2
        echo "  openssl  - self-signed, browser warns" >&2
        exit 1
    fi
fi

# ── Clean up stale state ──────────────────────────────────────────────────────
mkdir -p /tmp/m6
lsof -ti :8443 2>/dev/null | xargs kill -9 2>/dev/null || true
sleep 0.3
rm -f /tmp/m6/m6-html.sock /tmp/m6/m6-file.sock

# ── Start services ────────────────────────────────────────────────────────────
M6_SOCKET_OVERRIDE=/tmp/m6/m6-html.sock \
    m6-html "$SITE" "$SITE/configs/m6-html.conf" &
HTML_PID=$!

M6_SOCKET_OVERRIDE=/tmp/m6/m6-file.sock \
    m6-file "$SITE" "$SITE/configs/m6-file.conf" &
FILE_PID=$!

trap 'kill $HTML_PID $FILE_PID $HTTP_PID 2>/dev/null; wait 2>/dev/null' EXIT

# Wait for backend sockets
for i in $(seq 1 20); do
    [ -S /tmp/m6/m6-html.sock ] && [ -S /tmp/m6/m6-file.sock ] && break
    sleep 0.2
done

m6-http "$SITE" "$SITE/configs/system-dev.toml" &
HTTP_PID=$!

echo "Running at https://localhost:8443"
wait
```

The first block issues the certificate named in `configs/system-dev.toml` if `keys/dev.pem` does not exist yet. It uses `mkcert` when installed and falls back to a self-signed `openssl` certificate. The script then starts three background processes. `trap` kills them all on `Ctrl-C`. `set -e` aborts if any step fails.

## Hot reload

Change anything in `data/site.json` or a template, then:

```bash
touch site.toml
```

m6-http watches `site.toml` via inotify. Touching it triggers a full config and cache reload — no restart. Your changes are live in milliseconds.

---

**← [Prerequisites](/blog/guide-00-prerequisites)** | **[Example 02 — Blog with m6-md](/blog/guide-02-blog) →**
