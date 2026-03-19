use lettre::transport::smtp::authentication::Credentials;
use m6_render::prelude::*;

struct Global {
    mailer: SmtpTransport,
    from:   String,
    to:     String,
}

fn init_global(config: &Map<String, Value>) -> Result<Global> {
    let host = config["smtp"]["host"].as_str()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("smtp.host missing")))?;
    let port = config["smtp"]["port"].as_u64()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("smtp.port missing")))? as u16;
    let username = config["smtp"]["username"].as_str()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("smtp.username missing")))?;
    let password = config["smtp"]["password"].as_str()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("smtp.password missing")))?;
    let from = config["smtp"]["from"].as_str()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("smtp.from missing")))?.to_string();
    let to = config["smtp"]["to"].as_str()
        .ok_or_else(|| Error::Other(anyhow::anyhow!("smtp.to missing")))?.to_string();

    let creds = Credentials::new(username.to_string(), password.to_string());
    let mailer = SmtpTransport::relay(host)
        .map_err(|e| Error::Other(e.into()))?
        .port(port)
        .credentials(creds)
        .build();

    Ok(Global { mailer, from, to })
}

fn handle_post(req: &Request, global: &Global) -> Result<Response> {
    let name    = req.field("name")?;
    let email   = req.field("email")?;
    let message = req.field("message")?;

    let from_addr: lettre::Address = global.from.parse()
        .map_err(|e: lettre::address::AddressError| Error::Other(anyhow::anyhow!("{e}")))?;
    let to_addr: lettre::Address = global.to.parse()
        .map_err(|e: lettre::address::AddressError| Error::Other(anyhow::anyhow!("{e}")))?;

    let msg = Message::builder()
        .from(lettre::message::Mailbox::new(None, from_addr))
        .to(lettre::message::Mailbox::new(None, to_addr))
        .subject(format!("Contact from {}", name))
        .body(format!("From: {} <{}>\n\n{}", name, email, message))
        .map_err(|e| Error::Other(anyhow::anyhow!("{e}")))?;

    global.mailer.send(&msg).map_err(|e| Error::Other(e.into()))?;

    Response::render_with("templates/contact.html", req, json!({"sent": true, "name": name}))
}

fn main() -> Result<()> {
    App::with_global(init_global)
        .route_post("/contact", handle_post)
        .run()
}
