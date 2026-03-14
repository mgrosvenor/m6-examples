use m6_render::prelude::*;

// Shared — SmtpTransport is Send+Sync, no locking needed
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
        // GET /contact served by framework default (template render from config)
        .run()
}
