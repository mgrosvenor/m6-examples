+++
title   = "Guide: Prerequisites and Getting Started"
date    = "2025-01-08"
summary = "Install the m6 binaries, set up local TLS with mkcert, and clone the examples repo. Everything you need before running your first site."
tags    = ["guide", "getting-started"]
cover   = "https://picsum.photos/seed/guide00/1200/500"
+++

This is the start of a seven-part guide that walks through every m6 example, from a bare static site to a full CMS running under systemd.

m6 is an integrated system for building and serving websites. It covers the whole journey — from writing content in Markdown, through template rendering, to an in-memory cache that serves pre-compressed responses directly from RAM. Each part is designed to work with the others: the cache knows what each URL depends on, the renderer produces output the cache can serve directly, and the file watcher triggers precise eviction when content changes. You could assemble something similar from nginx, Varnish, a template engine, and a Markdown converter, but you'd be managing four mental models and four integration surfaces. m6 is one system with one config format and one set of concepts across the whole stack.

**In this guide:**

1. **[Prerequisites (this post)](/blog/guide-00-prerequisites)**
2. [Example 01 — Static Site](/blog/guide-01-static-site)
3. [Example 02 — Blog with m6-md](/blog/guide-02-blog)
4. [Example 03 — Contact Form](/blog/guide-03-contact-form)
5. [Example 04 — Login and Protected Pages](/blog/guide-04-auth)
6. [Example 05 — CMS Blog](/blog/guide-05-cms)
7. [Example 06 — Production with systemd](/blog/guide-06-systemd)
8. [Example 07 — Dev to Production](/blog/guide-07-dev-to-production)

---

## Install the binaries

```bash
cargo install m6-http m6-html m6-file m6-auth
```

This installs four binaries to `~/.cargo/bin/`:

- **m6-http** — in-memory response cache, router, and reverse proxy. The only public-facing process. Most requests in production are answered entirely from its RAM cache — backends are only contacted on cache misses.
- **m6-html** — renders HTML from Tera templates and JSON data files.
- **m6-file** — serves files from the filesystem.
- **m6-auth** — issues and verifies JWTs, manages credentials.

For the blog examples you'll also want `m6-md`, which converts Markdown files to the JSON format m6-html expects:

```bash
cargo install m6-md
```

## Set up local TLS

m6-http always requires TLS. For local development, [mkcert](https://github.com/FiloSottile/mkcert) creates certificates trusted by your local browser — no cert warnings, no `--insecure` flags.

```bash
# Install the local CA (run once per machine)
mkcert -install

# Create a certificate for localhost
mkcert localhost 127.0.0.1
```

This creates `localhost.pem` and `localhost-key.pem` in the current directory. The examples reference these from `../../localhost.pem` — run the command from the `examples/` directory and it'll be in the right place.

## Clone the examples

```bash
git clone https://github.com/m6/m6-examples
cd m6-examples

# Generate TLS certs in the right location
mkcert localhost 127.0.0.1
```

Now you have all five examples ready to run. Each is self-contained: its own `site.toml`, its own configs, its own templates and data. They share only the TLS certs in the `examples/` root.

## How the examples are structured

<pre class="mermaid">
flowchart LR
    Browser --"HTTPS"--> mhttp["m6-http"]
    subgraph "Backends (Unix sockets)"
        mhtml["m6-html"]
        mfile["m6-file"]
        mauth["m6-auth"]
        custom["Custom Renderer"]
    end
    mhttp --"Unix socket"--> mhtml
    mhttp --"Unix socket"--> mfile
    mhttp --"Unix socket"--> mauth
    mhttp --"Unix socket"--> custom
</pre>

Each example builds on the previous one. The directory names reflect the progression:

```
examples/
├── localhost.pem
├── localhost-key.pem
├── 01-static/
├── 02-blog/
├── 03-contact/
├── 04-auth/
├── 05-cms/
├── 06-systemd/
└── 07-dev-to-production/
```

Every example has a `dev.sh` that starts all the required processes and waits. `Ctrl-C` kills everything cleanly.

---

**← [m6-html and m6-render](/blog/m6-render-deep-dive)** | **[Example 01 — Static Site](/blog/guide-01-static-site) →**
