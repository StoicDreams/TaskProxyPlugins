//! The Jira plugin's pages: one issue through a View, and Create Issue. My Issues and All Issues are in `list`.
use crate::adf;
use crate::branch;
use crate::client::{Jira, encode};
use crate::fields::{self, Fields};
use crate::list;
use crate::views;
use taskproxy_plugin_sdk::ui::{self, Action, Choice, Field, FieldKind, Node};
use taskproxy_plugin_sdk::{
    ActionRequest, ActionResult, Error, HostAction, Map, Result, Value, json,
};

const LAST_VIEW_KEY: &str = "lastView";

pub fn param<'a>(params: &'a Map<String, Value>, name: &str) -> &'a str {
    params.get(name).and_then(Value::as_str).unwrap_or("")
}

fn form<'a>(request: &'a ActionRequest, name: &str) -> &'a str {
    request
        .form
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
}

fn params(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect()
}

pub fn open_issue(key: &str) -> Action {
    Action::new("open", format!("Open {}", key)).param("key", key)
}

/// One issue, through the chosen View, with its status, comment box, and actions.
pub fn issue(jira: &Jira, fields: &Fields, page_params: &Map<String, Value>) -> Result<Node> {
    let key = param(page_params, "key").trim().to_uppercase();
    if key.is_empty() {
        return Ok(ui::notice(
            "Open an issue from My Issues or All Issues.",
            "info",
        ));
    }
    let view_key = match param(page_params, "view") {
        "" => jira
            .host
            .store_get(LAST_VIEW_KEY)?
            .and_then(|value| value.as_str().map(String::from))
            .unwrap_or_else(|| views::DEFAULT_ID.to_string()),
        view => view.to_string(),
    };
    let view = views::find(jira.host, &view_key)?;
    let (issue, sections) = views::layers(jira, fields, &view, &key)?;
    let values = issue.get("fields").cloned().unwrap_or(Value::Null);
    let summary = values.get("summary").and_then(Value::as_str).unwrap_or("");
    let kind = values
        .pointer("/issuetype/name")
        .and_then(Value::as_str)
        .unwrap_or("Issue");
    let transitions = jira.get(&format!("issue/{}/transitions", encode(&key)))?;
    let moves: Vec<Choice> = transitions
        .get("transitions")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|transition| {
            let id = transition.get("id")?.as_str()?;
            let name = transition.get("name")?.as_str()?;
            let to = transition
                .pointer("/to/name")
                .and_then(Value::as_str)
                .unwrap_or(name);
            Some(Choice::new(
                id,
                if to == name {
                    name.to_string()
                } else {
                    format!("{} → {}", name, to)
                },
            ))
        })
        .collect();
    let view_choices: Vec<Choice> = views::all(jira.host)?
        .into_iter()
        .map(|(scope, view)| Choice::new(views::view_key(scope, &view.id), view.name))
        .collect();
    let mut children = vec![
        ui::row(vec![
            ui::badge(kind, ""),
            fields::value_node("status", values.get("status").unwrap_or(&Value::Null)),
            ui::heading(format!("{} {}", key, summary), 2),
        ]),
        ui::toolbar(vec![
            ui::button(
                Action::host(
                    "Open in Jira",
                    HostAction::OpenUrl {
                        url: jira.browse_url(&key),
                    },
                )
                .icon("arrow-corner-from-square"),
                "",
            ),
            ui::button(Action::new("issue.refresh", "Refresh").icon("refresh"), ""),
        ]),
        branch_form(&branch::name_for(jira, &issue)),
        Node::Form {
            fields: vec![
                Field::new(FieldKind::Select, "view", "View")
                    .options(view_choices)
                    .value(if view.id == views::DEFAULT_ID {
                        views::DEFAULT_ID
                    } else {
                        view_key.as_str()
                    }),
            ],
            submit: Action::new("issue.view", "Show").param("key", key.clone()),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: false,
        },
    ];
    if !moves.is_empty() {
        children.push(Node::Form {
            fields: vec![Field::new(FieldKind::Select, "transition", "Move to").options(moves)],
            submit: Action::new("issue.transition", "Move").param("key", key.clone()),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: false,
        });
    }
    children.extend(sections);
    children.push(Node::Form {
        fields: vec![list::comment_field(&key, "Add a comment")],
        submit: Action::new("issue.comment", "Comment").param("key", key.clone()),
        cancel: None,
        buttons: Vec::new(),
        children: Vec::new(),
        inline: false,
    });
    Ok(ui::stack(children))
}

