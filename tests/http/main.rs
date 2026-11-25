//! HTTP tests: each one starts the real server against real Postgres and
//! Garage and asserts only on what a visitor sees over HTTP.
//!
//! Run them with `scripts/test.sh` after `scripts/dev-up.sh`.

mod support;

mod documents;
mod download_names;
mod search_page;
