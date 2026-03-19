+++
title   = "Guide 06: Production with systemd"
date    = "2025-01-02"
summary = "Run every m6 process as a systemd unit with proper ordering, restart policy, log capture, security hardening, and horizontal scaling."
tags    = ["guide", "tutorial", "systemd", "production", "deployment"]
cover   = "https://picsum.photos/seed/guide06/1200/500"
+++

**Guide series:**
[Prerequisites](/blog/guide-00-prerequisites) →
[Example 01](/blog/guide-01-static-site) →
[Example 02](/blog/guide-02-blog) →
[Example 03](/blog/guide-03-contact-form) →
[Example 04](/blog/guide-04-auth) →
[Example 05](/blog/guide-05-cms) →
**Example 06 — systemd** →
[Example 07](/blog/guide-07-dev-to-production)

---

`examples/06-systemd/` takes the CMS blog from example 05 and runs it properly under systemd. Every process is a unit. Ordering, restart policy, log capture, socket directory permissions, deploy workflow, and horizontal scaling are all covered.

## Server preparation

```bash
# Dedicated user — no login shell, no home directory
useradd --system --no-create-home --shell /usr/sbin/nologin m6

# Site directory — owned by m6 user
mkdir -p /var/www/my-blog
chown m6:m6 /var/www/my-blog

# Auth keys — readable only by m6 user
mkdir -p /etc/m6
openssl ecparam -name prime256v1 -genkey -noout -out /etc/m6/auth.pem
openssl ec -in /etc/m6/auth.pem -pubout -out /etc/m6/auth.pub
chown m6:m6 /etc/m6/auth.pem /etc/m6/auth.pub
chmod 600   /etc/m6/auth.pem

# TLS certificate via Let's Encrypt
certbot certonly --standalone -d example.com
# Outputs /etc/letsencrypt/live/example.com/fullchain.pem
#         /etc/letsencrypt/live/example.com/privkey.pem
# Add m6 to the ssl-cert group or adjust permissions as needed
```

## Deploy the site directory

```bash
# From your development machine
rsync -av --delete \
  --exclude 'content/drafts/' \
  --exclude 'data/auth.db' \
  --exclude 'keys/' \
  examples/05-cms/ \
  user@example.com:/var/www/my-blog/

# On the server, fix ownership after rsync
chown -R m6:m6 /var/www/my-blog
```

`content/posts/` (published posts), `data/auth.db`, and `keys/` are excluded — these live exclusively on the server.

## Unit files

<pre class="mermaid">
flowchart TD
    net["network.target"]
    net --> mhtml["m6-html.service"]
    net --> mfile["m6-file.service"]
    net --> mauth["m6-auth.service"]
    net --> rcms["render-cms.service"]
    mhtml --> mhttp["m6-http.service"]
    mfile --> mhttp
    mauth --> mhttp
    rcms --> mhttp
</pre>

One unit per process. Renderers follow the same pattern; only `ExecStart` and `Description` differ.

### `/etc/systemd/system/m6-html.service`

```ini
[Unit]
Description=m6-html renderer
Documentation=https://github.com/m6/m6
After=network.target

[Service]
Type=simple
User=m6
Group=m6

# Creates /run/m6/ with correct ownership; removes it on stop
RuntimeDirectory=m6
RuntimeDirectoryMode=0750

ExecStart=/usr/local/bin/m6-html \
    /var/www/my-blog \
    /var/www/my-blog/configs/m6-html.conf

Restart=on-failure
RestartSec=2

StandardOutput=journal
StandardError=journal
SyslogIdentifier=m6-html

# Hardening
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/run/m6
ReadOnlyPaths=/var/www/my-blog /etc/m6

[Install]
WantedBy=multi-user.target
```

### `/etc/systemd/system/m6-file.service`

```ini
[Unit]
Description=m6-file renderer
After=network.target

[Service]
Type=simple
User=m6
Group=m6
RuntimeDirectory=m6
RuntimeDirectoryMode=0750

ExecStart=/usr/local/bin/m6-file \
    /var/www/my-blog \
    /var/www/my-blog/configs/m6-file.conf

Restart=on-failure
RestartSec=2
StandardOutput=journal
StandardError=journal
SyslogIdentifier=m6-file
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/run/m6
ReadOnlyPaths=/var/www/my-blog

[Install]
WantedBy=multi-user.target
```

### `/etc/systemd/system/m6-auth.service`

```ini
[Unit]
Description=m6-auth service
After=network.target

[Service]
Type=simple
User=m6
Group=m6
RuntimeDirectory=m6
RuntimeDirectoryMode=0750

ExecStart=/usr/local/bin/m6-auth \
    /var/www/my-blog \
    /var/www/my-blog/configs/m6-auth.conf

Restart=on-failure
RestartSec=2
StandardOutput=journal
StandardError=journal
SyslogIdentifier=m6-auth
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/run/m6 /var/www/my-blog/data
ReadOnlyPaths=/var/www/my-blog /etc/m6

[Install]
WantedBy=multi-user.target
```

`ReadWritePaths` includes `/var/www/my-blog/data` — m6-auth writes the SQLite database there.

