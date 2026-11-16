//! Branch names for issues, from a format the user chooses, such as `feature/{key}-{summary|slug}`. Each Jira
//! sign-in (site and email) keeps its own format, so projects that share a sign-in share it, and a client's Jira
//! can use the client's own branch rules.
use crate::client::{EMAIL, Jira, encode};
use crate::pages::param;
use taskproxy_plugin_sdk::ui::{self, Action, Field, FieldKind, Node};
use taskproxy_plugin_sdk::{ActionRequest, ActionResult, Map, Result, Value, json};

/// The key, then up to 40 characters of the summary, as Task Proxy named branches before formats.
pub const DEFAULT_FORMAT: &str = "{key}-{summary|slug|max:40}";
pub const PAGE: &str = "branches";

/// What a format can name from an issue.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IssueParts {
    pub key: String,
    pub summary: String,
    pub kind: String,
    pub parent: String,
    pub parent_summary: String,
}

impl IssueParts {
    pub fn of(issue: &Value) -> Self {
        let text = |path: &str| {
            issue
                .pointer(path)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        IssueParts {
            key: text("/key"),
            summary: text("/fields/summary"),
            kind: text("/fields/issuetype/name"),
            parent: text("/fields/parent/key"),
            parent_summary: text("/fields/parent/fields/summary"),
        }
    }

    /// An issue to preview a format with when none is chosen.
    pub fn sample() -> Self {
        IssueParts {
            key: "PROJ-123".into(),
            summary: "Fix the login timeout on slow networks".into(),
            kind: "Bug".into(),
            parent: "PROJ-100".into(),
            parent_summary: "Faster sign in".into(),
        }
    }

    fn value(&self, name: &str) -> Option<String> {
        let (project, number) = self.key.split_once('-').unwrap_or((self.key.as_str(), ""));
        Some(match name.to_ascii_lowercase().as_str() {
            "key" => self.key.clone(),
            "project" => project.to_string(),
            "number" => number.to_string(),
            "summary" => self.summary.clone(),
            "type" => self.kind.clone(),
            "parent" | "parent.key" => self.parent.clone(),
            "parent.summary" => self.parent_summary.clone(),
            _ => return None,
        })
    }
}

/// Lowercase words joined by `joiner`, with everything else dropped.
fn words(text: &str, joiner: char) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(&joiner.to_string())
}

/// Cuts text to `max` characters, at the last word break when there is one.
fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max).collect();
    match cut.rfind(['-', '_', ' ', '/']) {
        Some(at) if at > 0 => cut[..at].to_string(),
        _ => cut,
    }
}

fn apply(value: String, filter: &str, problems: &mut Vec<String>) -> String {
    let (name, argument) = filter.split_once(':').unwrap_or((filter, ""));
    match name.trim().to_ascii_lowercase().as_str() {
        "slug" => words(&value, '-'),
        "snake" => words(&value, '_'),
        "lower" => value.to_lowercase(),
        "upper" => value.to_uppercase(),
        "max" => match argument.trim().parse::<usize>() {
            Ok(max) => cut(&value, max),
            Err(_) => {
                problems.push(format!(
                    "max needs a number, as max:40, not '{}'.",
                    argument
                ));
                value
            }
        },
        "map" => {
            let mut fallback = None;
            for pair in argument.split(',') {
                let Some((from, to)) = pair.split_once('=') else {
                    problems.push(format!("Each map choice is from=to, not '{}'.", pair));
                    continue;
                };
                let from = from.trim();
                if from == "*" {
                    fallback = Some(to.trim().to_string());
                } else if from.eq_ignore_ascii_case(value.trim()) {
                    return to.trim().to_string();
                }
            }
            fallback.unwrap_or(value)
        }
        other => {
            problems.push(format!("'{}' is not a filter.", other));
            value
        }
    }
}

/// Removes what git does not allow in a branch name, such as spaces, `..`, and `~^:?*[\`.
pub fn clean(name: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        let c = if c.is_whitespace() { '-' } else { c };
        if c.is_control() || "~^:?*[\\".contains(c) {
            continue;
        }
        // Empty parts, such as an issue with no parent, leave no doubled separators.
        if (c == '-' || c == '/' || c == '.') && out.ends_with(c) {
            continue;
        }
        out.push(c);
    }
    let mut out = out.replace("@{", "");
    while out.ends_with(".lock") {
        out.truncate(out.len() - ".lock".len());
    }
    out.split('/')
        .map(|part| part.trim_matches(['.', '-']))
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

