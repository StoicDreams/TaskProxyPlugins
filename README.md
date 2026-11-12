# Task Proxy Plugins

Plugins for [Task Proxy](https://www.taskproxy.com), and the Rust kit for writing them.

| Plugin | What it does |
|---|---|
| [Jira](plugins/jira) | Jira Cloud without leaving Task Proxy: your issues, each issue with its parents through Views you design, new issues, and a branch for each issue. |

Install them from Task Proxy's **Plugins** page.

## How plugins work

A plugin is a WebAssembly file with a `plugin.json` manifest. Task Proxy runs it with [Extism](https://extism.org), with no WASI, network, or file access of its own. Everything goes through Task Proxy, which refuses anything the manifest does not declare:

- **Hosts:** HTTPS requests only to the hosts in `permissions.hosts`, such as `*.atlassian.net`. No redirects.
- **Variables:** only the keys in `permissions.variables`. The user enters them on the Plugins page, where they are saved encrypted for that plugin alone, and a project or global variable of the same name takes their place.
- **Storage:** the plugin's own encrypted data, and files under `.taskproxy/plugins/<id>/` in the open project, which a team shares through git.
- **Git:** `currentBranch` reads the branch, and `createBranch` lets a button create one. A plugin never changes the repo on its own: Task Proxy runs host actions only when the user clicks.

Plugins never send HTML or scripts. They describe their screens as data (`sdk/src/ui.rs`: stacks, tables, forms, collapsible sections, markdown), which Task Proxy draws with its own components and sanitizes.

## Writing a plugin

1. Copy `plugins/jira` as a starting point: a `cdylib` crate depending on `taskproxy-plugin-sdk`, with a `plugin.json`.
2. Implement `Plugin` (`render`, `action`, and optionally `branch_info`) and export it with `export_plugin!(MyPlugin)`.
3. Test it natively with a fake `Host`, as `plugins/jira/src/lib.rs` does in `testing::FakeHost`.
4. Run `pwsh ./PowerShell/Build.ps1`, then in Task Proxy choose **Plugins → Install from folder** and pick `dist/<id>`.

```bash
cargo test                                            # Every crate's tests, natively
cargo clippy --all-targets                            # Lint
pwsh ./PowerShell/Build.ps1                           # Build every plugin into dist/<id>
cargo run -p taskproxy-jira --example demo -- <dir>   # The Jira screens for the website, as JSON
```

## Publishing

Every push to `main` runs `.github/workflows/deploy.yml`: it tests, builds, signs both files of each plugin with the Task Proxy update key, and uploads them with `catalog.json` to `https://cdn.myfi.ws/apps/task-proxy/plugins/`. Task Proxy checks the signatures, and that the signed manifest asks for exactly the permissions the catalog shows, before installing. Bump a plugin's version in both its `Cargo.toml` and `plugin.json` before pushing a change.

## License

MIT