/// Creates a branch for the issue, named in the sign-in's format, which the name box can change first. Task Proxy
/// 0.1.122 and earlier do not take the name from the box, so they create the suggested one.
fn branch_form(suggested: &str) -> Node {
    Node::Form {
        fields: vec![
            Field::new(FieldKind::Text, "name", "Branch name")
                .value(suggested)
                .required(),
        ],
        submit: Action::host(
            "Create branch",
            HostAction::GitCreateBranch {
                name: suggested.to_string(),
            },
        )
        .icon("code-branch")
        .confirm("Create this branch from the current branch and switch to it?"),
        cancel: None,
        buttons: vec![ui::button(
            Action::new("branches.open", "Branch format").icon("gear"),
            "secondary",
        )],
        children: Vec::new(),
        inline: true,
    }
}

pub fn me(jira: &Jira) -> Result<String> {
    let site_key = format!("me:{}", jira.site);
    if let Some(id) = jira
        .host
        .store_get(&site_key)?
        .and_then(|value| value.as_str().map(String::from))
    {
        return Ok(id);
    }
    let id = jira
        .get("myself")?
        .get("accountId")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("Jira did not say who you are."))?
        .to_string();
    jira.host.store_set(&site_key, Some(json!(id)))?;
    Ok(id)
}

/// Create Issue: choose a project first, since the issue types depend on it.
pub fn create(jira: &Jira, page_params: &Map<String, Value>) -> Result<Node> {
    let projects = jira.get("project/search?maxResults=100&orderBy=name")?;
    let projects: Vec<Choice> = projects
        .get("values")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|project| {
            let key = project.get("key")?.as_str()?;
            let name = project.get("name")?.as_str()?;
            Some(Choice::new(key, format!("{} ({})", name, key)))
        })
        .collect();
    if projects.is_empty() {
        return Ok(ui::empty("You cannot create issues in any Jira project."));
    }
    let project = param(page_params, "project");
    let project_form = Node::Form {
        fields: vec![
            Field::new(FieldKind::Select, "project", "Project")
                .options(projects)
                .value(project),
        ],
        submit: Action::new("create.project", "Choose"),
        cancel: None,
        buttons: Vec::new(),
        children: Vec::new(),
        inline: false,
    };
    if project.is_empty() {
        return Ok(ui::stack(vec![project_form]));
    }
    let types = jira.get(&format!(
        "issue/createmeta/{}/issuetypes?maxResults=100",
        encode(project)
    ))?;
    let types: Vec<Choice> = types
        .get("issueTypes")
        .or_else(|| types.get("values"))
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|kind| {
            !kind
                .get("subtask")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .filter_map(|kind| {
            Some(Choice::new(
                kind.get("id")?.as_str()?,
                kind.get("name")?.as_str()?,
            ))
        })
        .collect();
    let first_type = types
        .first()
        .map(|choice| choice.value.clone())
        .unwrap_or_default();
    Ok(ui::stack(vec![
        project_form,
        Node::Form {
            fields: vec![
                Field::new(FieldKind::Select, "type", "Type")
                    .options(types)
                    .value(first_type)
                    .required(),
                Field::new(FieldKind::Text, "summary", "Summary").required(),
                Field::new(FieldKind::Textarea, "description", "Description")
                    .placeholder("Markdown: **bold**, *italic*, `code`, lists, and links"),
                Field::new(FieldKind::Text, "parent", "Parent")
                    .placeholder("PROJ-123")
                    .help("Optional: the issue this belongs under."),
                Field::new(FieldKind::Checkbox, "assignToMe", "Assign to me").value(true),
            ],
            submit: Action::new("create.submit", "Create issue").param("project", project),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: false,
        },
    ]))
}

