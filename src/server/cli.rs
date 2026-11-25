//! The `famat` command line. With no arguments (as `dx serve` runs it) it
//! serves the site. Later tickets add `famat import` and `famat set-passcode`.

use std::process::ExitCode;

use super::config::Config;

const USAGE: &str = "\
Usage: famat [COMMAND]

Commands:
  serve   Serve the site (the default). Listens on $IP:$PORT (127.0.0.1:8080).

Configuration comes from the environment; see README.md.";

pub fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        [] | ["serve"] => serve(),
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
    // Called again on every hot-patch under `dx serve`, so it must rebuild the
    // router from scratch each time.
    dioxus::serve(move || {
        let config = config.clone();
        async move { super::router(config).await }
    })
}
