+++
title   = "m6-http: Cache, Router, and Reverse Proxy"
date    = "2025-03-05"
summary = "m6-http's primary job is its in-memory response cache. On a cache hit it writes pre-rendered, pre-compressed bytes directly to the network socket — no backend contact, no work. The router only runs on cache misses."
tags    = ["m6-http", "performance", "internals"]
cover   = "https://picsum.photos/seed/m6http/1200/500"
+++

m6-http is the only process in an m6 deployment that listens on a public port. Its primary job is its **in-memory response cache**. Most requests in production never reach a backend. They're answered entirely from RAM.

Everything else — TLS termination, routing, auth enforcement, error page fetching — only runs when the cache can't answer the request.

## The cache

<pre class="mermaid">
flowchart TD
    Req["Request"] --> CacheHit{"Cache hit?"}
    CacheHit --"yes (common case)"--> ServeRAM["Write bytes to socket\n(no backend, no work)"]
    CacheHit --"no"--> AuthReq{"Auth required?"}
    AuthReq --"JWT invalid"--> Reject["401 / 403"]
    AuthReq --"JWT valid or no auth"--> RouteMatch{"Route match?"}
    RouteMatch --"no match"--> NotFound["404"]
    RouteMatch --"yes"--> Backend["Forward to backend"]
    Backend --> Cacheable{"Cacheable?"}
    Cacheable --"yes"--> CacheStore["Store in cache + return"]
    Cacheable --"no"--> Return["Return to client"]
</pre>

A cache entry is a complete HTTP response, ready to write to the network:

- Rendered HTML (or file bytes), already compressed to **brotli or gzip**
- All response headers, pre-formed as a byte string
- The status line

On a cache hit, m6-http does nothing except call `write()` on the socket. No template lookup, no file read, no compression, no backend contact, no allocation. For a typical page request, the server-side latency is a few microseconds.

**Cache key:** `(path, content-encoding)`. Query strings are stripped before lookup. Each encoding variant is cached independently — if a brotli client is first, the brotli variant is stored; the next gzip client triggers a fresh render which is cached as a separate entry.

**What gets cached:** Responses with `Cache-Control: public`. `no-store` and `private` are not cached. 4xx and 5xx responses are never cached regardless of headers.

**The cache is stored behind an `Arc`**, swapped atomically on invalidation. The swap is non-blocking. In-flight requests that hold a reference to the old cache complete normally — they're reading from an immutable snapshot. The new snapshot is built and swapped in; the old one is freed when the last reader drops it.

### Invalidation without TTLs

<pre class="mermaid">
flowchart LR
    DataFile["data file\nchanged on disk"] --> inotify["inotify\nevent"]
    inotify --> mhttp["m6-http"]
    mhttp --> Map["invalidation\nmap lookup"]
    Map --> Evict["evict only the\naffected URLs"]
    Map --> Keep["everything else\nstays cached"]
</pre>

m6-http builds an invalidation map at startup. Two sources:

**Route params** — each `params` file declared in renderer configs maps to all routes that reference it. If `data/posts.json` is a params file for `/blog` and `/blog/{stem}`, then writing to `data/posts.json` evicts both — and only both. The rest of the cache is untouched.

**Route groups** — glob-expanded at startup. `content/posts/hello-world.json` maps to `/blog/hello-world`. When that file changes, only that URL is evicted.

inotify fires → map lookup → targeted eviction. No TTLs, no sweep, no full-cache wipe, no stale-while-revalidate dance. Pages that weren't affected by the edit stay cached.

## The event loop

m6-http runs a single-threaded epoll event loop. No Tokio. No async runtime. Just epoll, file descriptors, and explicit state machines.

The constraints are self-imposed and deliberate:

- No heap allocation on the hot path
- No `unwrap()` or `expect()` — every error is handled
- No blocking calls in the event loop
- The cache `Arc` is swapped atomically; never mutated in place

A single-threaded event loop is easier to reason about, easier to profile, and has predictable latency. There's no lock contention because there are no locks on the hot path.

## HTTP/3 and QUIC

m6-http speaks HTTP/3 via [quinn](https://github.com/quinn-rs/quinn) and HTTP/1.1 and HTTP/2 via rustls. QUIC connections share the same epoll set as TCP connections — the QUIC UDP socket is just another fd.

QUIC matters on real network paths with packet loss. On loopback, TCP wins the Happy Eyeballs race because the crypto handshake is slower. That's expected.

## Routing (cache miss path only)

Route matching uses [matchit](https://github.com/ibraheemdev/matchit), a radix tree router. Specificity rules: exact paths beat parameterised paths, longer paths beat shorter ones.

The routing decision runs only on cache misses:

1. **Auth required?** Verify JWT locally. 401 or 403 before any backend contact.
2. **Route match?** Forward to backend pool.
3. **No match?** 404 per configured error mode.
4. **Response cacheable?** Store it. Next request is a cache hit.

## Backend pools

Each `[[backend]]` is a pool of Unix sockets. m6-http watches the socket directory via inotify:

- Socket appears matching glob → added to pool
- Socket disappears → removed from pool
- Connection fails → temporarily removed, retried with backoff (1s, 2s, 4s, max 30s)
- Empty pool → appropriate error response per config

Load balancing is least-connections. The active connection count is the only state tracked per backend member.

## Auth enforcement

Routes with `require` are checked before forwarding:

```toml
[[route]]
path    = "/dashboard"
backend = "m6-html"
require = "group:staff"
```

The JWT is extracted from `Authorization: Bearer` or the `session` cookie (header takes precedence). Signature verified locally against m6-auth's public key — no network call. Expiry checked. Claims checked against the `require` declaration.

For browser clients with an expired token but a valid refresh cookie, m6-http transparently redirects through `POST /auth/refresh` and back to the original path. API clients always receive 401 directly.

## Error handling

Three modes:

| Mode | Response |
|---|---|
| `"status"` | Status code, empty body |
| `"internal"` | Status code, m6-http-generated minimal HTML |
| `"custom"` | Status code, error page fetched from `[errors] path` |

In custom mode, m6-http makes a `GET <errors-path>?status=N&from=<original-path>` request to the error backend (typically m6-html), gets back a rendered HTML page, and returns it to the client with the original status code. If that fetch also fails, it falls back to internal mode. No recursion.

## Hot reload

`site.toml` is the sole inotify trigger for site content reloads. Touching it rebuilds:

- Routing table
- Backend pool configuration
- Auth config (public key path)
- Cache invalidation map (re-expands all route group globs)

No restart. In-flight requests complete against the old config. New requests use the new config.

TLS certificates reload independently when the cert or key file changes.

## Observability

Structured JSON to stdout. Every request logs: path, status, backend, latency in microseconds, cache hit/miss. Auth failures log at warn. Pool changes log at info.

Periodic stats every 10 seconds: `rps_avg`, `rps_peak`, `latency_p50_us`, `latency_p99_us`, `cache_hit_rate`, `backend_errors`, `pool_members`.

journald captures everything. `journalctl -u m6-http -f` is all you need.

## Running it

```
m6-http <site-dir> <system-config>
```

The system config contains only the `[server]` block — bind address and TLS paths that differ per environment. Everything else lives in `site.toml` alongside the content.

```bash
# Development
m6-http . configs/system-dev.toml

# Production
m6-http /srv/my-blog /etc/m6/my-blog.toml
```

Exit codes: `0` clean shutdown, `1` runtime error, `2` config or usage error (before binding).

---

**← [m6 Architecture](/blog/architecture)** | **[m6-html and m6-render](/blog/m6-render-deep-dive) →**
