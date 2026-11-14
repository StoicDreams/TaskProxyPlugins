# Task Proxy plugin spec

The plugin spec is the contract between Task Proxy and its plugins: what a plugin is, what it may declare, the functions it exports, the host calls it makes, and the screens it returns. A plugin that follows a spec version runs in every Task Proxy that supports that version.

- [Spec 0.1](0.1.md): the current line, with every patch and the draft of the next one.
- [Guidelines for plugin creators](guidelines.md): how to build a plugin that fits Task Proxy, beyond what the spec requires.

The SDK in [`sdk/`](../sdk) speaks the newest patch, `SPEC_VERSION` in `sdk/src/lib.rs`, and plugins can be written in any language with an Extism PDK by following the spec alone.

## Versions

The spec is versioned on its own, apart from Task Proxy and apart from each plugin. Task Proxy and plugins both update without waiting on a spec change, and a spec change never forces either to update.

A spec version is `major.minor.patch`, and each `major.minor` is a **line**, such as 0.1.

- **Patch (0.1.0 → 0.1.1):** only adds, so everything valid in the earlier patch stays valid and means the same. Every spec change should be a patch if it can.
- **Minor (0.1.x → 0.2.0):** a breaking change. The minor version goes up and the patch resets to 0. A plugin or app for line 0.1 knows nothing about line 0.2, and the two never mix.
- **Major:** stays 0 for now. If it is ever raised, the minor and patch reset, and it is breaking like a minor change.

A patch may add:

- new optional manifest fields, permissions, and contribution points
- new host calls, host actions, exports, and UI node types
- new optional fields on requests, results, and nodes, with a default that keeps the old meaning
- new values for an open set, such as a theme or field kind, when older hosts ignore what they do not know

Breaking, so a new minor version:

- removing or renaming anything
- making an optional field required, or changing a field's type, default, or meaning
- changing what a host call or export returns, or the envelope around it
- tightening a limit a plugin may already rely on, such as time, memory, or a size

A host or plugin always ignores fields it does not know, which is what lets patches add them.

## Compatibility

Each side records the spec versions it supports, and they match on a line both support.

- **Task Proxy** supports a list of lines, and for each one names the newest patch it implements. It lists them in `SPEC_VERSIONS` in `src-tauri/src/plugins/spec.rs`, and each release's list is in the table below and in its `Docs/Plugins.md`.
- **A plugin version** lists the lines it supports in its `plugin.json` as `spec`, naming for each the lowest patch it needs, such as `"spec": ["0.1.0"]`. A plugin that uses a host call added in 0.1.1 lists `0.1.1`. Listing the lowest patch, not the newest, lets the plugin run in older Task Proxy releases.
- **They match** on a line both list when Task Proxy's patch is at least the plugin's. With more than one line in common, Task Proxy uses the newest. A plugin learns which version it was given from the `hostInfo` host call (0.1.1).
- **No match:** Task Proxy refuses to install or run the plugin, and says which side to update. A plugin that needs a newer patch or line asks the user to update Task Proxy. A plugin whose lines Task Proxy no longer supports asks for a newer plugin version.

The catalog keeps older versions, so neither side has to update first:

- **Each catalog entry lists its `spec`.** When a new plugin version drops a line, the catalog keeps the newest older version that supports it in the entry's `versions`.
- **Task Proxy offers the newest version it can run,** or shows why it can run none.
- `scripts/build.ps1 -Catalog` writes this from the catalog already published.

### Before spec versions

Task Proxy 0.1.121 and earlier read no `spec`. They run only plugins whose manifest and catalog entry say `"apiVersion": 1`, and refuse any other value.

Later releases read `apiVersion: 1` without `spec` as spec 0.1.0. So while a plugin supports line 0.1, its manifest keeps `"apiVersion": 1` beside `spec`, and older releases still install it. For a plugin with no 0.1 line, use any other `apiVersion`, such as the oldest minor it supports, so older releases refuse it plainly.

Those releases also do not check `spec`, so they run a plugin that lists `0.1.1` or later. A plugin that uses something newer than 0.1.0 handles an error from it, since those releases answer an unknown host call with `{"error": ...}`. A failed `hostInfo` call means spec 0.1.0.

## Spec versions

| Version | Status | What changed |
|---|---|---|
| 0.1.0 | Released | The first spec: pages, actions, the Git Controller card, notifications, the screens Task Proxy draws, and the host calls for variables, storage, project files, the current branch, the clock, and HTTPS. |
| 0.1.1 | Released | The `hostInfo` host call. The manifest's `spec` list, and the catalog's `spec` and `versions`. |
| 0.1.2 | Released | The `reorder` node, for items the user drags into order, and forms' `inline`, for fields and buttons in one row. |
| 0.1.3 | Released | Images in markdown, from data URIs, and HTTP answers as base64 to download them. A switch between rendered and raw markdown. Properties that switch to an editor when their label is clicked. Forms that fill a host action's fields, such as a branch's name. |
| 0.1.4 | Draft | Processes, for the Task Manager plugin: the `processes` permission, the `processes` host call, and the `process.stop` and `process.kill` host actions. |

## Task Proxy releases

| Task Proxy | Spec versions |
|---|---|
| 0.1.115 to 0.1.117 | An early form of 0.1.0, as `apiVersion: 1`, without the `select`, `clip`, `panel`, and `inputs` nodes, sortable columns, dialogs, mentions, or notifications. Plugins drawn with those show placeholders. |
| 0.1.118 to 0.1.121 | 0.1.0, as `apiVersion: 1`. `spec` is not read. |
| 0.1.122 | 0.1.2. Checks `spec`, and offers the newest catalog version it can run. |
| 0.1.123 and later | 0.1.3 |

Add a row when a release changes what it supports.

## Plugins

Each plugin version records the spec versions it supports in its `plugin.json`, and the catalog keeps that for every published version.

| Plugin | Versions | Spec versions |
|---|---|---|
| Jira | 0.1.0 to 0.1.2 | 0.1.0, as `apiVersion: 1` |
| Jira | 0.1.3 and later | 0.1.0, with drag and drop in its column editor where Task Proxy supports 0.1.2, and description images, editable View fields, and editable branch names where it supports 0.1.3 |

Add a row when a plugin changes the spec versions it supports.

## Changing the spec

1. **Decide whether it is a patch.** Use the lists above. Prefer a patch: an optional field with a default is usually possible.
2. **Write it in the line's file first,** such as [0.1.md](0.1.md), marked "Since 0.1.N", and add it to the table above as Draft.
3. **Update the SDK and Task Proxy:**
   - the SDK's types and `SPEC_VERSION`
   - Task Proxy's `runtime.rs`, `plugin-page.js`, or `commands.rs`
   - `CallContext::handle` checks for any new permission
   - a test of any refusal, using the test plugin
4. **Release Task Proxy support:**
   - Raise the patch in `SPEC_VERSIONS` only once Task Proxy does everything that version describes.
   - Mark the version Released, and add the Task Proxy row here and in `Docs/Plugins.md`.
   - Update TaskProxy.com's [Plugin Development](https://www.taskproxy.com/plugins/development) page (`webapp/d/plugins/development.md` in the website repo). Each version has its own tab there, newest at the top. A new version gets a new tab with the whole spec as of that version, and its compatibility tables match this file's.
5. **Update plugins when they want the new feature.** They raise their `spec` entry to the patch that added it. Plugins that don't need it change nothing.
6. **A new line,** such as 0.2, gets its own file. Task Proxy can support 0.1 and 0.2 together for as long as plugins need 0.1. Dropping a line is a Task Proxy release, recorded in the table above.
