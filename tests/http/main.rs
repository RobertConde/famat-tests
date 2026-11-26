//! HTTP tests: each one starts the real server against real Postgres and
//! Garage and asserts only on what a visitor sees over HTTP.
//!
//! Run them with `scripts/test.sh` after `scripts/dev-up.sh`.

mod support;

mod add_test;
mod admin_login;
mod bulk_download;
mod document_kinds;
mod documents;
mod download_menu;
mod download_names;
mod facet_values;
mod import;
mod import_special;
mod old_urls;
mod pdf_conversion;
mod pdf_queue;
mod search_page;
mod zips;