/// A branch name from a format: `{name}` and `{name|filter|filter}` are replaced from the issue, and the rest is
/// kept. Returns the name and what was wrong with the format.
pub fn format(pattern: &str, issue: &IssueParts) -> (String, Vec<String>) {
    let mut problems = Vec::new();
    let mut out = String::new();
    let mut rest = pattern;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find('}') else {
            problems.push("A { has no } to end it.".into());
            out.push_str(&rest[start..]);
            rest = "";
            break;
        };
        let token = &rest[start + 1..start + end];
        let mut parts = token.split('|');
        let name = parts.next().unwrap_or("").trim();
        let mut value = match issue.value(name) {
            Some(value) => value,
            None => {
                problems.push(format!("{{{}}} is not something an issue has.", name));
                String::new()
            }
        };
        for filter in parts {
            value = apply(value, filter, &mut problems);
        }
        out.push_str(&value);
        rest = &rest[start + end + 1..];
    }
    out.push_str(rest);
    let name = clean(&out);
    if name.is_empty() {
        problems.push("The format makes an empty branch name.".into());
    }
    (name, problems)
}

/// Where a sign-in's format is kept: by its site and email.
fn store_key(jira: &Jira) -> String {
    let email = jira.host.var(EMAIL).ok().flatten().unwrap_or_default();
    format!("branchFormat:{}:{}", jira.site, email.to_lowercase())
}

/// The format this sign-in uses.
pub fn saved_format(jira: &Jira) -> String {
    jira.host
        .store_get(&store_key(jira))
        .ok()
        .flatten()
        .and_then(|value| value.as_str().map(String::from))
        .filter(|format| !format.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_FORMAT.to_string())
}

/// The branch name for an issue, in this sign-in's format, or the default one when that makes no name.
pub fn name_for(jira: &Jira, issue: &Value) -> String {
    let parts = IssueParts::of(issue);
    let (name, _) = format(&saved_format(jira), &parts);
    if name.is_empty() {
        format(DEFAULT_FORMAT, &parts).0
    } else {
        name
    }
}

const GUIDE: &str = r#"A format is text with **parts** in braces, such as `{key}`, that are filled in from the issue. Anything outside braces is kept as it is, so `feature/{key}` starts every branch with `feature/`.

| Part | Is | For PROJ-123 |
|---|---|---|
| `{key}` | The issue's key | `PROJ-123` |
| `{project}` | The project's key | `PROJ` |
| `{number}` | The issue's number | `123` |
| `{summary}` | The summary as written | `Fix the login timeout` |
| `{type}` | The issue type | `Bug` |
| `{parent}` | The parent's key, or nothing | `PROJ-100` |
| `{parent.summary}` | The parent's summary | `Faster sign in` |

Add **filters** after a part with `|`, and they run in order, as in `{summary|slug|max:40}`.

| Filter | Does | Example |
|---|---|---|
| `slug` | Lowercase words joined by `-` | `fix-the-login-timeout` |
| `snake` | Lowercase words joined by `_` | `fix_the_login_timeout` |
| `lower`, `upper` | Changes the case | `proj-123`, `PROJ-123` |
| `max:N` | Cuts to N characters, at a word break | `max:12` after `slug` makes `fix-the` |
| `map:A=B,C=D,*=E` | Swaps a value for another. `*` is used for anything else | `map:Bug=bugfix,Story=feature,*=task` on `{type}` makes `bugfix` |

Some examples:

- `{key}-{summary|slug|max:40}`, the default: `PROJ-123-fix-the-login-timeout-on-slow-networks`
- `feature/{key}_{summary|snake|max:20}`: `feature/PROJ-123_fix_the_login`
- `{type|map:Bug=bugfix,*=feature}/{key|lower}-{summary|slug}`: `bugfix/proj-123-fix-the-login-timeout-on-slow-networks`

Characters git does not allow, such as spaces, `~`, `^`, `:`, `?`, `*`, and `..`, are taken out, and a part that is empty, such as `{parent}` for an issue with no parent, leaves no doubled `-` or `/`. You can still change the name before creating each branch."#;

