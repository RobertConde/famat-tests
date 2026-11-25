//! The FAMAT Test Bank: one binary that serves the site and, later, provides
//! the `famat` CLI subcommands (`import`, `set-passcode`).
//!
//! Built twice by `dx`: with `--features server` it is the web server; with
//! `--features web` it is the WASM client that hydrates the server-rendered
//! pages. Public pages must work without the client (ADR 0001).

mod app;

#[cfg(feature = "server")]
mod server;

#[cfg(not(feature = "server"))]
fn main() {
    dioxus::launch(app::App);
}

#[cfg(feature = "server")]
fn main() -> std::process::ExitCode {
    server::cli::main()
}
