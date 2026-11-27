mod api;
mod auth;
mod config;
mod http;
mod mapping;

use config::Config;
use std::net::SocketAddr;

fn main() {
    let mut config = Config::load();
    let (server, bound_port) = match bind(&config) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("SourceTube could not bind a port: {error}");
            std::process::exit(1);
        }
    };
    if bound_port != config.port {
        config.port_in_use_fallback = true;
        config.port = bound_port;
        let scheme = if config.app_origin.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        let host = config.app_origin.split("://").nth(1).unwrap_or("localhost");
        let host = host.split(':').next().unwrap_or("localhost");
        config.app_origin = format!("{scheme}://{host}:{bound_port}");
        if config.redirect_uri.starts_with(&config.app_origin) {
            config.redirect_uri = format!("{}/api/auth/callback", config.app_origin);
        }
    }
    config.ensure_public_dir();
    write_port_file(config.port);

    println!(
        "SourceTube {} running at {}",
        env!("CARGO_PKG_VERSION"),
        config.app_origin
    );
    println!("Metadata API: {}", config.invidious_instances.join(", "));
    if !config.google_client_id.is_empty() {
        println!("Google OAuth enabled");
    }
    if config.port_in_use_fallback {
        println!("Default port was busy; selected port {}", config.port);
    }

    for request in server.incoming_requests() {
        http::handle(request, &config);
    }
}

fn write_port_file(port: u16) {
    let path = Config::runtime_dir().join("sourcetube-port");
    let _ = std::fs::write(path, port.to_string());
}

fn bind(config: &Config) -> Result<(tiny_http::Server, u16), String> {
    let attempts: Box<dyn Iterator<Item = u16>> = if config.strict_port {
        Box::new(std::iter::once(config.port))
    } else {
        Box::new(config.port..config.port.saturating_add(20))
    };

    let mut last_error = String::from("no port available");
    for port in attempts {
        let address = SocketAddr::from(([0, 0, 0, 0], port));
        match tiny_http::Server::http(address) {
            Ok(server) => return Ok((server, port)),
            Err(error) => last_error = error.to_string(),
        }
    }
    Err(last_error)
}
