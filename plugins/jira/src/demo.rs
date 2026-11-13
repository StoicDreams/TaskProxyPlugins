//! The Jira plugin's pages drawn from made-up Jira data, for the TaskProxy.com screenshots. The UI comes from the
//! plugin's own code, so the screenshots show what it really draws.
//!
//! Run with: cargo run -p taskproxy-jira --example demo -- <folder>
use crate::JiraPlugin;
use crate::testing::FakeHost;
use taskproxy_plugin_sdk::*;

fn doc(paragraphs: &[&str]) -> Value {
    let content: Vec<Value> = paragraphs
        .iter()
        .map(|text| {
            if let Some(item) = text.strip_prefix("- ") {
                json!({ "type": "bulletList", "content": [{ "type": "listItem", "content": [
                    { "type": "paragraph", "content": [{ "type": "text", "text": item }] }
                ] }] })
            } else {
                json!({ "type": "paragraph", "content": [{ "type": "text", "text": text }] })
            }
        })
        .collect();
    json!({ "type": "doc", "version": 1, "content": content })
}

fn user(name: &str) -> Value {
    json!({ "displayName": name, "accountId": name.to_lowercase().replace(' ', "-") })
}

fn status(name: &str, category: &str) -> Value {
    json!({ "name": name, "statusCategory": { "key": category } })
}

fn fields() -> Value {
    json!([
        { "id": "summary", "name": "Summary", "schema": { "type": "string" } },
        { "id": "status", "name": "Status", "schema": { "type": "status" } },
        { "id": "issuetype", "name": "Issue Type", "schema": { "type": "issuetype" } },
        { "id": "priority", "name": "Priority", "schema": { "type": "priority" } },
        { "id": "assignee", "name": "Assignee", "schema": { "type": "user" } },
        { "id": "reporter", "name": "Reporter", "schema": { "type": "user" } },
        { "id": "description", "name": "Description", "schema": { "type": "string" } },
        { "id": "comment", "name": "Comment", "schema": { "type": "comments-page" } },
        { "id": "labels", "name": "Labels", "schema": { "type": "array" } },
        { "id": "duedate", "name": "Due date", "schema": { "type": "date" } },
        { "id": "created", "name": "Created", "schema": { "type": "datetime" } },
        { "id": "updated", "name": "Updated", "schema": { "type": "datetime" } },
        { "id": "parent", "name": "Parent", "schema": { "type": "issuelink" } },
        { "id": "customfield_10016", "name": "Story point estimate", "custom": true, "schema": { "type": "number" } },
        { "id": "customfield_10020", "name": "Sprint", "custom": true, "schema": { "type": "array" } },
        { "id": "customfield_10050", "name": "Acceptance criteria", "custom": true, "schema": { "type": "string" } }
    ])
}

fn issue(key: &str, kind: &str, summary: &str, extra: Value) -> Value {
    let mut fields = json!({ "summary": summary, "issuetype": { "name": kind } });
    for (name, value) in extra.as_object().unwrap() {
        fields[name] = value.clone();
    }
    json!({ "key": key, "fields": fields })
}

