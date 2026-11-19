use m6_core::platform::process::raise_fd_limit;
use m6_core::prelude::*;
use m6_core::signal::{Action, Lifecycle};
use m6_core::site::finish_reload;
use m6_core::{genid_failed, genid_ok};
use serde_json::json;
use std::fs;
use std::io;
use std::time::Duration;

/// The name the service passes to `Site::load`. It is also the configuration file's
/// stem, the socket's name and the `[[backend]]` name in site.toml.
const NAME: &str = "render-cms";

/// The code routes, in the order `route` below matches them. `Site::load` compiles
/// them at start and hands each request back as `Handled::Code(index, req)`.
const ROUTES: &[CodeRoute] = &[
    CodeRoute { pattern: "/cms",                  method: RouteMethod::Get },
    CodeRoute { pattern: "/cms/new",              method: RouteMethod::Get },
    CodeRoute { pattern: "/cms/edit/{stem}",      method: RouteMethod::Get },
    CodeRoute { pattern: "/api/drafts",           method: RouteMethod::Post },
    CodeRoute { pattern: "/api/drafts/{stem}",    method: RouteMethod::Patch },
    CodeRoute { pattern: "/api/publish/{stem}",   method: RouteMethod::Post },
    CodeRoute { pattern: "/api/unpublish/{stem}", method: RouteMethod::Post },
];

// ── the service's own event loop ─────────────────────────────────────────────
//
// m6-core supplies the parts (the reactor, the signal descriptor, the
// connection slots, the read deadlines, routing, the shutdown lines). The
// service writes the loop that ties them together, so nothing here is hidden
// in a framework. docs/m6-app-anatomy.md in m6 walks through it step by step.
//
// start: blocking and allocation are fine. run: nothing blocks. stop: blocking
// is fine again.

const LISTENER: Token = Token(usize::MAX);
const SIGNALS: Token = Token(usize::MAX - 1);
/// The log's stdout, registered only while it holds bytes a full pipe refused.
const LOG: Token = Token(usize::MAX - 2);
/// The longest the loop waits when nothing happens, and the log's flush tick.
const TICK: Duration = Duration::from_secs(1);

