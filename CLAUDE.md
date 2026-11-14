# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

Plugins for Task Proxy (the desktop app in `../TaskProxyApp`), and the Rust SDK for writing them. Plugins compile to `wasm32-unknown-unknown` and run in Task Proxy under Extism. `README.md` explains the permission model.

## Commands

```bash
pwsh ./scripts/publish.ps1                   # Before every commit: format, bump changed plugins' versions, lint, test, build, write the Jira demo screens
cargo test                                   # All tests run natively, with a fake Host
cargo test -p taskproxy-jira <name>          # One Jira test
cargo clippy --all-targets -- -D warnings    # CI fails on any warning
pwsh ./scripts/build.ps1                  # Build every plugin into dist/<id> (plugin.json + plugin.wasm)
pwsh ./scripts/build.ps1 -UpdateAppFixtures   # Only when an app test needs a new test plugin: copy it into TaskProxyApp's Rust test fixtures, re-signed with a throwaway key
cargo run -p taskproxy-jira --example demo -- plugins/jira/demo   # Refresh the website loops' data, which TaskProxyApp's capture reads from here
```

## Layout

- `.taskproxy/`: this repo's own Task Proxy pages (Dev, Publish, New Plugin) with runnable commands.
- `scripts/`: `publish.ps1` (run before every commit) and `build.ps1` (build, sign, catalog, TaskProxyApp test fixtures).
- `sdk/`: `taskproxy-plugin-sdk`. `protocol.rs` and `ui.rs` are the contract with Task Proxy, which reads them as JSON in `src-tauri/src/plugins/` and `wc/plugin-page.js`. `host.rs` sends every host call as one JSON request through the `taskproxy_host` import.
- `test-plugin/`: makes whatever host call a test asks for, so Task Proxy's tests can check what is allowed. Its built `.wasm` is committed in TaskProxyApp.
- `plugins/jira/`: the Jira Cloud plugin. `client.rs` (REST API v3 with email and API token), `adf.rs` (Atlassian Document Format to and from markdown), `fields.rs`, `views.rs` (layered Views), `pages.rs`, `demo.rs`.

## Things that will catch you out

- **Pushing to `main` publishes every plugin** to the CDN catalog. Don't push unless asked.
- **The protocol is the versioned spec in `spec/`, shared with released apps.** Change `spec/0.1.md` (or the current line's file) first, following "Changing the spec" in `spec/README.md`. Anything that only adds is a patch; anything that removes, renames, or changes a meaning is a new minor version with patch 0. Raise the SDK's `SPEC_VERSION` with the spec, but raise a plugin's `spec` entry only when it uses the new patch, since that drops older apps. The app's side is `SPEC_VERSIONS` in TaskProxyApp's `src-tauri/src/plugins/spec.rs`.
- **Every `plugin.json` lists `spec`** and keeps `"apiVersion": 1` while it supports line 0.1, since Task Proxy 0.1.121 and earlier read only `apiVersion`. `build.ps1` refuses a plugin without `spec`, and `-Catalog` keeps older versions in `versions` when a new one drops a spec line.
- **WebAssembly has no clock or randomness here.** Use `Host::now_ms`, and derive ids from it.
- **A plugin's version is in two places:** its `Cargo.toml` and `plugin.json`. `publish.ps1` bumps both for a plugin that changed since the last commit (an SDK change counts for every plugin), and a Jira test checks they match. Don't bump by hand.
- **Nothing here writes into TaskProxyApp by default,** since a push there releases the app. Its website capture reads `plugins/jira/demo/` and `plugins/jira/plugin.json` from this checkout. The test plugin in its fixtures is updated only by hand, when an app test needs a new host call; an SDK change alone rebuilds a different `.wasm` that its tests do not need.
- **Icons are Task Proxy's** (`cdn.myfi.ws/icons`). An unknown name shows a placeholder.
- **Rebuilding the test plugin changes its signatures,** so use `build.ps1 -UpdateAppFixtures`, which re-signs it and replaces the test key in TaskProxyApp.

## Commit messages

Use multiple lines: a general statement, then one line per update as `Feature: ...` or `Bug: ...`.
