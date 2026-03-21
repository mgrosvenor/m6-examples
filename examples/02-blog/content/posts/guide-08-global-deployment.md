+++
title   = "Guide 08: Global Deployment"
date    = "2025-01-01"
summary = "Six nodes, five continents, one origin. A WireGuard hub-and-spoke backbone carries H2C traffic from cache nodes to Sydney. GeoDNS routes each user to the nearest edge."
tags    = ["guide", "tutorial", "deployment", "global", "wireguard", "cdn"]
cover   = "https://picsum.photos/seed/guide08/1200/500"
+++

**Guide series:**
[Prerequisites](/blog/guide-00-prerequisites) →
[Example 01](/blog/guide-01-static-site) →
[Example 02](/blog/guide-02-blog) →
[Example 03](/blog/guide-03-contact-form) →
[Example 04](/blog/guide-04-auth) →
[Example 05](/blog/guide-05-cms) →
[Example 06](/blog/guide-06-systemd) →
[Example 07](/blog/guide-07-dev-to-production) →
**Example 08 — Global Deployment**

---

`examples/09-global-deployment/` shows how to spread a single m6 site across six nodes worldwide using WireGuard as an encrypted backbone and GeoDNS to route each user to their nearest edge.

## Architecture

<pre class="mermaid">
graph TD
    DNS["GeoDNS\n(ClouDNS)"] -->|Oceania| SYD
    DNS -->|Americas West| SF
    DNS -->|Americas East| NYC
    DNS -->|Americas Central| CHI
    DNS -->|Europe| LON
    DNS -->|Asia| SIN

    subgraph "Sydney origin"
        SYD["m6-http\nbind=0.0.0.0:443 TLS\nh2c_bind=10.0.0.1:80"]
        BACKENDS["m6-html · m6-file\nm6-auth · render-cms"]
        SYD --> BACKENDS
    end

    subgraph "WireGuard 10.0.0.0/24"
        SF["San Francisco\n10.0.0.2"]
        NYC["New York\n10.0.0.3"]
        CHI["Chicago\n10.0.0.4"]
        LON["London\n10.0.0.5"]
        SIN["Singapore\n10.0.0.6"]
    end

    SF  -->|H2C| SYD
    NYC -->|H2C| SYD
    CHI -->|H2C| SYD
    LON -->|H2C| SYD
    SIN -->|H2C| SYD
</pre>

One **origin** in Sydney runs all backend services. Five **cache nodes** (SF, NYC, Chicago, London, Singapore) each run a single `m6-http` instance that proxies everything upstream.

The cache nodes are pure HTTP proxies. They hold no content, no auth database, no CMS. All writes go through the origin; reads are served locally when cached.

## Why H2C over WireGuard?

The cache nodes talk to Sydney over **H2C** (HTTP/2 cleartext) on WireGuard. WireGuard already encrypts the tunnel end-to-end, so adding TLS on top would mean:

- Double encryption — wasted CPU on both ends
- Certificate management for internal-only endpoints
- No benefit: WireGuard peers are authenticated by public key

H2C on WireGuard gives you HTTP/2 multiplexing (multiple in-flight requests per connection, header compression) with WireGuard's encryption and zero-round-trip reconnect. It's faster and simpler than an internal TLS setup.

## Node roles

### Sydney origin

One m6-http instance handles both the public edge and the H2C backbone via two fields in the system config:

**`configs/sydney.toml`:**

```toml
[server]
bind     = "0.0.0.0:443"
tls_cert = "/etc/letsencrypt/live/syd.example.com/fullchain.pem"
tls_key  = "/etc/letsencrypt/live/syd.example.com/privkey.pem"
h2c_bind = "10.0.0.1:80"
```

- `bind` — public HTTPS on all interfaces; served to end users and Oceania GeoDNS traffic
- `h2c_bind` — cleartext HTTP/2 on the WireGuard interface, only reachable from cache nodes

Both listeners share the same site config and backend Unix sockets in `/run/m6/*.sock`.

### Cache nodes

Each cache node runs one m6-http instance with:

**`configs/cache-<city>.toml`** — TLS, node-specific cert path:

```toml
# London example
[server]
bind     = "0.0.0.0:443"
tls_cert = "/etc/letsencrypt/live/lon.example.com/fullchain.pem"
tls_key  = "/etc/letsencrypt/live/lon.example.com/privkey.pem"
```

**`configs/cache-site.toml`** — identical on every cache node:

```toml
[[backend]]
name = "origin"
url  = "h2c://10.0.0.1:80"

[[route]]
path    = "/"
backend = "origin"

# ... full route table mirrors origin-site.toml ...

# CMS and auth routes always bypass cache
[[route]]
path      = "/cms"
backend   = "origin"
require   = "group:editors"
cache     = "no-store"
```

The `cache = "no-store"` on CMS and auth routes ensures writers always reach the origin. Every other route can be served from the cache node's in-memory HTTP cache.

## WireGuard hub-and-spoke

Sydney is the WireGuard hub. Each cache node is a spoke.

**`wireguard/sydney-wg0.conf`:**

```ini
[Interface]
Address    = 10.0.0.1/24
ListenPort = 51820
PrivateKey = <sydney-private-key>

[Peer]  # San Francisco
PublicKey  = <sf-public-key>
AllowedIPs = 10.0.0.2/32

[Peer]  # New York
PublicKey  = <nyc-public-key>
AllowedIPs = 10.0.0.3/32

# ... London (10.0.0.5), Singapore (10.0.0.6), Chicago (10.0.0.4)
```