/// The Branch Names page: this sign-in's format, a preview of it on an issue, and how formats work.
pub fn page(jira: &Jira, page_params: &Map<String, Value>) -> Result<Node> {
    let saved = saved_format(jira);
    let draft = match param(page_params, "format") {
        "" => saved.clone(),
        draft => draft.to_string(),
    };
    let key = param(page_params, "key").trim().to_uppercase();
    let (issue, about) = if key.is_empty() {
        (IssueParts::sample(), "a sample issue".to_string())
    } else {
        match jira.get(&format!(
            "issue/{}?fields=summary,issuetype,parent",
            encode(&key)
        )) {
            Ok(found) => (IssueParts::of(&found), key.clone()),
            Err(err) => {
                return Ok(ui::stack(vec![
                    ui::notice(format!("{} could not be read: {}", key, err.0), "warning"),
                    ui::button(
                        Action::new("branches.preview", "Back").param("format", draft),
                        "secondary",
                    ),
                ]));
            }
        }
    };
    let (name, problems) = format(&draft, &issue);
    let mut preview = vec![
        ui::row(vec![
            ui::muted(format!("For {}:", about)),
            ui::badge(issue.key.clone(), ""),
            ui::muted(issue.summary.clone()),
        ]),
        ui::row(vec![
            ui::strong("Branch"),
            ui::markdown(format!("`{}`", name)),
        ]),
    ];
    for problem in &problems {
        preview.push(ui::notice(problem.clone(), "warning"));
    }
    let mut buttons = Vec::new();
    if draft != saved {
        preview.push(ui::muted("This format is not saved yet."));
        buttons.push(ui::button(
            Action::new("branches.save", "Save this format")
                .param("format", draft.clone())
                .icon("save"),
            "primary",
        ));
    }
    if saved != DEFAULT_FORMAT {
        buttons.push(ui::button(
            Action::new("branches.reset", "Use the default format").confirm(format!(
                "Go back to the default format, {}?",
                DEFAULT_FORMAT
            )),
            "secondary",
        ));
    }
    if !buttons.is_empty() {
        preview.push(ui::toolbar(buttons));
    }
    Ok(ui::stack(vec![
        ui::muted(
            "The format branches are named with when you create one from an issue. It is kept for this Jira sign-in, so projects that share the sign-in share it.",
        ),
        Node::Form {
            fields: vec![
                Field::new(FieldKind::Text, "format", "Format")
                    .value(draft.clone())
                    .required()
                    .placeholder(DEFAULT_FORMAT),
                Field::new(FieldKind::Text, "key", "Preview with")
                    .value(key)
                    .placeholder("An issue key, or blank for a sample issue"),
            ],
            submit: Action::new("branches.preview", "Preview").icon("eye"),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: true,
            changed: None,
        },
        ui::section("branch-preview", "Preview", true, preview),
        ui::section(
            "branch-guide",
            "How formats work",
            false,
            vec![ui::markdown(GUIDE)],
        ),
    ]))
}

fn form_text<'a>(request: &'a ActionRequest, name: &str) -> &'a str {
    request
        .form
        .get(name)
        .or_else(|| request.params.get(name))
        .and_then(Value::as_str)
        .unwrap_or("")
}

