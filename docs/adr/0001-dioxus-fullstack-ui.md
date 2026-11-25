---
status: proposed
---

# Dioxus fullstack for the UI, pending a spike

v1 is mostly server-rendered search, downloads, and Admin forms, which plain axum + askama templates + htmx would handle with less risk. We chose Dioxus fullstack (pinned to 0.7.x, on axum) anyway: the roadmap includes live in-class tests, tests built from the question bank, and analytics. Those want one Rust component model on both client and server, rather than a second UI stack added later.

This becomes **accepted** only if a one-day spike shows that Dioxus can do four things: render the public search page on the server, handle the Admin passcode session, accept a file upload, and stream a zip through a plain axum route. If the spike fails, fall back to askama + htmx.

## Consequences

- Dioxus has broken its API at every minor version (0.5 → 0.6 → 0.7, with 0.8 in alpha), so the version is pinned and upgrades are deliberate.
- File downloads and zips stay plain axum routes, not server functions.
- Agents' knowledge of the 0.7 API may be incomplete, so the spec should point them at the 0.7 docs.
