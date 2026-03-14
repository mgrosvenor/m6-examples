# m6-examples

Seven examples for the [m6](https://github.com/mgrosvenor/m6) web server framework, each building on the last.

## Prerequisites

```bash
# Install m6 binaries
cargo install --git https://github.com/mgrosvenor/m6 m6-http m6-html m6-file m6-auth-server m6-auth-cli

# TLS for development (run once)
mkcert -install && mkcert localhost 127.0.0.1
# Outputs localhost.pem and localhost-key.pem in the current directory
# Place or symlink them at the repo root so examples can find ../../localhost.pem
```

## Examples

| # | Directory | What it covers |
|---|-----------|---------------|
| 01 | `examples/01-static/` | Static site: templates, assets, hand-authored JSON |
| 02 | `examples/02-blog/` | Blog: Markdown posts via `m6-md`, single JSON params file |
| 03 | `examples/03-contact/` | Contact form: custom Rust renderer, SMTP email, POST handling |
| 04 | `examples/04-auth/` | Login and protected pages: JWT auth, HttpOnly cookies, `require` |
| 05 | `examples/05-cms/` | CMS blog: full system with publish flow, route invalidation |
| 06 | `examples/06-systemd/` | Production: systemd units, hardening, scaling, deploy workflow |
| 07 | `examples/07-dev-to-production/` | Dev → production: single site.toml, system config split |

## Quick start

```bash
cd examples/01-static
./dev.sh
# open https://localhost:8443
```

Each example has its own `dev.sh`. Examples with a custom Rust renderer need a build step first:

```bash
cd examples/03-contact
cargo build --release -p render-contact
./dev.sh
```

## Building custom renderers

The workspace `Cargo.toml` at the repo root includes all custom renderer crates so they share a
`target/` directory:

```bash
# Build all renderers
cargo build --release

# Or build one
cargo build --release -p render-contact
cargo build --release -p render-cms
```
