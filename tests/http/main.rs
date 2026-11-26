//! HTTP tests: each one starts the real server against real Postgres and
//! Garage and asserts only on what a visitor sees over HTTP.
//!
//! Run them with `scripts/test.sh` after `scripts/dev-up.sh`.

mod support;

mod add_test;
mod admin_login;
mod documents;
mod download_menu;
mod download_names;
mod search_page;
mod zips;
