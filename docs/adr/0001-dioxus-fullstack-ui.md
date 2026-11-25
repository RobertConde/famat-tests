---
status: accepted
---

# Dioxus fullstack for the UI

v1 is mostly server-rendered search, downloads, and Admin forms, which plain axum + askama templates + htmx would handle with less risk. We chose Dioxus fullstack, pinned to `=0.7.10` and running on axum, anyway: the roadmap includes live in-class tests, tests built from the question bank, and analytics. Those want one Rust component model on both client and server, rather than a second UI stack added later.

The spike (branch `spike/dioxus`, issue #14) confirmed all four things v1 needs. The public search page server-renders and works with JS off. The Admin passcode session works, using cookies and CSRF. Uploads stream into Garage. Zips stream through a plain axum route. The costs are about 367 KB brotli of WASM, 6–16 s incremental builds, and a ~2 min clean release build.

## Consequences

- Pin `dioxus = "=0.7.10"` and `dioxus-cli 0.7.10` (`--locked`) in the build, and make upgrades deliberate; Dioxus has broken its API at every minor version.
- Any server function that can fail with a 4xx returns `Result<T, ServerFnError>`. The default `Result<T>` always answers HTTP 500.
- Every server function is a public endpoint, so it authorizes itself. Page middleware doesn't cover `/api/*`.
- Admin writes are native `<form method="post">` posts with a hidden CSRF field. They return an explicit 303 (a raw `Response` with `Set-Cookie` where needed), because Dioxus rewrites 2xx form posts to `302 → Referer`.
- Uploads need `DefaultBodyLimit::max(..)`, since axum caps bodies at 2 MB by default.
- The build uses the spike's `[profile.wasm-release]` (opt-level z, fat LTO, 1 CGU) and `[web] pre_compress = true`, and builds into a clean out dir.
- Downloads and zips stay plain axum routes, using the `tokio::io::duplex` + `async_zip` streaming pattern.
- Hydration uses inline scripts, so a future strict CSP needs nonces or hashes.
- Agents' knowledge of the 0.7 API may be incomplete, so the spec points them at the 0.7 docs, `examples/07-fullstack/`, and the spike code.
