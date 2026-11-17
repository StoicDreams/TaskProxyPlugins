//! Jira's fields: their names, finding custom ones like Story Points and Sprint, and showing their values.
use crate::adf;
use crate::client::Jira;
use serde::{Deserialize, Serialize};
use std::cell::RefCell;
use taskproxy_plugin_sdk::ui::{self, Node};
use taskproxy_plugin_sdk::{Map, Result, Value, json};

/// The field list changes rarely, so it is kept for a day.
const CACHE_MS: u64 = 24 * 60 * 60 * 1000;
const CACHE_KEY: &str = "fields";

/// Which story points field each project's boards estimate with, kept for a day.
const ESTIMATION_MS: u64 = 24 * 60 * 60 * 1000;
const ESTIMATION_KEY: &str = "estimation";

const STORY_POINTS_NAMES: [&str; 3] = ["Story point estimate", "Story Points", "Story points"];

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FieldInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub custom: bool,
    /// The schema type, such as string, number, user, array, or option.
    #[serde(default)]
    pub kind: String,
    /// A custom field's type, such as `com.atlassian.jira.plugin.system.customfieldtypes:textarea`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub custom_type: String,
}

/// Fields listed first in the View editor, in this order.
pub const COMMON: [&str; 16] = [
    "summary",
    "status",
    "issuetype",
    "priority",
    "assignee",
    "reporter",
    "description",
    "comment",
    "labels",
    "components",
    "fixVersions",
    "duedate",
    "created",
    "updated",
    "resolution",
    "parent",
];

/// Fields a View never offers, since they are shown another way or are not readable values.
const HIDDEN: [&str; 9] = [
    "attachment",
    "issuelinks",
    "subtasks",
    "worklog",
    "timetracking",
    "thumbnail",
    "watches",
    "votes",
    "lastViewed",
];

pub struct Fields {
    pub list: Vec<FieldInfo>,
    /// The story points field each project's board estimates with, by project key, or "" when none was found. Jira
    /// shows only that one, so it is the one shown here too, even when another story points field has a value.
    estimation: RefCell<Map<String, Value>>,
}

/// The project an issue key belongs to, such as IR for IR-11645.
fn project_of(key: &str) -> &str {
    key.rsplit_once('-')
        .map(|(project, _)| project)
        .unwrap_or(key)
}