**`wireguard/sf-wg0.conf`** (spoke):

```ini
[Interface]
Address    = 10.0.0.2/24
PrivateKey = <sf-private-key>

[Peer]  # Sydney
PublicKey           = <sydney-public-key>
Endpoint            = syd.example.com:51820
AllowedIPs          = 10.0.0.1/32
PersistentKeepalive = 25
```

`PersistentKeepalive = 25` keeps the NAT mapping alive on VPS providers that drop idle UDP sessions.

## systemd units

Two units on Sydney, one on each cache node:

**Origin (one unit — both listeners):**

```ini
# m6-http-origin.service
[Service]
ExecStart=/usr/local/bin/m6-http \
    /var/www/my-blog \
    /etc/m6/sydney.toml
```

`sydney.toml` has both `bind` (TLS) and `h2c_bind` (WireGuard backbone) — one process, two listening sockets.

**Cache nodes (one unit, parameterised via EnvironmentFile):**

```ini
# m6-http-cache.service
[Service]
EnvironmentFile=/etc/m6/node-config
ExecStart=/usr/local/bin/m6-http \
    /var/www/my-blog \
    ${NODE_CONFIG}
```

`/etc/m6/node-config` on each cache node contains just:

```bash
NODE_CONFIG=/etc/m6/cache.toml
```

The same `.service` file deploys identically to all five cities. Only the environment file differs per node.

## TLS certificates

Cache nodes need per-node certificates for their own hostname (`sf.example.com`, `lon.example.com`, etc.). Certbot DNS challenge is the easiest path for wildcard certs:

```bash
# One wildcard covers all nodes
certbot certonly --dns-cloudflare \
    -d "*.example.com" \
    --dns-cloudflare-credentials /etc/m6/cloudflare.ini
```

Or use individual per-node certs with standalone certbot on each node.

Auto-renewal hook — `m6-http` reloads the cert when `site.toml` is touched:

```bash
# /etc/letsencrypt/renewal-hooks/deploy/m6-http-reload.sh
touch /var/www/my-blog/site.toml
```

## GeoDNS with ClouDNS

ClouDNS GeoDNS routes each user to their nearest node by returning a different A record per geographic region:

| Region | Hostname resolves to |
|---|---|
| Oceania + default | syd.example.com |
| Americas West | sf.example.com |
| Americas East | nyc.example.com |
| Americas Central | chi.example.com |
| Europe | lon.example.com |
| Asia | sin.example.com |

The actual A records point to each server's public IP. GeoDNS is purely a routing layer — it has no knowledge of m6 or your content.

All six names (`syd.example.com`, `sf.example.com`, …) are A records that ClouDNS serves conditionally based on the resolver's location.

## First-time setup

**Origin (Sydney):**

```bash
./setup-origin.sh syd.example.com admin@example.com
```

The script installs WireGuard, certbot, and ufw; generates auth keys in `/etc/m6/`; patches the domain into the system config; installs systemd units.

**Each cache node:**

```bash
# On the cache node
./setup-cache-node.sh sf sf.example.com wireguard/sf-wg0.conf
```

Then sync the WireGuard keys (generate with `wg genkey | tee privkey | wg pubkey > pubkey` on each node, patch into the `.conf` files, then `wg set wg0 peer <pubkey> ...` or restart `wg-quick`).

## Deploy

```bash
# From your dev machine — push content to origin
./deploy.sh user@syd.example.com

# From origin — push static assets to cache nodes
# (cache nodes hold no server-side content, but having templates
#  locally avoids a round-trip for error pages)
rsync -av --delete \
  --exclude 'content/drafts/' --exclude 'data/auth.db' --exclude 'keys/' \
  /var/www/my-blog/ root@sf.example.com:/var/www/my-blog/
```

Because cache nodes proxy everything to origin, you only need to keep static assets in sync. Blog content is always fetched live from origin.

## Local demo

Run the entire 6-node topology on one machine using loopback addresses:

```bash
cd examples/09-global-deployment
./dev.sh
```

This starts:

| Instance | Address | Role |
|---|---|---|
| Origin (Sydney) | https://127.0.0.1:9001 | Public TLS — direct access + CMS |
| Origin H2C backbone | http://127.0.0.1:9000 | Cache-node backbone (internal) |
| Cache SF | https://127.0.0.1:9002 | San Francisco proxy |
| Cache NYC | https://127.0.0.1:9003 | New York proxy |
| Cache Chicago | https://127.0.0.1:9004 | Chicago proxy |
| Cache London | https://127.0.0.1:9005 | London proxy |
| Cache Singapore | https://127.0.0.1:9006 | Singapore proxy |

One m6-http process on the origin handles both listeners (`bind` for TLS, `h2c_bind` for H2C). All five cache instances proxy back to `127.0.0.1:9000` over plain H2C (mirroring production WireGuard). You can hit any of the five TLS ports and get the same blog served from the same origin.

TLS uses a `mkcert` self-signed cert generated on first run. The CMS is available at `https://127.0.0.1:9001/cms` (login: admin / admin).

## Cost

Six entry-level VPS nodes (~$6/month each on most providers) gives you global reach for ~$36/month total. There is no CDN contract, no origin shield fee, no per-GB egress pricing beyond what the VPS provider charges (typically 1–2 TB/month included). The WireGuard backbone traffic between nodes is minimal — it's only cache misses and CMS writes.

---

**← [Example 07 — Dev to Production](/blog/guide-07-dev-to-production)**
