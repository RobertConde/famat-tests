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

The servers the tests start convert no PDFs unless a test asks (`PDF_WORKERS=0`). The PDF queue tests (`pdf_queue`) use a stand-in for LibreOffice. The real conversions are slow and need LibreOffice (`soffice`) installed, so they are `#[ignore]`d:

```sh
scripts/test.sh pdf_conversion -- --ignored
```

CI (`.github/workflows/ci.yml`) runs the same scripts on every push, with Docker. The slow PDF tests run in their own job, which installs LibreOffice.

## Release build

```sh
scripts/bundle.sh     # dx bundle --web --release into a clean dist/
```

This produces `dist/server` with `dist/public/` (pre-compressed WASM and JS) beside it.

The app image (`Containerfile`, Debian trixie) builds that bundle and adds LibreOffice (Writer and Impress, without their GUIs) and metric-compatible fonts:

```sh
podman build -t famat -f Containerfile .
```

CI (`.github/workflows/image.yml`) builds the bundle, runs the HTTP suite against it, puts that same bundle in the image (`--build-context bundle=dist`), checks that the image answers `/healthz`, and on `main` pushes it to `ghcr.io/robertconde/famat-tests` (`:latest` and `:sha-<commit>`).

## Deployment

Production runs on Debian 13 as Podman Quadlet units, behind a Cloudflare Tunnel, with nightly restic backups. The files are in `deploy/`, and the runbooks (setup, Cloudflare, import, restore, upgrading) in `docs/deploy/`.

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
| `PDF_WORKERS` | PDF conversions run at once, default 2; 0 turns conversion off (jobs still queue) |
| `PDF_TIMEOUT_SECS` | hard timeout per conversion, default 120 |
| `PDF_SOFFICE` | the LibreOffice command, default `soffice` |
| `PDF_PROFILE_TEMPLATE` | a LibreOffice profile each job copies (the image has one); unset, jobs start from an empty one |
| `PDF_WORK_DIR` | where jobs make their temporary directories, default the system's |

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

## Importing the old archive

`famat import` builds the archive from the old site's data, all kept outside git:

- the students' sheet and the AI labels
- the docs export and the old SQL dump
- the Documents unpacked from bundle zips
- the attach decisions and the AI's review flags

Check what it would do first. `--dry-run` only reads the inputs, prints the summary and writes the reconciliation report:

```sh
scripts/with-dev-env.sh cargo run --features server -- import \
  --sheet "test list - 000final.csv" \
  --ai-labels data/labels/ai-labels.csv \
  --docs "<the docs export>" \
  --legacy-sql doralmao_famat.sql \
  --unpacked data/labels/unpacked.csv \
  --unpacked-dir data/unpacked \
  --attach data/labels/attach-1.out.csv \
  --attach data/labels/attach-2.out.csv \
  --review data/labels/review-core.csv \
  --report import-report.txt \
  --dry-run
```

Without `--dry-run` it replaces the whole archive:

- Every file streams into Garage: each Original, plus each supplied PDF. Garage's copy is read back and checked against the source's size and SHA-256 before anything in Postgres changes.
- Then, in one transaction, the archive tables are wiped and rebuilt, and the ID sequences move past the imported IDs (Tests after 3706, Documents after 10394). Objects under `documents/` that the import didn't write are then deleted.
- Every Original being prepared gets a PDF conversion job (the backfill). A running server's workers pick them up within seconds; otherwise the server converts them once it starts.
- It can run again and again until an Admin changes something. From the first change log entry on, it refuses before touching anything.

The reconciliation report goes to stdout (its summary and any problems) and to `--report` (all of it, default `import-report.txt`):

- **Source files:** a line for each file of the docs export and of the unpacked directory. Each one was imported, imported Hidden, imported as a supplied PDF, dropped or failed, and the line says why.
- **Old Tests:** each old Test is either kept with its Documents, or the line says where each of its Documents went.
- **Garage and PDFs:** every object matched its source, Garage holds nothing else, and the conversion queue has a job for each Original being prepared.
- **Problems:** anything missing, failed or unaccounted for, and inputs that disagree.

What it does with the inputs:

