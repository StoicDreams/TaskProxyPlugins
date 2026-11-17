# Guidelines for plugin creators

The [spec](0.1.md) says what Task Proxy requires. These guidelines are about building a plugin that feels like part of Task Proxy, keeps its users' data safe, and keeps working as Task Proxy and the spec move on. Follow them unless you have a reason not to.

## Scope and permissions

- **Ask for the least you need.** Every permission is shown before installing, and users judge a plugin by that list. Name exact hosts where you can, and use `*.example.com` only when each user has their own subdomain, as Jira Cloud does.
- **One plugin, one job.** Jira is one plugin. A plugin for Jira and GitHub together would need both sets of permissions in every project.
- **Prefer a host action to a capability.** To change something outside the plugin, such as the repo, return a button with a host action, so the user sees and clicks it, instead of asking for a permission to do it directly.
- **Never send data to a host it did not come from** unless that is the plugin's purpose and its description says so.

## Settings and projects

- **Settings are per project.** Declare what the plugin needs as `variables`, and read them with `var`. Never ask for a token in a page's form and store it yourself: declared variables are encrypted, masked, and kept per project for you.
- **Mark every credential `secret`,** and write `help` that says where to get it, such as the page that creates an API token.
- **Draw a setup page instead of an error.** When a variable is missing, `render` should explain what is needed and offer the `openSettings` host action. Task Proxy also draws the setup itself for projects that are not set up, so this is for a value that is set but stops working, such as an expired token.
- **Keep shared data in project files.** Use `.taskproxy/plugins/<id>/` for what a team should share through git, such as Jira's Views, and storage for what is personal. Write files in a stable format with sorted keys and an ending newline, so diffs stay small.

## Screens

- **Use the nodes, not workarounds.** Use `table` for lists, `properties` for an item's fields, `section` for parts the user may hide, and `notice` for something they must see. Markdown is for text that came as rich text, such as an issue's description, not for layout.
- **Keep keys stable.** A `section` or `panel` is remembered by its `key`. Build keys from what the part shows, such as `parent-2` or `comments`, never from a counter or the time.
- **Confirm what can't be undone.** Give every action that deletes, sends, or changes something outside Task Proxy a `confirm` question that names what will happen: "Delete the comment on PROJ-12?", not "Are you sure?".
- **Say what happened.** After a change, return a short `message` or redraw the page so the change shows. Use `refresh` rather than rebuilding the page yourself when the data changed behind it.
- **Show an `empty` node** for a list with nothing in it, saying why and what to do: "No issues are assigned to you. Try All Issues."
- **Use a switch for anything with two states.** Something that is on or off, such as a split view, is a `button` with `on` (`ui::switch` in the SDK), labeled with what it switches, such as "Split view", never a button whose label says "On" or "Off".
- **Load choices late.** For a value changed in place, use a `select` node, whose choices are asked for only when it is clicked, rather than a form with every choice for every row.
- **Write plain, short text.** Use sentence case, a person's words rather than an API's ("Story points", not `customfield_10016`), and messages that say what to do next. Error messages from your exports are shown as they are, so make them sentences.
- **Check every icon name** against `https://cdn.myfi.ws/icons/all.json`. An unknown name shows a placeholder.

## Speed and limits

- **Each call starts fresh.** Nothing survives in memory between calls, so cache what is slow to fetch and slow to change in storage, with the time from `now`, as Jira caches its fields for a day.
- **Page through long lists** with the service's paging rather than loading everything. A call has 30 seconds of WebAssembly time and 64 MB.
- **Ask a service only for what the page shows,** such as only the fields a Jira View lists.
- **Notifications are polled every two minutes.** Make `notifications` quick, and return the same `id` for the same notification each time, or it will pop up again.

## Staying compatible

- **List the lowest spec patch you need.** If you only use what 0.1.0 has, list `0.1.0`, even when the SDK speaks a later patch. Raise it only when you use something newer, and then your plugin no longer installs on Task Proxy releases without that patch.
- **Keep `apiVersion: 1`** while your plugin supports line 0.1, for Task Proxy 0.1.121 and earlier.
- **Expect older hosts.** Those releases do not check `spec`, so a host call added after 0.1.0 may answer with an error there. Handle it, as by hiding a feature, rather than failing the page.
- **Ignore what you don't know.** Read requests leniently and don't fail on extra fields, so a newer Task Proxy can add them.
- **Raise your plugin's version on every release,** in `plugin.json` and its build file together. Published versions are never replaced.
- **Breaking changes to your own data** need a migration. Read the old format of your storage and project files as well as the new one.

## Security

- **Treat everything from a service as untrusted text.** It is drawn safely as text, but don't pass it into `markdown` unless it is meant to be markdown, and never into a URL for `openUrl` without checking it is the service's own HTTPS address.
- **Keep secrets out of URLs, messages, notifications, and project files.** Send credentials in headers, as `http` requires.
- **Don't log values.** A plugin has no log, but errors are shown to the user and may be screenshotted. Name what failed, not the token that failed.
- **Validate params.** An action's `params` come from your own screens, but a page's params come from its address, which anyone can link to. Check a key looks like a key before using it in a request.

## Testing

- **Test natively.** With the Rust SDK, plugin logic takes a `&dyn Host`, so unit tests pass a fake host with recorded responses and need no WebAssembly. Record real responses from the service, with the private parts replaced, as test fixtures.
- **Test the manifest against the code,** as Jira's `the_manifest_matches_the_plugin` does: its version, `spec`, and the variables it reads.
- **Try it in Task Proxy** with Install from folder, built with `pwsh ./scripts/build.ps1`, before publishing. Check a project that is not set up, a project that is, and a wrong or expired credential.
- **Build to WebAssembly in CI,** so a dependency that doesn't build for `wasm32-unknown-unknown` is found before a release.

## Publishing

- **Every plugin is open to review.** Plugins in Stoic Dreams' catalog are built and signed by this repo's CI, from source here.
- **Publishing never changes Task Proxy.** A plugin release updates the catalog only, and never needs a Task Proxy release or a change to the TaskProxyApp repo.
- **Document it:**
  - a page on TaskProxy.com in its category
  - looping videos of its main actions, recorded from a demo, as Jira's are
  - a row in the spec README's Plugins table when it changes the spec versions it supports