/// Answers the actions of My Issues, the issue page, and Create Issue. `None` when it is not one of them.
pub fn action(jira: &Jira, request: &ActionRequest) -> Result<Option<ActionResult>> {
    let p = &request.params;
    let result = match request.action.as_str() {
        "open" => ActionResult::navigate("issue", params(&[("key", param(p, "key"))])),
        "issue.refresh" => ActionResult::refresh(),
        "issue.view" => {
            let view = form(request, "view");
            jira.host.store_set(LAST_VIEW_KEY, Some(json!(view)))?;
            ActionResult::navigate("issue", params(&[("key", param(p, "key")), ("view", view)]))
        }
        "issue.transition" => {
            let transition = form(request, "transition");
            if transition.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Choose where to move it.", "warning"),
                ));
            }
            let key = param(p, "key");
            jira.post(
                &format!("issue/{}/transitions", encode(key)),
                &json!({ "transition": { "id": transition } }),
            )?;
            ActionResult::refresh().with_message(format!("{} moved.", key), "success")
        }
        "issue.comment" => {
            let body = form(request, "body");
            if body.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Write a comment first.", "warning"),
                ));
            }
            let key = param(p, "key");
            jira.post(
                &format!("issue/{}/comment", encode(key)),
                &json!({ "body": adf::from_markdown_with_mentions(body, &list::mentions(request, "body")) }),
            )?;
            ActionResult::refresh().with_message("Comment added.", "success")
        }
        "create.project" => {
            ActionResult::navigate("create", params(&[("project", form(request, "project"))]))
        }
        "create.submit" => {
            let summary = form(request, "summary");
            let kind = form(request, "type");
            if summary.is_empty() || kind.is_empty() {
                return Ok(Some(
                    ActionResult::default()
                        .with_message("An issue needs a type and a summary.", "warning"),
                ));
            }
            let mut values = json!({
                "project": { "key": param(p, "project") },
                "issuetype": { "id": kind },
                "summary": summary,
            });
            let description = form(request, "description");
            if !description.is_empty() {
                values["description"] = adf::from_markdown(description);
            }
            let parent = form(request, "parent").to_uppercase();
            if !parent.is_empty() {
                values["parent"] = json!({ "key": parent });
            }
            if request
                .form
                .get("assignToMe")
                .is_some_and(|value| value == &json!(true) || value == "true" || value == "on")
            {
                values["assignee"] = json!({ "accountId": me(jira)? });
            }
            let created = jira.post("issue", &json!({ "fields": values }))?;
            let key = created
                .get("key")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            ActionResult::navigate("issue", params(&[("key", &key)]))
                .with_message(format!("Created {}.", key), "success")
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

/// The first issue key in a branch name, such as PROJ-12 in feature/proj-12-login.
pub fn issue_key(branch: &str) -> Option<String> {
    let upper = branch.to_uppercase();
    let bytes = upper.as_bytes();
    let mut start = 0;
    while start < bytes.len() {
        let at_boundary = start == 0 || !bytes[start - 1].is_ascii_alphanumeric();
        if at_boundary && bytes[start].is_ascii_uppercase() {
            let mut end = start + 1;
            while end < bytes.len()
                && (bytes[end].is_ascii_uppercase() || bytes[end].is_ascii_digit())
            {
                end += 1;
            }
            if end < bytes.len() && bytes[end] == b'-' {
                let digits_start = end + 1;
                let mut digits_end = digits_start;
                while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
                    digits_end += 1;
                }
                let after_ok =
                    digits_end == bytes.len() || !bytes[digits_end].is_ascii_alphanumeric();
                if digits_end > digits_start && after_ok && end - start >= 2 {
                    return Some(upper[start..digits_end].to_string());
                }
            }
        }
        start += 1;
    }
    None
}