- AI rows replace the sheet's row for the same File Number.
- Values are tidied: `?`, `-`, blank and year `0` become unknown. Month is not applicable for State, National and Mail-in. Event names become singular. Test type parentheses are dropped. Washington becomes State with Host Washington Mu Alpha Theta.
- Calculus/Mu "Applications" becomes Mu Applications or Calculus Applications, from the file name, the AI evidence or the file's bytes. Undecided ones become Calculus Applications and get Needs review.
- Documents with identical labels form one Test. A Test keeps the old `test_id` of the old Test it shares the most Documents with. Documents keep their `tid`.
- A Document with rows for several Tests, such as a Master Key, is one Document on each of them.
- **Hidden** with a note: duplicates (`duplicate of <tid>`, from `Duplicate Of` and the attach decisions), broken files (an AI note starting "broken"), and non-test files (`Hidden` yes). A Test with no Published Document of a known kind, or with neither Year nor Competition, is Hidden by the publish rule.
- **Dropped:** `.lnk` shortcuts, and the bundle zips named in `unpacked.csv`. Their Documents are imported from the unpacked directory instead, and any PDF that came with one is its supplied PDF. Any other zip (6293) stays a zip, not convertible.
- **Attach decisions:**
  - `attach` puts the Document on the Test holding the Documents it names.
  - `new test` keeps it off the old Test its labels match. If it has no other Test, it gets a Test of its own.
  - `duplicate of` hides it. When that "duplicate" is a PDF and the Original is a Word or PowerPoint file, it is the Original's supplied PDF (3623/3624 and 3732/3733), not a Document of its own.
- **Needs review**, one comment line per reason:
  - the flags in `review-core.csv`
  - unpacked.csv notes starting "needs review"
  - the attach-checks' mislabels (#10)
- PDF status: `ready` for a PDF or a supplied PDF. `preparing` for anything the converter takes (Word, PowerPoint, OpenDocument, RTF, images). Otherwise `not_convertible`. A file with no extension has its type read from its contents.
- Display orders are seeded as in the spec.
- The fixture set the tests use, `tests/fixtures/import/`, shows each input's format.

## Facet values

- **Facet values** (`/admin/facets?facet=host`) lists one Facet's values in display order, with how many Tests use each (Hidden and Trashed ones too). Every action is a native form post: ↑/↓, **Rename**, **Merge into…**, **Delete** and **+ Add value**. With JavaScript, values can also be dragged by their handle, and the drop posts the Facet's new order.
- Reordering renumbers the whole Facet `1..n`. The search dropdowns and the results order follow it.
- A rename keeps the old name as an alias in `facet_value_aliases`. A merge moves every Test to the surviving value, and the old name and its aliases become the survivor's aliases.
- Search URLs resolve a value by its name, then by an alias, then by either one ignoring case (`server::facet_values::FacetValueList::find`). The page then shows the current name, so `/?host=Chiles` still finds the same Tests after "Chiles" becomes "Lawton Chiles".
- A value is deleted only when no Test uses it.

## Document kinds

- **Document kinds** (`/admin/document-kinds`) lists the kinds in display order, each with how many Documents use it (Hidden ones and those in Trash included).
- Each row's name and its questions / answers / solutions ticks post together with its **Save**. ↑ / ↓ reorder without JavaScript; once hydrated, rows can also be dragged by their handle.
- **Delete** is offered, and allowed, only for a kind no Document uses, including files held by Add test's duplicate warning.
- Nothing about kinds is cached. The Includes filters, the download menus' kind groups and the zips read the flags on every request, and Add test's kind dropdowns follow the order.

## Old URLs

The old PHP site's URLs redirect with a 301 (`server::legacy`):

- `scripts/download.php?tid=N` → `/documents/N/pdf`
- `docs/<N> <name>`, a file in the old listing → `/documents/N/original`. `docs/` itself → the search page.
- `index.php?year=…&category=…&level=…&style=…&host=…&submit=…` → the same search. The old form also submitted it to `/`, so `/` with `category`, `level`, `style` or `submit` redirects too. `category` "Invitationals - January" becomes `competition=Invitational&month=January`, `level` becomes `division_level` and `style` becomes `test_type`. The values are tidied as the import tidies them, `?` and `-` become Unknown, and then each one is written by its current name. A name that is no value passes through.