fn read_list(body: &Value) -> Vec<FieldInfo> {
    body.as_array()
        .map(|list| {
            list.iter()
                .filter_map(|field| {
                    Some(FieldInfo {
                        id: field.get("id")?.as_str()?.to_string(),
                        name: field.get("name")?.as_str()?.to_string(),
                        custom: field
                            .get("custom")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        kind: field
                            .pointer("/schema/type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        custom_type: field
                            .pointer("/schema/custom")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Fields {
    /// The site's fields, from the plugin's storage when they were read in the last day.
    pub fn load(jira: &Jira) -> Result<Fields> {
        let now = jira.host.now_ms()?;
        let site = jira.site.clone();
        if let Some(cached) = jira.host.store_get(CACHE_KEY)?
            && cached.get("site").and_then(Value::as_str) == Some(site.as_str())
            && let Some(at) = cached.get("at").and_then(Value::as_u64)
            && now.saturating_sub(at) < CACHE_MS
            && let Ok(list) = serde_json::from_value::<Vec<FieldInfo>>(cached["list"].clone())
        {
            return Fields::new(list, jira);
        }
        let list = read_list(&jira.get("field")?);
        jira.host.store_set(
            CACHE_KEY,
            Some(json!({ "site": site, "at": now, "list": list })),
        )?;
        Fields::new(list, jira)
    }

    fn new(list: Vec<FieldInfo>, jira: &Jira) -> Result<Fields> {
        let now = jira.host.now_ms()?;
        let estimation = jira
            .host
            .store_get(ESTIMATION_KEY)?
            .filter(|cached| {
                cached.get("site").and_then(Value::as_str) == Some(jira.site.as_str())
                    && cached
                        .get("at")
                        .and_then(Value::as_u64)
                        .is_some_and(|at| now.saturating_sub(at) < ESTIMATION_MS)
            })
            .and_then(|cached| cached.get("projects").and_then(Value::as_object).cloned())
            .unwrap_or_default();
        Ok(Fields {
            list,
            estimation: RefCell::new(estimation),
        })
    }

    #[cfg(test)]
    pub fn from_list(list: Vec<FieldInfo>) -> Fields {
        Fields {
            list,
            estimation: RefCell::new(Map::new()),
        }
    }

    /// Finds which story points field the boards of these issues' projects estimate with, for projects not looked
    /// up in the last day. Only needed on a site with more than one story points field.
    pub fn learn<'a>(&self, jira: &Jira, keys: impl IntoIterator<Item = &'a str>) {
        let ids = self.story_points_ids();
        if ids.len() < 2 {
            return;
        }
        let mut missing: Vec<String> = Vec::new();
        for key in keys {
            let project = project_of(key).to_string();
            if !project.is_empty()
                && !missing.contains(&project)
                // Entries from before boards were kept are looked up again.
                && !self
                    .estimation
                    .borrow()
                    .get(&project)
                    .is_some_and(Value::is_object)
            {
                missing.push(project);
            }
        }
        if missing.is_empty() {
            return;
        }
        for project in missing {
            let found = estimation_field(jira, &project, &ids)
                .map(|(field, board)| json!({ "field": field, "board": board }))
                .unwrap_or_else(|| json!({}));
            self.estimation.borrow_mut().insert(project, found);
        }
        let projects = self.estimation.borrow().clone();
        if let Ok(now) = jira.host.now_ms() {
            let _ = jira.host.store_set(
                ESTIMATION_KEY,
                Some(json!({ "site": jira.site, "at": now, "projects": projects })),
            );
        }
    }

    /// The story points field Jira shows for this issue's project, when its board says.
    pub fn estimation_of(&self, key: &str) -> Option<String> {
        self.estimation
            .borrow()
            .get(project_of(key))
            .and_then(|found| found.get("field"))
            .and_then(Value::as_str)
            .filter(|id| self.is_story_points(id))
            .map(String::from)
    }

    /// The board whose estimation is that field, which can set it even when the issue's edit screen lacks it.
    pub fn estimation_board(&self, key: &str) -> Option<u64> {
        self.estimation_of(key)?;
        self.estimation
            .borrow()
            .get(project_of(key))
            .and_then(|found| found.get("board"))
            .and_then(Value::as_u64)
    }

    pub fn find(&self, id: &str) -> Option<&FieldInfo> {
        self.list.iter().find(|field| field.id == id)
    }

    pub fn name(&self, id: &str) -> String {
        self.find(id)
            .map(|field| field.name.clone())
            .unwrap_or_else(|| id.to_string())
    }

    fn by_names(&self, names: &[&str]) -> Option<String> {
        names.iter().find_map(|name| {
            self.list
                .iter()
                .find(|field| field.name.eq_ignore_ascii_case(name))
                .map(|field| field.id.clone())
        })
    }

    /// Story points is a custom field, named differently in team-managed and company-managed projects.
    pub fn story_points(&self) -> Option<String> {
        self.story_points_ids().into_iter().next()
    }

    /// Every story points field on the site. Many sites have both "Story point estimate", for team-managed
    /// projects, and "Story Points", for company-managed ones, and an issue's points are in only one of them.
    pub fn story_points_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        for name in STORY_POINTS_NAMES {
            for field in &self.list {
                if field.name.eq_ignore_ascii_case(name) && !ids.contains(&field.id) {
                    ids.push(field.id.clone());
                }
            }
        }
        ids
    }

    pub fn is_story_points(&self, id: &str) -> bool {
        self.story_points_ids().iter().any(|points| points == id)
    }

    /// The field ids to ask Jira for, to show these fields: story points brings every story points field.
    pub fn wanted(&self, reals: &[String]) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for real in reals {
            let more = if self.is_story_points(real) {
                self.story_points_ids()
            } else {
                vec![real.clone()]
            };
            for id in more {
                if !out.contains(&id) {
                    out.push(id);
                }
            }
        }
        out
    }

    /// A field's value on an issue. The key is the issue's own, not one of its fields, and story points come
    /// from whichever of the site's story points fields has a value.
    pub fn value_of(&self, issue: &Value, real: &str) -> Value {
        if matches!(real, "issuekey" | "key") {
            return issue.get("key").cloned().unwrap_or(Value::Null);
        }
        let values = issue.get("fields").unwrap_or(&Value::Null);
        if self.is_story_points(real) {
            let key = issue.get("key").and_then(Value::as_str).unwrap_or("");
            if let Some(id) = self.estimation_of(key) {
                return values.get(&id).cloned().unwrap_or(Value::Null);
            }
            return self
                .story_points_ids()
                .iter()
                .filter_map(|id| values.get(id))
                .find(|value| !value.is_null())
                .cloned()
                .unwrap_or(Value::Null);
        }
        values.get(real).cloned().unwrap_or(Value::Null)
    }

    pub fn sprint(&self) -> Option<String> {
        self.by_names(&["Sprint"])
    }

    /// Every field a View can show: the common ones first, then the rest by name.
    pub fn choices(&self) -> Vec<FieldInfo> {
        let mut first: Vec<FieldInfo> = COMMON
            .iter()
            .filter_map(|id| self.find(id).cloned())
            .collect();
        for id in [self.story_points(), self.sprint()].into_iter().flatten() {
            if let Some(field) = self.find(&id) {
                first.push(field.clone());
            }
        }
        let mut rest: Vec<FieldInfo> = self
            .list
            .iter()
            .filter(|field| !first.iter().any(|chosen| chosen.id == field.id))
            .filter(|field| !HIDDEN.contains(&field.id.as_str()))
            // One story points column covers every story points field.
            .filter(|field| !self.is_story_points(&field.id))
            .cloned()
            .collect();
        rest.sort_by_key(|field| field.name.to_ascii_lowercase());
        first.extend(rest);
        first
    }
}

/// `2026-11-10T14:05:00.000+0000` as `2026-11-10 14:05`, and dates as they are.
pub fn short_date(text: &str) -> String {
    if text.len() >= 16 && text.as_bytes()[10] == b'T' {
        format!("{} {}", &text[..10], &text[11..16])
    } else {
        text.to_string()
    }
}

/// The theme for an issue type's name, so each kind of issue reads at a glance. Bugs are `danger`.
pub fn issue_type_theme(name: &str) -> &'static str {
    match name.trim().to_lowercase().as_str() {
        "bug" | "defect" | "incident" => "danger",
        "story" | "user story" => "success",
        "task" | "sub-task" | "subtask" => "info",
        "epic" | "initiative" => "tertiary",
        "feature" | "improvement" | "new feature" => "primary",
        "spike" | "research" => "warning",
        _ => "",
    }
}

/// Whether an issue is a bug, by its type.
pub fn is_bug(issue: &Value) -> bool {
    issue
        .pointer("/fields/issuetype/name")
        .and_then(Value::as_str)
        .is_some_and(|name| issue_type_theme(name) == "danger")
}

/// A table row's theme: `danger` when its issue or its parent is a bug. Jira's `parent` field carries the parent's
/// type, so a parent that was not fetched still counts.
pub fn row_theme(issue: &Value, parent: Option<&Value>) -> String {
    let parent_is_bug =
        parent.is_some_and(is_bug) || issue.pointer("/fields/parent").is_some_and(is_bug);
    if is_bug(issue) || parent_is_bug {
        "danger".into()
    } else {
        String::new()
    }
}

/// The theme for a status, by its category: to do, in progress, or done.
pub fn status_theme(status: &Value) -> &'static str {
    match status
        .pointer("/statusCategory/key")
        .and_then(Value::as_str)
    {
        Some("indeterminate") => "info",
        Some("done") => "success",
        _ => "",
    }
}

/// A value as short text, for tables and lists of values.
pub fn value_text(value: &Value) -> String {
    match value {
        Value::Null => String::new(),
        Value::String(text) => short_date(text),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => if *flag { "Yes" } else { "No" }.to_string(),
        Value::Array(list) => list
            .iter()
            .map(value_text)
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(object) => {
            if object.get("type").and_then(Value::as_str) == Some("doc") {
                return adf::to_markdown(value);
            }
            for key in ["displayName", "name", "value", "key"] {
                if let Some(text) = object.get(key).and_then(Value::as_str) {
                    return text.to_string();
                }
            }
            String::new()
        }
    }
}

/// A field's value, drawn for an issue: rich text as markdown, statuses as badges, and comments in full.
pub fn value_node(id: &str, value: &Value) -> Node {
    if value.is_null() || value.as_array().is_some_and(Vec::is_empty) {
        return ui::muted("None");
    }
    if id == "comment" {
        return comments(value);
    }
    if id == "status" {
        return ui::badge(value_text(value), status_theme(value));
    }
    if value.get("type").and_then(Value::as_str) == Some("doc") {
        let markdown = adf::to_markdown(value);
        return if markdown.is_empty() {
            ui::muted("None")
        } else {
            ui::markdown(markdown)
        };
    }
    if id == "parent" {
        let summary = value
            .pointer("/fields/summary")
            .and_then(Value::as_str)
            .unwrap_or("");
        return ui::text(
            format!("{} {}", value_text(value), summary)
                .trim()
                .to_string(),
        );
    }
    let text = value_text(value);
    if text.is_empty() {
        ui::muted("None")
    } else {
        ui::text(text)
    }
}

fn comments(value: &Value) -> Node {
    let list = value
        .get("comments")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if list.is_empty() {
        return ui::muted("No comments");
    }
    ui::stack(
        list.iter()
            .map(|comment| {
                let author = comment
                    .pointer("/author/displayName")
                    .and_then(Value::as_str)
                    .unwrap_or("Someone");
                let when = short_date(comment.get("created").and_then(Value::as_str).unwrap_or(""));
                let body = adf::to_markdown(comment.get("body").unwrap_or(&Value::Null));
                ui::stack(vec![
                    ui::row(vec![ui::strong(author), ui::muted(when)]),
                    ui::markdown(body),
                ])
            })
            .collect(),
    )
}

/// The story points field a project's boards estimate with, from the first board that estimates with one.
fn estimation_field(jira: &Jira, project: &str, ids: &[String]) -> Option<(String, u64)> {
    let boards = jira
        .agile_get(&format!(
            "board?projectKeyOrId={}&maxResults=20",
            crate::client::encode(project)
        ))
        .ok()?;
    for board in boards.get("values").and_then(Value::as_array)? {
        let Some(id) = board.get("id").and_then(Value::as_u64) else {
            continue;
        };
        let Ok(config) = jira.agile_get(&format!("board/{}/configuration", id)) else {
            continue;
        };
        if let Some(field) = config
            .pointer("/estimation/field/fieldId")
            .and_then(Value::as_str)
            .filter(|field| ids.iter().any(|id| id == field))
        {
            return Some((field.to_string(), id));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FakeHost, sample_fields};

    #[test]
    fn values_come_from_the_key_and_whichever_story_points_field_is_set() {
        let fields = Fields::from_list(read_list(&sample_fields()));
        let story = json!({ "key": "P-10", "fields": {
            "summary": "Story", "customfield_10016": null, "customfield_10026": 5
        } });
        assert_eq!(fields.value_of(&story, "issuekey"), json!("P-10"));
        assert_eq!(fields.value_of(&story, "key"), json!("P-10"));
        assert_eq!(fields.value_of(&story, "summary"), json!("Story"));
        assert_eq!(fields.value_of(&story, "customfield_10016"), json!(5));
        assert_eq!(fields.value_of(&story, "customfield_10026"), json!(5));
        assert_eq!(fields.value_of(&story, "missing"), Value::Null);
        let team = json!({ "key": "T-1", "fields": { "customfield_10016": 3 } });
        assert_eq!(fields.value_of(&team, "customfield_10016"), json!(3));
        assert_eq!(
            fields.wanted(&["summary".into(), "customfield_10016".into()]),
            vec!["summary", "customfield_10016", "customfield_10026"]
        );
    }

    #[test]
    fn story_points_show_from_the_field_the_projects_board_estimates_with() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.answer(
            "GET",
            "/rest/agile/1.0/board?projectKeyOrId=IR&maxResults=20",
            200,
            json!({ "values": [{ "id": 7 }, { "id": 8 }] }),
        );
        host.answer(
            "GET",
            "/rest/agile/1.0/board?projectKeyOrId=T&maxResults=20",
            200,
            json!({ "values": [] }),
        );
        // The first board is kanban with no estimation; the second estimates with "Story Points".
        host.answer(
            "GET",
            "/rest/agile/1.0/board/7/configuration",
            200,
            json!({ "id": 7 }),
        );
        host.answer("GET", "/rest/agile/1.0/board/8/configuration", 200, json!({ "estimation": {
            "type": "field", "field": { "fieldId": "customfield_10026", "displayName": "Story Points" }
        } }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        // Jira shows 3 in Story Points, and an older 2 is left in Story point estimate.
        let parent = json!({ "key": "IR-11645", "fields": { "customfield_10016": 2, "customfield_10026": 3 } });
        let unset = json!({ "key": "IR-11646", "fields": { "customfield_10016": 2, "customfield_10026": null } });
        let team = json!({ "key": "T-1", "fields": { "customfield_10016": 5 } });
        assert_eq!(
            fields.value_of(&parent, "customfield_10016"),
            json!(2),
            "not learned yet"
        );
        fields.learn(&jira, ["IR-11645", "IR-11646", "T-1"]);
        assert_eq!(fields.value_of(&parent, "customfield_10016"), json!(3));
        assert_eq!(
            fields.value_of(&unset, "customfield_10026"),
            Value::Null,
            "Jira shows none"
        );
        assert_eq!(
            fields.value_of(&team, "customfield_10016"),
            json!(5),
            "no board, so whichever is set"
        );
        // Learned once, and kept for the next load.
        let asked = host.sent.borrow().len();
        fields.learn(&jira, ["IR-1"]);
        let again = Fields::load(&jira).unwrap();
        again.learn(&jira, ["IR-2", "T-2"]);
        assert_eq!(host.sent.borrow().len(), asked);
        assert_eq!(again.value_of(&parent, "customfield_10026"), json!(3));
    }

    #[test]
    fn fields_are_read_once_a_day_and_custom_ones_found() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        assert_eq!(fields.story_points().as_deref(), Some("customfield_10016"));
        assert_eq!(fields.sprint().as_deref(), Some("customfield_10020"));
        assert_eq!(
            fields.story_points_ids(),
            vec!["customfield_10016", "customfield_10026"]
        );
        assert_eq!(fields.name("customfield_10050"), "Acceptance criteria");
        Fields::load(&jira).unwrap();
        assert_eq!(
            host.sent.borrow().len(),
            1,
            "the second load used the stored list"
        );
        host.now.set(host.now.get() + CACHE_MS + 1);
        Fields::load(&jira).unwrap();
        assert_eq!(host.sent.borrow().len(), 2, "a day later it asked again");
        let choices: Vec<String> = fields.choices().into_iter().map(|field| field.id).collect();
        assert_eq!(
            choices,
            vec![
                "summary",
                "status",
                "priority",
                "assignee",
                "description",
                "comment",
                "customfield_10016",
                "customfield_10020",
                "customfield_10050",
                "issuekey",
                "customfield_10040"
            ]
        );
    }

    #[test]
    fn values_are_shown_by_their_kind() {
        assert_eq!(value_node("assignee", &Value::Null), ui::muted("None"));
        assert_eq!(value_node("labels", &json!([])), ui::muted("None"));
        assert_eq!(
            value_node("assignee", &json!({ "displayName": "Sam Lee" })),
            ui::text("Sam Lee")
        );
        assert_eq!(
            value_node("labels", &json!(["web", "auth"])),
            ui::text("web, auth")
        );
        assert_eq!(value_node("customfield_10016", &json!(5)), ui::text("5"));
        assert_eq!(
            value_node(
                "status",
                &json!({ "name": "In Progress", "statusCategory": { "key": "indeterminate" } })
            ),
            ui::badge("In Progress", "info")
        );
        assert_eq!(
            value_node("created", &json!("2026-11-10T14:05:31.000+0000")),
            ui::text("2026-11-10 14:05")
        );
        assert_eq!(
            value_node(
                "customfield_10020",
                &json!([{ "id": 3, "name": "Sprint 12", "state": "active" }])
            ),
            ui::text("Sprint 12")
        );
        let doc = json!({ "type": "doc", "version": 1, "content": [
            { "type": "paragraph", "content": [{ "type": "text", "text": "Hello" }] }
        ] });
        assert_eq!(value_node("description", &doc), ui::markdown("Hello"));
        assert_eq!(
            value_node(
                "parent",
                &json!({ "key": "P-1", "fields": { "summary": "Login" } })
            ),
            ui::text("P-1 Login")
        );
    }

    #[test]
    fn comments_show_who_when_and_what() {
        let value = json!({ "comments": [{
            "author": { "displayName": "Sam Lee" }, "created": "2026-11-10T14:05:31.000+0000",
            "body": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "Done" }] }] }
        }] });
        assert_eq!(
            value_node("comment", &value),
            ui::stack(vec![ui::stack(vec![
                ui::row(vec![ui::strong("Sam Lee"), ui::muted("2026-11-10 14:05")]),
                ui::markdown("Done"),
            ])])
        );
        assert_eq!(
            value_node("comment", &json!({ "comments": [] })),
            ui::muted("No comments")
        );
    }
}