/// Answers the Branch Names page's actions, or `None` when the action is not one of them.
pub fn action(jira: &Jira, request: &ActionRequest) -> Result<Option<ActionResult>> {
    Ok(Some(match request.action.as_str() {
        "branches.open" => ActionResult::navigate(PAGE, Map::new()),
        "branches.preview" => {
            let mut params = Map::new();
            for name in ["format", "key"] {
                let value = form_text(request, name).trim();
                if !value.is_empty() {
                    params.insert(name.into(), json!(value));
                }
            }
            ActionResult::navigate(PAGE, params)
        }
        "branches.save" => {
            let draft = form_text(request, "format").trim().to_string();
            let (_, problems) = format(&draft, &IssueParts::sample());
            if draft.is_empty() || !problems.is_empty() {
                return Ok(Some(ActionResult::default().with_message(
                    "Fix the format's problems before saving it.",
                    "warning",
                )));
            }
            jira.host.store_set(&store_key(jira), Some(json!(draft)))?;
            ActionResult::navigate(PAGE, Map::new())
                .with_message("The branch format is saved.", "success")
        }
        "branches.reset" => {
            jira.host.store_set(&store_key(jira), None)?;
            ActionResult::navigate(PAGE, Map::new())
                .with_message("Branches use the default format again.", "success")
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::FakeHost;

    fn sample(pattern: &str) -> String {
        let (name, problems) = format(pattern, &IssueParts::sample());
        assert!(problems.is_empty(), "{}: {:?}", pattern, problems);
        name
    }

    #[test]
    fn formats_fill_parts_and_run_filters_in_order() {
        assert_eq!(
            sample(DEFAULT_FORMAT),
            "PROJ-123-fix-the-login-timeout-on-slow-networks"
        );
        assert_eq!(
            sample("feature/{key}_{summary|snake|max:20}"),
            "feature/PROJ-123_fix_the_login"
        );
        assert_eq!(
            sample("{type|map:Bug=bugfix,Story=feature,*=task}/{key|lower}-{summary|slug}"),
            "bugfix/proj-123-fix-the-login-timeout-on-slow-networks"
        );
        assert_eq!(
            sample("{type|map:Story=feature,*=task}/{number}"),
            "task/123"
        );
        assert_eq!(
            sample("{project|lower}/{parent}/{key}"),
            "proj/PROJ-100/PROJ-123"
        );
        assert_eq!(sample("{parent.summary|slug}"), "faster-sign-in");
        assert_eq!(sample("{summary|slug|max:12}"), "fix-the");
        assert_eq!(sample("{KEY|lower}"), "proj-123");
        // A summary with no words leaves only the key, as before formats.
        let mut empty = IssueParts::sample();
        empty.summary = "!!!".into();
        assert_eq!(format(DEFAULT_FORMAT, &empty).0, "PROJ-123");
        empty.summary = "Fix the login page's redirect".into();
        assert_eq!(
            format(DEFAULT_FORMAT, &empty).0,
            "PROJ-123-fix-the-login-page-s-redirect"
        );
    }

    #[test]
    fn names_are_kept_valid_for_git() {
        assert_eq!(clean("feature/ a b ~^:?*[\\ c"), "feature/a-b-c");
        assert_eq!(clean("a..b//c--d.lock"), "a.b/c-d");
        assert_eq!(clean("/-x-/"), "x");
        assert_eq!(clean("x@{y"), "xy");
        // An issue with no parent leaves no empty part.
        let mut issue = IssueParts::sample();
        issue.parent.clear();
        assert_eq!(format("{parent}/{key}-{parent}-x", &issue).0, "PROJ-123-x");
    }

    #[test]
    fn mistakes_in_a_format_are_reported() {
        let issue = IssueParts::sample();
        assert_eq!(format("{nope}-{key}", &issue).1.len(), 1);
        assert!(format("{key|shout}", &issue).1[0].contains("not a filter"));
        assert!(format("{key|max:x}", &issue).1[0].contains("needs a number"));
        assert!(format("{key", &issue).1[0].contains("no }"));
        assert!(format("{parent}", &IssueParts::default()).1[0].contains("empty"));
    }

    #[test]
    fn each_sign_in_keeps_its_own_format() {
        let host = FakeHost::connected();
        let jira = Jira::connect(&host).unwrap().unwrap();
        let issue = json!({ "key": "P-7", "fields": { "summary": "Add Export", "issuetype": { "name": "Story" } } });
        assert_eq!(name_for(&jira, &issue), "P-7-add-export");
        let save = |format: &str| {
            action(
                &jira,
                &ActionRequest {
                    page: PAGE.into(),
                    action: "branches.save".into(),
                    params: json!({ "format": format }).as_object().unwrap().clone(),
                    ..Default::default()
                },
            )
            .unwrap()
            .unwrap()
        };
        assert_eq!(save("{bad}").message.unwrap().theme, "warning");
        save("{type|map:Story=feature,*=fix}/{key}");
        assert_eq!(name_for(&jira, &issue), "feature/P-7");
        assert!(
            host.store
                .borrow()
                .keys()
                .any(|key| key == "branchFormat:https://team.atlassian.net:me@team.com")
        );
        // The page previews a draft on the sample issue, and offers to save it.
        let draft = json!({ "format": "x/{number}" })
            .as_object()
            .unwrap()
            .clone();
        let page = serde_json::to_value(page(&jira, &draft).unwrap())
            .unwrap()
            .to_string();
        assert!(page.contains("`x/123`"), "{}", page);
        assert!(page.contains("Save this format"));
        assert!(page.contains("How formats work"));
    }
}