fn main() -> io::Result<()> {
    // Block the termination signals before anything can start a thread.
    m6_core::signal::block();

    // ── start ────────────────────────────────────────────────────────────
    let mut site = Site::load(NAME, ROUTES, &[], &TeraFactory);
    let read_timeout = site.read_timeout();
    let ceiling = raise_fd_limit().unwrap_or_else(|e| fail("raising the open file limit", e));
    let table = site.limits().get(&m6_core::limits::CONNECTIONS);
    let ceiling = ceiling.min(table.max());
    let initial = table.start().min(ceiling);

    let mut poll = Poll::new().unwrap_or_else(|e| fail("creating the poll", e));
    // The signal descriptor opens before the socket does: the socket is how a
    // caller knows the service is ready, and a stop sent right after must find
    // the handler armed.
    let mut signals = Signals::new().unwrap_or_else(|e| fail("opening the signal descriptor", e));
    let mut listener = listen_unix(site.socket_path(), site.socket_mode())
        .unwrap_or_else(|e| fail(&format!("binding {}", site.socket_path().display()), e));
    poll.registry()
        .register(&mut listener, LISTENER, Interest::READABLE)?;
    poll.registry()
        .register_fd(signals.raw_fd(), SIGNALS, Interest::READABLE)?;

    let mut lifecycle = Lifecycle::new(site.name(), Some(site.socket_path().to_path_buf()));
    lifecycle.started();

    // The connection table and its buffers follow the `[limits]` keys.
    let read_limit = site.limits().get(&m6_core::limits::REQUEST_BODY).clone();
    let write_limit = site.limits().get(&m6_core::limits::RESPONSE_BUFFER).clone();
    let mut conns = Slots::new(initial, ceiling, move || {
        ServerConn::with_limits(&read_limit, &write_limit)
    });
    // `read_timeout_s = 0` means no deadline, which is the zero duration.
    let mut deadlines = Timeouts::new(initial, read_timeout.unwrap_or(Duration::ZERO));
    let mut events = Events::with_capacity(256);
    let mut accepting = true;
    let mut last_tick = m6_core::platform::now();

    // ── run ──────────────────────────────────────────────────────────────
    'run: loop {
        // An interrupted wait is not an error: wait again.
        match poll.poll(
            &mut events,
            Some(deadlines.timeout(m6_core::platform::now(), TICK)),
        ) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
        let now = m6_core::platform::now();
        for ev in &events {
            match ev.token() {
                // New connections: a free slot for each, registered, with a
                // read deadline.
                LISTENER => {
                    while let Some(taken) = conns.take() {
                        if let Some(new_len) = taken.grew_to {
                            deadlines.grow(new_len);
                        }
                        let mut stream = match listener.accept() {
                            Ok((stream, _)) => stream,
                            Err(e) => {
                                conns.free(taken.key);
                                match m6_core::platform::net::accept_error(&e) {
                                    m6_core::platform::net::Accept::Drained => break,
                                    m6_core::platform::net::Accept::SkipOne => continue,
                                    m6_core::platform::net::Accept::Stop => {
                                        eprintln!("{NAME}: accept error: {e}");
                                        break;
                                    }
                                }
                            }
                        };
                        poll.registry().register(
                            &mut stream,
                            Token(taken.key),
                            Interest::READABLE,
                        )?;
                        if read_timeout.is_some() {
                            deadlines.touch(Token(taken.key), now);
                        }
                        conns.get_mut(taken.key).attach(stream);
                    }
                    // At the ceiling, stop accepting: new clients wait in the
                    // kernel's backlog.
                    if conns.is_full() {
                        poll.registry().deregister(&mut listener)?;
                        accepting = false;
                    }
                }
                SIGNALS => {
                    let mut stop = false;
                    while let Some(signal) = signals.read() {
                        if lifecycle.on_signal(signal) == Action::Stop {
                            stop = true;
                        }
                    }
                    if stop {
                        break 'run;
                    }
                }
                LOG => m6_core::log::flush(),
                // One connection's turn, then wait for what it still needs,
                // push its deadline back, or close it.
                Token(k) => {
                    let conn = conns.get_mut(k);
                    let was = conn.interest();
                    match turn(conn, &mut site) {
                        Ok(true) => {
                            if conn.interest() != was {
                                let interest = conn.interest();
                                let stream = conn
                                    .stream()
                                    .expect("every registered token has an attached connection");
                                poll.registry().reregister(stream, Token(k), interest)?;
                            }
                            if read_timeout.is_some() {
                                deadlines.touch(Token(k), now);
                            }
                        }
                        _ => m6_core::h1::close_conn(&poll, &mut conns, &mut deadlines, k)?,
                    }
                }
            }
        }

        // A full stdout holds its bytes. Watch it for writability only while
        // it does, so the loop never waits on the log.
        if let Err(e) = m6_core::log::watch(poll.registry(), LOG) {
            m6_core::log::error!(error = %e, "log descriptor registration failed");
        }
        // A client that sent nothing before its deadline is closed unanswered.
        while let Some(Token(k)) = deadlines.expired(m6_core::platform::now()) {
            m6_core::h1::close_conn(&poll, &mut conns, &mut deadlines, k)?;
        }
        // A slot came free while the table was full: accept again.
        if !accepting && !conns.is_full() {
            poll.registry()
                .register(&mut listener, LISTENER, Interest::READABLE)?;
            accepting = true;
        }
        if m6_core::platform::now() - last_tick >= TICK {
            m6_core::log::tick(m6_core::platform::wall());
            last_tick = m6_core::platform::now();
        }
    }

    // ── stop ─────────────────────────────────────────────────────────────
    // Connections still open are dropped: a stop does not wait for a request
    // in flight. `complete` removes the socket and logs the line.
    lifecycle.complete();
    m6_core::log::stop()?;
    Ok(())
}