### `/etc/systemd/system/render-cms.service`

```ini
[Unit]
Description=render-cms custom renderer
After=network.target

[Service]
Type=simple
User=m6
Group=m6
RuntimeDirectory=m6
RuntimeDirectoryMode=0750

ExecStart=/var/www/my-blog/bin/render-cms \
    /var/www/my-blog \
    /var/www/my-blog/configs/render-cms.conf

Restart=on-failure
RestartSec=2
StandardOutput=journal
StandardError=journal
SyslogIdentifier=render-cms
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/run/m6 /var/www/my-blog/content
ReadOnlyPaths=/var/www/my-blog /etc/m6

[Install]
WantedBy=multi-user.target
```

The `render-cms` binary lives in `/var/www/my-blog/bin/` — deployed alongside the site, not installed system-wide.

### `/etc/systemd/system/m6-http.service`

```ini
[Unit]
Description=m6-http reverse proxy and cache
Documentation=https://github.com/m6/m6
After=network.target m6-html.service m6-file.service m6-auth.service render-cms.service

[Service]
Type=simple
User=m6
Group=m6
RuntimeDirectory=m6
RuntimeDirectoryMode=0750

ExecStart=/usr/local/bin/m6-http /var/www/my-blog

Restart=on-failure
RestartSec=2
StandardOutput=journal
StandardError=journal
SyslogIdentifier=m6-http

# Bind to port 443
AmbientCapabilities=CAP_NET_BIND_SERVICE
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/run/m6
ReadOnlyPaths=/var/www/my-blog /etc/letsencrypt /etc/m6

[Install]
WantedBy=multi-user.target
```

`After=` expresses preference, not a hard dependency. m6-http starts after renderers but handles missing backends gracefully — the socket pool backoff mechanism covers the brief window between m6-http binding and all renderers being ready.

## Install and start

```bash
cp examples/06-systemd/systemd/*.service /etc/systemd/system/

systemctl daemon-reload

systemctl enable m6-html m6-file m6-auth render-cms m6-http
systemctl start  m6-html m6-file m6-auth render-cms m6-http

systemctl status m6-html m6-file m6-auth render-cms m6-http
```

## Verify

```bash
# Check m6-http is listening
ss -tlnp | grep :443

# Check all sockets appeared
ls -la /run/m6/
# Expected:
# /run/m6/m6-html.sock
# /run/m6/m6-file.sock
# /run/m6/m6-auth.sock
# /run/m6/render-cms.sock

# Smoke test
curl -s -o /dev/null -w "%{http_code}" https://example.com/
# 200
curl -s -o /dev/null -w "%{http_code}" https://example.com/cms
# 302 (redirect to /login)
```

## Logs

```bash
# Follow all m6 processes together
journalctl -f -u m6-http -u m6-html -u m6-file -u m6-auth -u render-cms

# Structured log processing
journalctl -u m6-http -o cat | jq .

# Errors only
journalctl -u m6-http -p err

# Between timestamps
journalctl -u m6-http --since "2024-01-15 09:00" --until "2024-01-15 10:00"
```

## Deploy updates

```bash
# Template or config changes — rsync and done
# m6-http detects site.toml change via inotify and reloads
rsync -av --delete \
  --exclude 'content/drafts/' \
  --exclude 'data/auth.db' \
  --exclude 'keys/' \
  examples/05-cms/ user@example.com:/var/www/my-blog/

# Binary changed — deploy and restart just that unit
cargo build --release -p render-cms
rsync target/release/render-cms user@example.com:/var/www/my-blog/bin/
ssh user@example.com systemctl restart render-cms
# m6-http detects socket reappear and resumes routing — other units unaffected

# TLS cert renewal — certbot handles automatically
# m6-http detects cert file change and reloads TLS context — no restart needed
certbot renew
```

## Horizontal scaling

Add a second m6-html instance with zero config change to m6-http:

```bash
# Create a second config (content identical to m6-html.conf)
cp /var/www/my-blog/configs/m6-html.conf \
   /var/www/my-blog/configs/m6-html-2.conf

# Copy and edit the unit
cp /etc/systemd/system/m6-html.service \
   /etc/systemd/system/m6-html-2.service
# Edit ExecStart to reference m6-html-2.conf, update Description and SyslogIdentifier

systemctl daemon-reload
systemctl enable m6-html-2
systemctl start  m6-html-2

# /run/m6/m6-html-2.sock appears
# m6-http detects it via inotify — adds to pool within milliseconds
# Traffic now load-balanced across both instances (least-connections)
```

Scale back down:

```bash
systemctl stop m6-html-2
# Socket disappears — m6-http removes from pool immediately
```

## Crash recovery

`Restart=on-failure` with `RestartSec=2` handles crashes automatically. During the 2-second restart window, m6-http's pool backoff returns 503 for affected routes. When the socket reappears, m6-http resumes normal routing.

```bash
# Simulate a crash
systemctl kill -s KILL m6-html

# Watch systemd restart it
journalctl -u m6-html -f
```

---

**← [Example 05 — CMS Blog](/blog/guide-05-cms)** | **[Example 07 — Dev to Production](/blog/guide-07-dev-to-production) →**