/// The card for the Git Controller: the branch's issue, its status, and who has it.
pub fn branch_card(jira: &Jira, branch: &str) -> Result<Option<Node>> {
    let Some(key) = issue_key(branch) else {
        return Ok(None);
    };
    let issue = match jira.get(&format!(
        "issue/{}?fields=summary,status,assignee,issuetype",
        encode(&key)
    )) {
        Ok(issue) => issue,
        Err(err) if err.0.starts_with("Jira could not find") => return Ok(None),
        Err(err) => return Err(err),
    };
    let values = issue.get("fields").cloned().unwrap_or(Value::Null);
    let assignee = fields::value_text(values.get("assignee").unwrap_or(&Value::Null));
    Ok(Some(ui::stack(vec![
        ui::row(vec![
            fields::value_node("status", values.get("status").unwrap_or(&Value::Null)),
            ui::link(key.clone(), open_issue(&key)),
            ui::text(
                values
                    .get("summary")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            ),
        ]),
        ui::muted(if assignee.is_empty() {
            "Unassigned".to_string()
        } else {
            format!("Assigned to {}", assignee)
        }),
    ])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::FakeHost;

    fn request(action: &str, params: Value, form: Value) -> ActionRequest {
        ActionRequest {
            page: "issue".into(),
            action: action.into(),
            params: params.as_object().cloned().unwrap_or_default(),
            form: form.as_object().cloned().unwrap_or_default(),
            ..Default::default()
        }
    }

    #[test]
    fn branch_names_find_their_issue() {
        for (branch, key) in [
            ("PROJ-12-fix-login", Some("PROJ-12")),
            ("feature/proj-12-login", Some("PROJ-12")),
            ("bugfix/AB2-7", Some("AB2-7")),
            ("main", None),
            // Looks like a key, so Jira is asked, and its "not found" hides the card.
            ("release-2026", Some("RELEASE-2026")),
            ("x-1", None),
            ("PROJ-12abc", None),
        ] {
            assert_eq!(issue_key(branch).as_deref(), key, "{}", branch);
        }
    }

    #[test]
    fn actions_comment_move_and_create() {
        let host = FakeHost::connected();
        host.answer(
            "POST",
            "/rest/api/3/issue/P-3/comment",
            201,
            json!({ "id": "1" }),
        );
        host.answer(
            "POST",
            "/rest/api/3/issue/P-3/transitions",
            204,
            Value::Null,
        );
        host.answer(
            "GET",
            "/rest/api/3/myself",
            200,
            json!({ "accountId": "acc-1" }),
        );
        host.answer("POST", "/rest/api/3/issue", 201, json!({ "key": "P-9" }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let commented = action(
            &jira,
            &request(
                "issue.comment",
                json!({ "key": "P-3" }),
                json!({ "body": "**Done**" }),
            ),
        )
        .unwrap()
        .unwrap();
        assert!(commented.refresh);
        let moved = action(
            &jira,
            &request(
                "issue.transition",
                json!({ "key": "P-3" }),
                json!({ "transition": "31" }),
            ),
        )
        .unwrap()
        .unwrap();
        assert_eq!(moved.message.unwrap().text, "P-3 moved.");
        let created = action(&jira, &request(
            "create.submit",
            json!({ "project": "P" }),
            json!({ "type": "10001", "summary": "New thing", "description": "Why", "parent": "p-1", "assignToMe": true }),
        )).unwrap().unwrap();
        assert_eq!(created.navigate.unwrap().params["key"], "P-9");
        let sent = host.sent.borrow();
        let comment: Value = serde_json::from_str(sent[0].body.as_deref().unwrap()).unwrap();
        assert_eq!(
            comment["body"]["content"][0]["content"][0]["marks"][0]["type"],
            "strong"
        );
        let issue: Value = serde_json::from_str(sent[3].body.as_deref().unwrap()).unwrap();
        assert_eq!(issue["fields"]["parent"]["key"], "P-1");
        assert_eq!(issue["fields"]["assignee"]["accountId"], "acc-1");
        assert_eq!(issue["fields"]["description"]["type"], "doc");
        let empty = action(
            &jira,
            &request(
                "issue.comment",
                json!({ "key": "P-3" }),
                json!({ "body": "  " }),
            ),
        )
        .unwrap()
        .unwrap();
        assert_eq!(empty.message.unwrap().theme, "warning");
    }

    #[test]
    fn the_branch_card_shows_its_issue_or_nothing() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/issue/PJ-3?fields=summary,status,assignee,issuetype", 200, json!({
            "key": "PJ-3", "fields": { "summary": "Fix login", "status": { "name": "In Progress" }, "assignee": null }
        }));
        host.answer(
            "GET",
            "/rest/api/3/issue/PJ-404?fields=summary,status,assignee,issuetype",
            404,
            json!({}),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let card =
            serde_json::to_string(&branch_card(&jira, "PJ-3-fix-login").unwrap().unwrap()).unwrap();
        assert!(
            card.contains("Fix login") && card.contains("Unassigned"),
            "{}",
            card
        );
        assert_eq!(branch_card(&jira, "main").unwrap(), None);
        assert_eq!(branch_card(&jira, "feature/pj-404-gone").unwrap(), None);
    }
}