/// One connection's turn: write what it owes, read, dispatch, write, and
/// report whether the connection stays open.
fn turn(conn: &mut ServerConn, site: &mut Site) -> io::Result<bool> {
    loop {
        // Write what is owed before reading anything.
        if conn.interest() == Interest::WRITABLE {
            match conn.flush()? {
                Flush::Done => {}
                Flush::More => return Ok(true),
                Flush::Close => return Ok(false),
            }
        }
        let mut heads = head_slots();
        let (response, idle) = match conn.read(&mut heads)? {
            Read::Request(head) => {
                let response = match site.handle(&head) {
                    // A page, a 404 or an error, already finished.
                    Handled::Done(response) => response,
                    // One of ROUTES: run its handler, then `finish` adds the
                    // site-wide parts (templates, Cache-Control, CSRF, flash).
                    Handled::Code(index, req) => {
                        let response = route(index, &req)
                            .unwrap_or_else(|e| m6_core::response::error_to_response(&e));
                        site.finish(&req, response)
                    }
                    // `POST /genid`: read config, templates and params again
                    // through the start path, and keep the old site if that
                    // fails.
                    Handled::Reload(reload) => {
                        let response = match site.reload(ROUTES, &[], &TeraFactory) {
                            Ok(next) => {
                                *site = next;
                                genid_ok()
                            }
                            Err(e) => genid_failed(&e),
                        };
                        finish_reload(&reload, response)
                    }
                    Handled::Named(..) => unreachable!("{NAME} registers no named handlers"),
                };
                (Some(response), false)
            }
            Read::More => (None, true),
            Read::Closed => return Ok(false),
        };
        if let Some(response) = response {
            conn.respond_response(response);
        }
        match conn.flush()? {
            // Answered and kept alive: a pipelined request may already be
            // buffered, so read again before waiting.
            Flush::Done if !idle => {}
            Flush::Done | Flush::More => return Ok(true),
            Flush::Close => return Ok(false),
        }
    }
}

/// A failure at start is printed and exits 2, the code for a bad start.
fn fail(what: &str, error: impl std::fmt::Display) -> ! {
    eprintln!("{NAME}: {what}: {error}");
    std::process::exit(2)
}

/// The handler for one route: the arm's number is the route's place in `ROUTES`.
fn route(index: usize, req: &Request) -> Result<Response> {
    match index {
        0 => handle_dashboard(req),
        1 => handle_new(req),
        2 => handle_edit(req),
        3 => handle_create_draft(req),
        4 => handle_update_draft(req),
        5 => handle_publish(req),
        6 => handle_unpublish(req),
        _ => unreachable!("route {index} is not in ROUTES"),
    }
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
    let stem  = slugify(body["title"].as_str().unwrap_or("untitled"));
    let draft = json!({
        "stem":   stem,
        "title":  body["title"],
        "body":   body["body"],
        "author": req["auth_username"],
        "date":   today_iso8601(),
    });
    req.write_json(&format!("content/drafts/{}.json", stem), &draft)?;
    Ok(Response::json_status(json!({"stem": stem}), 201))
}

fn handle_update_draft(req: &Request) -> Result<Response> {
    let stem = req["stem"].as_str().ok_or(Error::NotFound)?;
    let body: Value = req.body_json()?;
    let path = format!("content/drafts/{}.json", stem);
    let mut draft = req.read_json(&path)?;
    if let Some(t) = body.get("title") { draft["title"] = t.clone(); }
    if let Some(b) = body.get("body")  { draft["body"]  = b.clone(); }
    req.write_json(&path, &draft)?;
    Ok(Response::json(json!({"ok": true})))
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
    // No touch of site.toml: m6 services read their files once, at start, and a
    // touch reloads nothing. New content reaches readers when the site's
    // generation advances (`POST /genid`, see m6-http.md). This example does not
    // send that request yet, so restart m6-html to see a change.
    let _ = fs::remove_file(req.site_path(&draft_path));

    Ok(Response::json(json!({"published": true, "path": post["path"]})))
}

fn handle_unpublish(req: &Request) -> Result<Response> {
    let stem         = req["stem"].as_str().ok_or(Error::NotFound)?;
    let publish_path = format!("content/posts/{}.json", stem);
    let draft_path   = format!("content/drafts/{}.json", stem);

    let mut post = req.read_json(&publish_path)?;
    post["draft"] = json!(true);
    req.write_json(&draft_path, &post)?;
    let _ = fs::remove_file(req.site_path(&publish_path));
    update_index(req)?;
    // No touch of site.toml here either: see `handle_publish`.

    Ok(Response::json(json!({"unpublished": true})))
}

fn update_index(req: &Request) -> Result<()> {
    let posts = req.list_json("content/posts/")?;
    let index = json!({ "documents": posts });
    req.write_json_atomic("data/posts.json", &index)
}
