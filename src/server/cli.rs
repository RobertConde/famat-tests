//! The `famat` command line. With no arguments (as `dx serve` runs it) it
//! serves the site. Later tickets add `famat import`.

use std::{
    io::{self, BufRead, IsTerminal},
    net::SocketAddr,
    process::ExitCode,
};

use super::{
    auth,
    config::{Config, DatabaseConfig},
    db,
};

const USAGE: &str = "\
Usage: famat [COMMAND]

Commands:
  serve          Serve the site (the default). Listens on $IP:$PORT (127.0.0.1:8080).
  set-passcode   Set the Admin passcode, ending every Admin session. Reads it
                 from a prompt, or from the first line of stdin when piped.

Configuration comes from the environment; see README.md.";

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] | ["serve"] => serve(),
        ["set-passcode"] => set_passcode(),
        ["-h" | "--help" | "help"] => {
            println!("{USAGE}");
            ExitCode::SUCCESS
        }
        _ => {
            eprintln!("famat: unexpected arguments: {}\n\n{USAGE}", args.join(" "));
            ExitCode::from(2)
        }
    }
}

fn serve() -> ExitCode {
    let config = match Config::from_env() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("famat: {e:#}");
            return ExitCode::FAILURE;
        }
    };

    // Under `dx serve`, Dioxus serves, so hot-patching works. It calls this
    // again on every hot-patch, so it must rebuild the router from scratch.
    // Dioxus does not pass the socket address on, so there the login rate
    // limit relies on CF-Connecting-IP alone.
    if dioxus::cli_config::is_cli_enabled() {
        dioxus::serve(move || {
            let config = config.clone();
            async move { super::router(config).await }
        })
    }

    // Otherwise serve it ourselves, with the socket address available to
    // handlers as the rate-limit fallback when CF-Connecting-IP is absent.
    dioxus::logger::initialize_default();
    let result = runtime().block_on(async move {
        let router = super::router(config).await?;
        let addr = dioxus::cli_config::fullstack_address_or_localhost();
        let listener = tokio::net::TcpListener::bind(addr).await?;
        dioxus::logger::tracing::info!("serving on http://{addr}");
        axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await?;
        anyhow::Ok(())
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("famat: {e:#}");
            ExitCode::FAILURE
        }
    }
}

/// `famat set-passcode`. The passcode never appears in argv, where `ps` and
/// shell history would show it.
fn set_passcode() -> ExitCode {
    let result = (|| {
        let database = DatabaseConfig::from_env()?;
        let passcode = read_passcode()?;
        if passcode.chars().count() < auth::MIN_PASSCODE_CHARS {
            anyhow::bail!("the passcode must be at least {} characters", auth::MIN_PASSCODE_CHARS);
        }
        runtime().block_on(async move {
            let db = db::connect(&database).await?;
            auth::set_passcode(&db, &passcode).await
        })
    })();
    match result {
        Ok(ended) => {
            println!("Passcode set. Admin sessions ended: {ended}.");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("famat set-passcode: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn read_passcode() -> anyhow::Result<String> {
    if io::stdin().is_terminal() {
        let passcode = rpassword::prompt_password("New Admin passcode: ")?;
        let again = rpassword::prompt_password("Repeat it: ")?;
        if passcode != again {
            anyhow::bail!("the two passcodes differ");
        }
        return Ok(passcode);
    }
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("starting the async runtime")
}
