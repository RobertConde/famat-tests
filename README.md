# FAMAT Test Bank

The searchable archive of past FAMAT and Mu Alpha Theta competition tests, rewritten in Rust: one `famat` binary serving a [Dioxus](https://dioxuslabs.com/learn/0.7/) fullstack app (pinned `=0.7.10`, on axum 0.8), with Postgres for metadata and Garage (S3) for files. See `GLOSSARY.md` for vocabulary and `docs/adr/` for decisions.

## Prerequisites

- Rust (stable, edition 2024) with the `wasm32-unknown-unknown` target
- `dioxus-cli` 0.7.10: `cargo install dioxus-cli --version 0.7.10 --locked`
- Podman (or Docker, with `CONTAINER_ENGINE=docker`). On macOS start the VM first: `podman machine start`

## Run it locally

```sh
scripts/dev-up.sh     # start Postgres (127.0.0.1:55432) and Garage (127.0.0.1:3900)
scripts/dev.sh        # dx serve against them: http://127.0.0.1:8080
scripts/dev-down.sh   # stop them (add --wipe to delete their data too)
```

`dev-up.sh` generates the database password and the Garage key and writes them to `~/.cache/famat-dev/dev.env` (outside the repo; never commit it). Migrations run when the server starts.

To log in as an Admin locally, set the passcode first (it is read from a prompt, or from stdin when piped, never from the command line):

```sh
scripts/with-dev-env.sh cargo run --features server -- set-passcode
```

Then open `/login`. Setting a new passcode ends every Admin session.

## Run the tests

```sh
scripts/dev-up.sh
scripts/test.sh                 # cargo test --features server
scripts/test.sh search_page     # just the tests whose names match
```

The suite in `tests/http/` is HTTP-only: each test starts the real `famat` binary on a free port against its own fresh Postgres database and its own key prefix in the Garage bucket. It sets up data with fixtures, then asserts only on what comes back over HTTP: HTML, status codes, headers and bytes. Everything is removed afterwards. Set `FAMAT_TEST_KEEP=1` to keep a test's database, objects and server log. Set `FAMAT_TEST_SERVER=dist/server` to run the suite against a release bundle.

CI (`.github/workflows/ci.yml`) runs the same scripts on every push, with Docker.

## Release build

```sh
scripts/bundle.sh     # dx bundle --web --release into a clean dist/
```

This produces `dist/server` with `dist/public/` (pre-compressed WASM and JS) beside it.

## Configuration

The server reads its configuration from the environment:

| Variable | |
|---|---|
| `DATABASE_URL` | Postgres URL |
| `DATABASE_MAX_CONNECTIONS` | pool size, default 10 |
| `S3_ENDPOINT`, `S3_REGION`, `S3_BUCKET` | Garage's S3 API |
| `S3_ACCESS_KEY_ID`, `S3_SECRET_ACCESS_KEY` | Garage key |
| `S3_KEY_PREFIX` | prefix for every object key, default empty |
| `IP`, `PORT` | listen address, default `127.0.0.1:8080` |

## Admin login

- Only an argon2id hash of the shared passcode is stored, set by `famat set-passcode`. Sessions live in Postgres (only a SHA-256 of each cookie's token) and last 30 days.
- Every Admin server function authorizes itself with `server::auth::require_admin` (reads) or `require_admin_form` (form posts, which also check the CSRF token). Anonymous callers get 401, a bad token 403.
- Every non-GET request with another site's `Origin` is refused with 403.
- Failed logins are limited to 5 per minute per `CF-Connecting-IP` (an IPv6 /64 counts as one visitor), falling back to the socket address when the header is absent. The header is trusted because the Cloudflare Tunnel is the only way in; don't expose the server's port directly.

## Adding Tests

- **Add test** (`/admin/tests/new`) is one native multipart form, so it works without JavaScript. With JavaScript, choosing files lists them with their guessed kinds and a label field each.
- Files are PDF, DOC, DOCX, PPT, PPTX, RTF, ODT or ZIP, at most 100 MB (MiB) each and 1 GB per upload. Behind Cloudflare, one request is also capped by the plan (100 MB on Free and Pro).
- Each file streams into Garage under its Document ID as it arrives, so the server holds at most one 8 MiB part of it. A refused upload deletes whatever it had stored.
- If the six Facet values match an untrashed Test, the files wait in `pending_uploads` while the Admin chooses: add them to that Test, save a new Test anyway, or discard them. Unanswered ones are swept after a week.

## Change log

Every Admin write records what changed, before and after, and when, in the `change_log` table, through `server::change_log`. Entries are written inside the same transaction as the change:

```rust
let mut tx = state.db.begin().await?;
// … the write …
change_log::record(&mut tx, Entity::Test, id, [Change::new("host", "Chiles", "Lawton Chiles")]).await?;
tx.commit().await?;
```

There is one entry per field, with human-readable values (Facet value names, `true`/`false`). A new record also gets an `id` entry with no before (`record_created`), and a deleted one an `id` entry with no after (`record_deleted`).

## Layout

- `src/app.rs`: the Dioxus app (routes, pages, server functions), compiled for both server and WASM
- `src/app/admin.rs`: the login and logout pages and the Admin menu
- `src/app/add_test.rs`, `src/app/upload_rules.rs`: the Add test page, and the file rules it shares with the server (allowed types, kind guessing, Word+PDF pairing)
- `src/download.rs`: what the download menus offer (kind groups, Formats, the bulk zip and its 500-Test cap), shared by the page and the zip routes
- `src/server/`: server-only code: CLI, config, Admin auth, Postgres, Garage, the change log, and plain axum routes such as downloads
- `migrations/`: sqlx migrations
- `tests/http/`: the HTTP test suite and its harness (`support/`)
- `scripts/`, `dev/`: local Postgres + Garage, and the run, test and bundle commands