A `tid` that names no Published Document redirects to the search page with a 302, since it may be published later.

## Change log

Every Admin write records what changed, before and after, and when, in the `change_log` table, through `server::change_log`. Entries are written inside the same transaction as the change:

```rust
let mut tx = state.db.begin().await?;
// … the write …
change_log::record(&mut tx, Entity::Test, id, [Change::new("host", "Chiles", "Lawton Chiles")]).await?;
tx.commit().await?;
```

There is one entry per field, with human-readable values (Facet value names, `true`/`false`). A new record also gets an `id` entry with no before (`record_created`), and a deleted one an `id` entry with no after (`record_deleted`).

## PDF conversion

Every Document keeps its Original, and a PDF is made from it in the background (`server::pdf`).

- **Queue:** the `pdf_jobs` table holds one job per Document whose `pdf_status` is `preparing`.
  - Add test queues them in its own transaction.
  - At startup, every `preparing` Document without a job gets one, which covers a backfill or a crash.
  - Workers claim jobs with `FOR UPDATE SKIP LOCKED` and a lease. If a worker dies, its lease runs out and the job is claimed again.
- **Limits:** 2 jobs at once and a 120 s hard timeout, after which the converter's whole process group is killed. Each job is tried twice at most (one retry).
- **Converters:**
  - Word, RTF, ODF and PowerPoint go through LibreOffice, headless, with a throwaway profile per job and `MathTypeToMath` off. Two `soffice` processes sharing a profile silently produce nothing.
  - Images are wrapped losslessly: a JPEG byte for byte, anything else as its own pixels.
  - A PDF Original is its own PDF. A ZIP is not convertible.
  - An Original with no extension has its type read from its contents.
- **Success** means a non-empty PDF that parses. It is stored as `documents/{id}/pdf`, with its sha256 in `document_files`, and the status becomes `ready`.
- **Failure** after the retry sets `failed` and Needs review, with the error as the comment. A later success clears a Needs review that was only about the failure.
- **Admins** (the PDF problems page, `/admin/needs-review`) can **Upload PDF** to replace a PDF by hand, **Retry PDF**, and **Retry all failed**. Each is a CSRF-checked form post written to the change log.

Until the PDF is ready, a Document's PDF link says "PDF being prepared" and offers the Original.

To queue a conversion from other code, inside the transaction that sets `pdf_status = 'preparing'`:

```rust
server::pdf::enqueue(&mut tx, document_id).await?;
tx.commit().await?;
state.pdf.wake();
```

## Layout

- `src/app.rs`: the Dioxus app (routes, pages, server functions), compiled for both server and WASM
- `src/app/admin.rs`: the login and logout pages and the Admin menu
- `src/app/add_test.rs`, `src/app/upload_rules.rs`: the Add test page, and the file rules it shares with the server (allowed types, kind guessing, Word+PDF pairing)
- `src/app/document_kinds.rs`: the Document kinds page
- `src/app/facet_values.rs`: the Facet values page
- `src/app/pdf.rs`: the PDF problems page, and the Upload PDF / Retry PDF / Retry all failed forms
- `src/download.rs`: what the download menus offer (kind groups, Formats, the bulk zip and its 500-Test cap), shared by the page and the zip routes
- `src/server/`: server-only code: CLI, config, Admin auth, Postgres, Garage, the change log, the PDF queue (`pdf/`), and plain axum routes such as downloads
- `src/server/import/`: `famat import`: reading the inputs, tidying, the import plan and its special cases, loading it, and the reconciliation report
- `migrations/`: sqlx migrations
- `tests/http/`: the HTTP test suite and its harness (`support/`)
- `tests/fixtures/import/`: the synthetic inputs the import tests run on
- `tests/fixtures/pdf/`: the files the PDF conversion tests convert
- `Containerfile`: the app image
- `deploy/`, `docs/deploy/`: the production units, backup script and runbooks
- `scripts/`, `dev/`: local Postgres + Garage, and the run, test and bundle commands