/// A host answering like a Jira site for a made-up Atlas project.
pub fn host() -> FakeHost {
    let host = FakeHost::connected();
    host.project_open.set(true);
    host.answer("GET", "/rest/api/3/field", 200, fields());
    let sprint = json!([{ "name": "Atlas Sprint 14", "state": "active" }]);
    let row = |key: &str, summary: &str, state: Value, priority: &str, points: u32| {
        issue(
            key,
            "Task",
            summary,
            json!({
                "status": state, "priority": { "name": priority }, "updated": "2026-11-11T16:42:00.000+0000",
                "customfield_10016": points, "customfield_10020": sprint.clone()
            }),
        )
    };
    host.answer("POST", "/rest/api/3/search/jql", 200, json!({ "isLast": true, "issues": [
        row("ATL-142", "Show CI status on the dashboard", status("In Progress", "indeterminate"), "High", 5),
        row("ATL-139", "Cache release notes between sessions", status("In Review", "indeterminate"), "Medium", 3),
        row("ATL-151", "Retry failed webhook deliveries", status("To Do", "new"), "High", 8),
        row("ATL-156", "Add keyboard shortcut for search", status("To Do", "new"), "Low", 2),
        row("ATL-133", "Fix timezone in deploy summaries", status("Done", "done"), "Medium", 3),
    ] }));
    host.answer("GET", "/rest/api/3/issue/ATL-142", 200, issue("ATL-142", "Task", "Show CI status on the dashboard", json!({
        "status": status("In Progress", "indeterminate"), "priority": { "name": "High" },
        "assignee": user("Sam Lee"), "reporter": user("Priya Shah"), "customfield_10016": 5,
        "customfield_10020": sprint,
        "description": doc(&[
            "Show the latest CI result next to each repo on the dashboard, so a failed build is seen right away.",
            "- Use the same icons as the Git Controller",
            "- Refresh when a run finishes",
        ]),
        "comment": { "comments": [
            { "author": user("Priya Shah"), "created": "2026-11-10T09:12:00.000+0000",
              "body": doc(&["Design is in the Release Visibility doc. The icon should match the repo list."]) },
            { "author": user("Sam Lee"), "created": "2026-11-11T15:30:00.000+0000",
              "body": doc(&["Icons are in. Working on refreshing when a run finishes."]) }
        ] },
        "parent": { "key": "ATL-120" }
    })));
    host.answer("GET", "/rest/api/3/issue/ATL-120", 200, issue("ATL-120", "Story", "Release visibility for teams", json!({
        "status": status("In Progress", "indeterminate"), "priority": { "name": "High" },
        "assignee": user("Priya Shah"), "reporter": user("Morgan Diaz"), "customfield_10016": 13,
        "description": doc(&["As a team lead, I want to see what is building and shipping without opening another tool."]),
        "customfield_10050": doc(&[
            "- Every repo shows its latest CI result",
            "- Failed runs stand out at a glance",
        ]),
        "comment": { "comments": [
            { "author": user("Morgan Diaz"), "created": "2026-11-04T13:05:00.000+0000",
              "body": doc(&["Let's get the dashboard part into Sprint 14."]) }
        ] },
        "parent": { "key": "ATL-100" }
    })));
    host.answer("GET", "/rest/api/3/issue/ATL-100", 200, issue("ATL-100", "Epic", "Q4 developer experience", json!({
        "status": status("In Progress", "indeterminate"),
        "description": doc(&["Cut the time developers spend switching between tools by a third this quarter."]),
    })));
    host.answer(
        "GET",
        "/rest/api/3/issue/ATL-142/transitions",
        200,
        json!({ "transitions": [
        { "id": "21", "name": "Start review", "to": { "name": "In Review" } },
        { "id": "31", "name": "Done", "to": { "name": "Done" } },
        { "id": "11", "name": "Stop progress", "to": { "name": "To Do" } }
    ] }),
    );
    host.answer(
        "GET",
        "/rest/api/3/project/search",
        200,
        json!({ "values": [
        { "key": "ATL", "name": "Atlas" }, { "key": "OPS", "name": "Operations" }
    ] }),
    );
    host.answer("GET", "/rest/api/3/issue/createmeta/ATL/issuetypes", 200, json!({ "issueTypes": [
        { "id": "10001", "name": "Task" }, { "id": "10002", "name": "Story" }, { "id": "10003", "name": "Bug" },
        { "id": "10005", "name": "Sub-task", "subtask": true }
    ] }));
    let team_view = json!([{
        "id": "team-view", "name": "Team view",
        "layers": [
            ["status", "assignee", "reporter", "priority", "storyPoints", "sprint", "description", "comment"],
            ["status", "assignee", "storyPoints", "description", "customfield_10050", "comment"],
            ["description"]
        ]
    }]);
    host.files.borrow_mut().insert(
        "views.json".into(),
        serde_json::to_string_pretty(&team_view).unwrap(),
    );
    host.store
        .borrow_mut()
        .insert("lastView".into(), json!("project:team-view"));
    host.store.borrow_mut().insert(
        "filters".into(),
        json!([{ "name": "Sprint 14", "jql": "sprint in openSprints() AND project = ATL" },
               { "name": "Bugs I reported", "jql": "reporter = currentUser() AND type = Bug" }]),
    );
    host
}

fn params(pairs: &[(&str, &str)]) -> Map<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| (key.to_string(), json!(value)))
        .collect()
}

/// Each screen, by name.
pub fn screens() -> Result<Vec<(&'static str, Value)>> {
    let host = host();
    let render = |page: &str, pairs: &[(&str, &str)]| -> Result<Value> {
        let node = JiraPlugin.render(
            &host,
            RenderRequest {
                page: page.into(),
                params: params(pairs),
            },
        )?;
        Ok(serde_json::to_value(node)?)
    };
    let mut screens = vec![
        ("issues", render("issues", &[])?),
        ("issue", render("issue", &[("key", "ATL-142")])?),
        ("views", render("views", &[])?),
        ("create", render("create", &[("project", "ATL")])?),
    ];
    JiraPlugin.action(
        &host,
        ActionRequest {
            page: "views".into(),
            action: "views.edit".into(),
            params: params(&[("scope", "project"), ("id", "team-view")]),
            ..Default::default()
        },
    )?;
    screens.push(("view-editor", render("views", &[("edit", "1")])?));
    let added = JiraPlugin.action(
        &host,
        ActionRequest {
            page: "views".into(),
            action: "draft.add".into(),
            params: json!({ "layer": 2 })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            form: json!({ "field": "status" })
                .as_object()
                .cloned()
                .unwrap_or_default(),
            ..Default::default()
        },
    )?;
    screens.push(("view-editor-added", serde_json::to_value(added.ui)?));
    let card = JiraPlugin.branch_info(
        &host,
        BranchInfoRequest {
            branch: "ATL-142-show-ci-status-on-the-dashboard".into(),
        },
    )?;
    screens.push(("branch-card", serde_json::to_value(card)?));
    Ok(screens)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_demo_screen_renders() {
        let screens = screens().unwrap();
        assert_eq!(screens.len(), 7);
        let issue = serde_json::to_string(&screens[1].1).unwrap();
        for expected in [
            "Parent · Story ATL-120",
            "Grandparent · Epic ATL-100",
            "Atlas Sprint 14",
            "Sam Lee",
        ] {
            assert!(issue.contains(expected), "{} in {}", expected, issue);
        }
        assert!(!screens[5].1.is_null());
        assert!(!screens[6].1.is_null());
    }
}
