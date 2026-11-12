# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Plugins for Task Proxy (the desktop app in `../TaskProxyApp`), and the Rust SDK for writing them. Plugins compile to `wasm32-unknown-unknown` and run in Task Proxy under Extism. `README.md` explains the permission model.

## Commands

```bash
cargo test                                   # All tests run natively, with a fake Host
cargo test -p taskproxy-jira <name>          # One Jira test
cargo clippy --all-targets -- -D warnings    # CI fails on any warning
pwsh ./PowerShell/Build.ps1                  # Build every plugin into dist/<id> (plugin.json + plugin.wasm)
pwsh ./PowerShell/Build.ps1 -UpdateAppFixtures   # Rebuild the test plugin into TaskProxyApp's Rust test fixtures, re-signed with a throwaway key
cargo run -p taskproxy-jira --example demo -- ../TaskProxyApp/tools/capture/fixtures/jira   # Refresh the website screenshot data
```

## Layout

- `sdk/`: `taskproxy-plugin-sdk`. `protocol.rs` and `ui.rs` are the contract with Task Proxy, which reads them as JSON in `src-tauri/src/plugins/` and `wc/plugin-page.js`. `host.rs` sends every host call as one JSON request through the `taskproxy_host` import.
- `test-plugin/`: makes whatever host call a test asks for, so Task Proxy's tests can check what is allowed. Its built `.wasm` is committed in TaskProxyApp.
- `plugins/jira/`: the Jira Cloud plugin. `client.rs` (REST API v3 with email and API token), `adf.rs` (Atlassian Document Format to and from markdown), `fields.rs`, `views.rs` (layered Views), `pages.rs`, `demo.rs`.

## Things that will catch you out

- **Pushing to `main` publishes every plugin** to the CDN catalog. Don't push unless asked.
- **The protocol is shared with a released app.** Adding optional fields and node types is fine, since Task Proxy draws unknown nodes as a placeholder. Renaming or removing anything breaks installed apps; that needs a new `API_VERSION` in the SDK and in TaskProxyApp's `plugins/manifest.rs`.
- **WebAssembly has no clock or randomness here.** Use `Host::now_ms`, and derive ids from it.
- **A plugin's version is in two places:** its `Cargo.toml` and `plugin.json`. A Jira test checks they match.
- **Icons are Task Proxy's** (`cdn.myfi.ws/icons`). An unknown name shows a placeholder.
- **Rebuilding the test plugin changes its signatures,** so use `Build.ps1 -UpdateAppFixtures`, which re-signs it and replaces the test key in TaskProxyApp.

## Commit messages

Use multiple lines: a general statement, then one line per update as `Feature: ...` or `Bug: ...`.
