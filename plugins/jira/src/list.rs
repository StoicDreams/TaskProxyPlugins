//! My Issues and All Issues: a search built from filters and JQL, columns chosen from the issue, its parent, and
//! its grandparent, titles that sort, and cells that change their issue in place.
//!
//! The page's address holds the search, so Back returns to it: `jql`, each filter as `f.<name>` (a JSON list),
//! `sort` and `dir`, and `token` for the page of results.
use crate::adf;
use crate::client::{Jira, encode};
use crate::fields::{self, Fields};
use crate::images;
use crate::pages::{open_issue, param};
use crate::views::{SPRINT, STORY_POINTS, field_label, layer_name, portable, resolve};
use serde::{Deserialize, Serialize};
use taskproxy_plugin_sdk::ui::{
    self, Action, Choice, Column, Field, FieldKind, Node, ReorderItem, Sort, TableRow,
};
use taskproxy_plugin_sdk::{
    ActionRequest, ActionResult, Error, Host, Map, Result, Value, host_supports, json,
};

const PAGE_SIZE: u64 = 50;
/// The most searches one page of My Issues makes to fill itself, when finished subtasks are left out.
const MAX_FILL_REQUESTS: usize = 5;
/// The issue, its parent, and its grandparent.
pub const LAYERS: usize = 3;
const FILTER_OPTIONS_KEY: &str = "filterOptions";
const FILTER_OPTIONS_MS: u64 = 60 * 60 * 1000;
const USERS_KEY: &str = "users";
const USERS_MS: u64 = 24 * 60 * 60 * 1000;
/// Your open issues, and your finished subtasks from the last 90 days, of which only those whose parent is not finished
/// yet are shown: a subtask marked Done after code review still needs following until its story is tested.
pub const MY_JQL: &str = "assignee = currentUser() AND (resolution = Unresolved OR (issuetype in subTaskIssueTypes() AND updated >= -90d)) ORDER BY updated DESC";
pub const ALL_JQL: &str = "resolution = Unresolved ORDER BY updated DESC";
/// The usual planning poker values, offered for story points.
const POINTS: [&str; 11] = ["0", "0.5", "1", "2", "3", "5", "8", "13", "20", "40", "100"];
/// Fields JQL cannot sort by.
const UNSORTABLE: [&str; 6] = [
    "description",
    "comment",
    "environment",
    "attachment",
    "parent",
    "labels",
];
/// Panel keys, shared by the buttons that toggle them.
const FILTERS_PANEL: &str = "list-filters";
const JQL_PANEL: &str = "list-jql";
const COLUMNS_PANEL: &str = "list-columns";
const SAVE_PANEL: &str = "list-save";

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum List {
    Mine,
    All,
}

impl List {
    pub fn of(page: &str) -> List {
        if page == "all" { List::All } else { List::Mine }
    }

    fn page(self) -> &'static str {
        match self {
            List::Mine => "issues",
            List::All => "all",
        }
    }

    fn default_jql(self) -> &'static str {
        match self {
            List::Mine => MY_JQL,
            List::All => ALL_JQL,
        }
    }

    fn columns_key(self) -> &'static str {
        match self {
            List::Mine => "columns.issues",
            List::All => "columns.all",
        }
    }

    /// My Issues keeps the key saved searches always had.
    fn saved_key(self) -> &'static str {
        match self {
            List::Mine => "filters",
            List::All => "filters.all",
        }
    }

    fn filters(self) -> &'static [(&'static str, &'static str)] {
        const MINE: [(&str, &str); 5] = [
            ("project", "Project"),
            ("type", "Type"),
            ("status", "Status"),
            ("priority", "Priority"),
            ("sprint", "Sprint"),
        ];
        const ALL: [(&str, &str); 6] = [
            ("assignee", "Assignee"),
            ("project", "Project"),
            ("type", "Type"),
            ("status", "Status"),
            ("priority", "Priority"),
            ("sprint", "Sprint"),
        ];
        match self {
            List::Mine => &MINE,
            List::All => &ALL,
        }
    }
}

/// A saved search. Older ones saved only their JQL.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct Saved {
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    jql: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    params: Map<String, Value>,
}

fn saved(host: &dyn Host, list: List) -> Result<Vec<Saved>> {
    Ok(host
        .store_get(list.saved_key())?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

/// The issue's own key, which Jira lists as a field named Key.
pub const KEY: &str = "issuekey";

/// One column of a list: a field of the issue (layer 0), its parent (1), or its grandparent (2). Fields are saved
/// as Views save them, so story points and sprint work on any site.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ListColumn {
    pub layer: usize,
    pub field: String,
}

impl ListColumn {
    fn new(layer: usize, field: &str) -> Self {
        ListColumn {
            layer,
            field: field.to_string(),
        }
    }

    /// Unique in a list, as the column's id in the editor.
    fn id(&self) -> String {
        format!("{}:{}", self.layer, self.field)
    }

    /// The column's title, such as "Story points" or "Parent key".
    fn label(&self, fields: &Fields) -> String {
        let name = if self.field == KEY {
            "Key".to_string()
        } else {
            field_label(&self.field, fields)
        };
        if self.layer == 0 {
            name
        } else {
            format!("{} {}", layer_name(self.layer), name.to_lowercase())
        }
    }
}

fn default_columns() -> Vec<ListColumn> {
    [
        KEY,
        "summary",
        "status",
        "priority",
        SPRINT,
        STORY_POINTS,
        "updated",
    ]
    .iter()
    .map(|field| ListColumn::new(0, field))
    .collect()
}

/// The list's columns, in order. Lists once saved their columns as one list per layer, after a Key column that
/// was always first, so those become the issue's key followed by each layer's columns.
fn columns(host: &dyn Host, list: List) -> Result<Vec<ListColumn>> {
    let Some(saved) = host.store_get(list.columns_key())? else {
        return Ok(default_columns());
    };
    let read: Vec<ListColumn> = match &saved {
        Value::Array(items) if !items.is_empty() && items.iter().all(Value::is_array) => {
            let mut out = vec![ListColumn::new(0, KEY)];
            for (layer, fields) in items.iter().take(LAYERS).enumerate() {
                for field in fields.as_array().into_iter().flatten() {
                    if let Some(field) = field.as_str() {
                        out.push(ListColumn::new(layer, field));
                    }
                }
            }
            out
        }
        _ => match serde_json::from_value(saved) {
            Ok(read) => read,
            Err(_) => return Ok(default_columns()),
        },
    };
    let mut out: Vec<ListColumn> = Vec::new();
    for column in read {
        if column.layer < LAYERS && !out.contains(&column) {
            out.push(column);
        }
    }
    Ok(out)
}

fn save_columns(host: &dyn Host, list: List, columns: &[ListColumn]) -> Result<()> {
    host.store_set(list.columns_key(), Some(serde_json::to_value(columns)?))
}

/// A filter's chosen values, from the page's address.
fn chosen(params: &Map<String, Value>, name: &str) -> Vec<String> {
    match params.get(&format!("f.{}", name)) {
        Some(Value::Array(list)) => list
            .iter()
            .filter_map(|value| value.as_str().map(String::from))
            .collect(),
        Some(Value::String(text)) => serde_json::from_str(text).unwrap_or_default(),
        _ => Vec::new(),
    }
}

fn quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

/// JQL's conditions and its ORDER BY, split apart.
fn split_order(jql: &str) -> (String, Option<String>) {
    let lower = jql.to_ascii_lowercase();
    let mut search = 0;
    while let Some(found) = lower[search..].find("order by") {
        let at = search + found;
        let before_ok = at == 0 || lower.as_bytes()[at - 1].is_ascii_whitespace();
        if before_ok {
            return (
                jql[..at].trim().to_string(),
                Some(jql[at + 8..].trim().to_string()),
            );
        }
        search = at + 8;
    }
    (jql.trim().to_string(), None)
}

/// One filter as JQL, or `None` when nothing is chosen.
fn filter_clause(name: &str, values: &[String]) -> Option<String> {
    if values.is_empty() {
        return None;
    }
    let names = || {
        values
            .iter()
            .map(|value| quote(value))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let any = |parts: Vec<String>| {
        if parts.len() == 1 {
            parts.into_iter().next().unwrap()
        } else {
            format!("({})", parts.join(" OR "))
        }
    };
    Some(match name {
        "project" => format!("project in ({})", names()),
        "type" => format!("issuetype in ({})", names()),
        "status" => format!("status in ({})", names()),
        "priority" => format!("priority in ({})", names()),
        "sprint" => any(values
            .iter()
            .map(|value| match value.as_str() {
                "open" => "sprint in openSprints()".to_string(),
                "future" => "sprint in futureSprints()".to_string(),
                "none" => "sprint is EMPTY".to_string(),
                id => format!("sprint = {}", id.parse::<u64>().unwrap_or(0)),
            })
            .collect()),
        "assignee" => any(values
            .iter()
            .map(|value| match value.as_str() {
                "me" => "assignee = currentUser()".to_string(),
                "none" => "assignee is EMPTY".to_string(),
                id => format!("assignee = {}", quote(id)),
            })
            .collect()),
        _ => return None,
    })
}

/// The JQL name to sort a field by, or `None` when JQL cannot sort by it.
fn sort_name(real: &str, fields: &Fields) -> Option<String> {
    if UNSORTABLE.contains(&real) {
        return None;
    }
    if real == KEY {
        return Some("key".into());
    }
    if let Some(number) = real.strip_prefix("customfield_") {
        // Text fields cannot be sorted.
        if fields
            .find(real)
            .is_some_and(|field| field.kind == "string")
        {
            return None;
        }
        return Some(format!("cf[{}]", number));
    }
    Some(real.to_string())
}

/// The search the page's address asks for: its JQL, each filter, and the sort.
fn search_jql(list: List, page_params: &Map<String, Value>, fields: &Fields) -> String {
    let jql = match param(page_params, "jql").trim() {
        "" => list.default_jql(),
        jql => jql,
    };
    let (conditions, order) = split_order(jql);
    let mut parts = Vec::new();
    if !conditions.is_empty() {
        parts.push(format!("({})", conditions));
    }
    for (name, _) in list.filters() {
        if let Some(clause) = filter_clause(name, &chosen(page_params, name)) {
            parts.push(clause);
        }
    }
    let sorted = sort_of(page_params)
        .and_then(|(field, dir)| Some((sort_name(&resolve(&field, fields)?, fields)?, dir)));
    let order = match sorted {
        Some((name, dir)) => Some(format!(
            "{} {}",
            name,
            if dir == Sort::Desc { "DESC" } else { "ASC" }
        )),
        None => order,
    };
    let mut query = parts.join(" AND ");
    if let Some(order) = order.filter(|order| !order.is_empty()) {
        query = format!("{} ORDER BY {}", query, order).trim().to_string();
    }
    query
}

fn sort_of(page_params: &Map<String, Value>) -> Option<(String, Sort)> {
    let field = param(page_params, "sort");
    if field.is_empty() {
        return None;
    }
    let dir = if param(page_params, "dir") == "desc" {
        Sort::Desc
    } else {
        Sort::Asc
    };
    Some((field.to_string(), dir))
}

fn issues_of(found: &Value) -> Vec<Value> {
    found
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// Whether an issue is finished: resolved, or in a status Jira counts as done.
pub(crate) fn is_finished(issue: &Value) -> bool {
    issue
        .pointer("/fields/resolution")
        .is_some_and(|resolution| !resolution.is_null())
        || issue
            .pointer("/fields/status/statusCategory/key")
            .and_then(Value::as_str)
            == Some("done")
}

fn key_of(issue: &Value) -> &str {
    issue.get("key").and_then(Value::as_str).unwrap_or("")
}

fn parent_key(issue: &Value) -> Option<&str> {
    issue.pointer("/fields/parent/key").and_then(Value::as_str)
}

/// Issues by key, with these fields and their parent.
fn fetch(jira: &Jira, keys: &[String], wanted: &[String]) -> Result<Vec<Value>> {
    if keys.is_empty() {
        return Ok(Vec::new());
    }
    let mut wanted: Vec<String> = wanted.to_vec();
    wanted.push("parent".into());
    let list = keys
        .iter()
        .map(|key| quote(key))
        .collect::<Vec<_>>()
        .join(", ");
    let found = jira.post(
        "search/jql",
        &json!({ "jql": format!("key in ({})", list), "maxResults": 100, "fields": wanted }),
    )?;
    Ok(issues_of(&found))
}

fn unique(keys: impl Iterator<Item = String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for key in keys {
        if !out.contains(&key) {
            out.push(key);
        }
    }
    out
}

/// A value on one line, for a table cell.
fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Fields whose value is picked from a list, which change in place.
pub(crate) fn is_choice(real: &str, fields: &Fields) -> bool {
    matches!(real, "status" | "priority" | "assignee")
        || fields.sprint().as_deref() == Some(real)
        || fields.is_story_points(real)
        || fields
            .find(real)
            .is_some_and(|field| field.kind == "option")
}

/// Fields of text the user can edit in a dialog.
pub(crate) fn is_editable_text(real: &str, fields: &Fields) -> bool {
    matches!(real, "summary" | "description" | "environment")
        || (real.starts_with("customfield_")
            && fields
                .find(real)
                .is_some_and(|field| field.kind == "string"))
}

/// Rich text fields hold Jira documents rather than plain strings.
pub(crate) fn is_rich_text(real: &str, fields: &Fields) -> bool {
    matches!(real, "description" | "environment")
        || fields
            .find(real)
            .is_some_and(|field| field.custom_type.ends_with(":textarea"))
}

/// The sprint a value names: the last one, which is the current or next one.
fn sprint_text(value: &Value) -> String {
    value
        .as_array()
        .and_then(|list| list.last())
        .map(fields::value_text)
        .unwrap_or_default()
}

fn latest_comment(value: &Value) -> Option<String> {
    let last = value.get("comments")?.as_array()?.last()?;
    let author = last
        .pointer("/author/displayName")
        .and_then(Value::as_str)
        .unwrap_or("Someone");
    let body = one_line(&adf::to_markdown(last.get("body").unwrap_or(&Value::Null)));
    Some(format!("{}: {}", author, body))
}

pub(crate) fn cell(fields: &Fields, real: &str, label: &str, issue: Option<&Value>) -> Node {
    let Some(issue) = issue else {
        return ui::muted("");
    };
    let key = key_of(issue);
    if matches!(real, "issuekey" | "key") {
        return ui::link(key.to_string(), open_issue(key));
    }
    let value = &fields.value_of(issue, real);
    if real == "comment" {
        let latest = latest_comment(value);
        return Node::Clip {
            muted: latest.is_none(),
            text: latest.unwrap_or_else(|| "No comments".into()),
            max: 30,
            tip: 100,
            action: Some(
                Action::new("cell.comments", format!("Comments on {}", key)).param("key", key),
            ),
            always: true,
            theme: String::new(),
        };
    }
    if is_choice(real, fields) {
        let text = if fields.sprint().as_deref() == Some(real) {
            sprint_text(value)
        } else {
            fields::value_text(value)
        };
        return Node::Select {
            text: if text.is_empty() { "None".into() } else { text },
            theme: if real == "status" {
                fields::status_theme(value).to_string()
            } else {
                String::new()
            },
            options: Action::new("cell.options", format!("Change {}", label))
                .param("key", key)
                .param("field", real),
            action: Action::new("cell.set", format!("Change {}", label))
                .param("key", key)
                .param("field", real),
        };
    }
    let text = one_line(&fields::value_text(value));
    Node::Clip {
        muted: matches!(real, "updated" | "created"),
        theme: if real == "issuetype" {
            fields::issue_type_theme(&text).to_string()
        } else {
            String::new()
        },
        text,
        max: 30,
        tip: 100,
        action: Some(
            Action::new("cell.open", format!("Show {}", label))
                .param("key", key)
                .param("field", real),
        ),
        always: false,
    }
}

fn cached(host: &dyn Host, key: &str, site: &str, max_age: u64) -> Result<Option<Value>> {
    let now = host.now_ms()?;
    Ok(host.store_get(key)?.filter(|cached| {
        cached.get("site").and_then(Value::as_str) == Some(site)
            && cached
                .get("at")
                .and_then(Value::as_u64)
                .is_some_and(|at| now.saturating_sub(at) < max_age)
    }))
}

fn names_from(list: &Value, path: &str) -> Vec<String> {
    let mut names: Vec<String> = list
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|item| item.pointer(path).and_then(Value::as_str).map(String::from))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names.dedup();
    names
}

/// Active and future sprints, from the project's boards or, with no project, from up to 20 scrum boards.
fn sprints(jira: &Jira, project: Option<&str>) -> Vec<Choice> {
    let path = match project {
        Some(project) => format!("board?projectKeyOrId={}&maxResults=20", encode(project)),
        None => "board?type=scrum&maxResults=20".to_string(),
    };
    let Ok(boards) = jira.agile_get(&path) else {
        return Vec::new();
    };
    let mut found: Vec<(u64, String, bool)> = Vec::new();
    for board in boards
        .get("values")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let Some(id) = board.get("id").and_then(Value::as_u64) else {
            continue;
        };
        // Kanban boards have no sprints, and answer with an error.
        let Ok(list) = jira.agile_get(&format!(
            "board/{}/sprint?state=active,future&maxResults=50",
            id
        )) else {
            continue;
        };
        for sprint in list
            .get("values")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let (Some(id), Some(name)) = (
                sprint.get("id").and_then(Value::as_u64),
                sprint.get("name").and_then(Value::as_str),
            ) else {
                continue;
            };
            if !found.iter().any(|(seen, ..)| *seen == id) {
                let active = sprint.get("state").and_then(Value::as_str) == Some("active");
                found.push((id, name.to_string(), active));
            }
        }
    }
    found.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.1.cmp(&b.1)));
    found
        .into_iter()
        .map(|(id, name, active)| {
            Choice::new(
                id.to_string(),
                if active {
                    format!("{} (active)", name)
                } else {
                    name
                },
            )
        })
        .collect()
}

/// The choices for each filter, kept for an hour since they rarely change.
fn filter_options(jira: &Jira) -> Result<Map<String, Value>> {
    if let Some(cached) = cached(jira.host, FILTER_OPTIONS_KEY, &jira.site, FILTER_OPTIONS_MS)?
        && let Some(options) = cached.get("options").and_then(Value::as_object)
    {
        return Ok(options.clone());
    }
    let plain = |names: Vec<String>| -> Vec<Choice> {
        names
            .into_iter()
            .map(|name| Choice::new(name.clone(), name))
            .collect()
    };
    let projects: Vec<Choice> = jira
        .get("project/search?maxResults=100&orderBy=name")
        .ok()
        .and_then(|found| found.get("values").cloned())
        .and_then(|list| list.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|project| {
            let key = project.get("key")?.as_str()?;
            let name = project.get("name")?.as_str()?;
            Some(Choice::new(key, format!("{} ({})", name, key)))
        })
        .collect();
    let types = plain(names_from(
        &jira.get("issuetype").unwrap_or(Value::Null),
        "/name",
    ));
    let statuses = plain(names_from(
        &jira.get("status").unwrap_or(Value::Null),
        "/name",
    ));
    // Priorities keep Jira's order, highest first.
    let priorities: Vec<Choice> = jira
        .get("priority")
        .ok()
        .and_then(|list| list.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|priority| priority.get("name")?.as_str())
        .map(|name| Choice::new(name, name))
        .collect();
    let mut sprint_choices = vec![
        Choice::new("open", "Open sprints"),
        Choice::new("future", "Future sprints"),
        Choice::new("none", "No sprint"),
    ];
    sprint_choices.extend(sprints(jira, None));
    let mut options = Map::new();
    options.insert("project".into(), serde_json::to_value(projects)?);
    options.insert("type".into(), serde_json::to_value(types)?);
    options.insert("status".into(), serde_json::to_value(statuses)?);
    options.insert("priority".into(), serde_json::to_value(priorities)?);
    options.insert("sprint".into(), serde_json::to_value(sprint_choices)?);
    jira.host.store_set(
        FILTER_OPTIONS_KEY,
        Some(json!({ "site": jira.site, "at": jira.host.now_ms()?, "options": options })),
    )?;
    Ok(options)
}

/// The site's people, for the Assignee filter, kept for a day.
fn people(jira: &Jira) -> Result<Vec<Choice>> {
    if let Some(cached) = cached(jira.host, USERS_KEY, &jira.site, USERS_MS)?
        && let Ok(list) = serde_json::from_value::<Vec<Choice>>(cached["list"].clone())
    {
        return Ok(list);
    }
    let mut list: Vec<Choice> = jira
        .get("users/search?maxResults=1000")?
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|user| {
            user.get("accountType").and_then(Value::as_str) == Some("atlassian")
                && user.get("active").and_then(Value::as_bool).unwrap_or(true)
        })
        .filter_map(|user| {
            Some(Choice::new(
                user.get("accountId")?.as_str()?,
                user.get("displayName")?.as_str()?,
            ))
        })
        .collect();
    list.sort_by_key(|choice| choice.label.to_lowercase());
    jira.host.store_set(
        USERS_KEY,
        Some(json!({ "site": jira.site, "at": jira.host.now_ms()?, "list": list })),
    )?;
    Ok(list)
}

fn filter_choices(jira: &Jira, options: &Map<String, Value>, name: &str) -> Result<Vec<Choice>> {
    if name == "assignee" {
        let mut choices = vec![Choice::new("me", "Me"), Choice::new("none", "Unassigned")];
        choices.extend(people(jira)?);
        return Ok(choices);
    }
    Ok(options
        .get(name)
        .and_then(|list| serde_json::from_value(list.clone()).ok())
        .unwrap_or_default())
}

/// Leaves out finished issues, unless they are subtasks whose parent is not finished yet.
fn keep_unfinished_parents(jira: &Jira, issues: &mut Vec<Value>) -> Result<()> {
    let parent_keys = unique(
        issues
            .iter()
            .filter(|issue| is_finished(issue))
            .filter_map(|issue| parent_key(issue).map(String::from)),
    );
    let finished_parents: Vec<String> = fetch(
        jira,
        &parent_keys,
        &["resolution".to_string(), "status".to_string()],
    )?
    .iter()
    .filter(|parent| is_finished(parent))
    .map(|parent| key_of(parent).to_string())
    .collect();
    issues.retain(|issue| {
        !is_finished(issue)
            || parent_key(issue).is_some_and(|parent| {
                parent_keys.iter().any(|key| key == parent)
                    && !finished_parents.iter().any(|key| key == parent)
            })
    });
    Ok(())
}

/// The page: the search form, the column chooser, saved searches, the table, and paging.
pub fn page(
    jira: &Jira,
    fields: &Fields,
    list: List,
    page_params: &Map<String, Value>,
) -> Result<Node> {
    let host = jira.host;
    let chosen_columns = columns(host, list)?;
    let jql = match param(page_params, "jql") {
        "" => list.default_jql(),
        jql => jql,
    };
    let token = param(page_params, "token");
    // Each column with the site's own field id.
    let resolved: Vec<(ListColumn, String)> = chosen_columns
        .iter()
        .filter_map(|column| Some((column.clone(), resolve(&column.field, fields)?)))
        .collect();
    let wanted = |depth: usize| -> Vec<String> {
        fields.wanted(
            &resolved
                .iter()
                .filter(|(column, _)| column.layer == depth)
                .map(|(_, real)| real.clone())
                .collect::<Vec<_>>(),
        )
    };
    let uses = |depth: usize| resolved.iter().any(|(column, _)| column.layer == depth);
    let mut first = wanted(0);
    // The issue type tints bug rows, even when no column shows it.
    for always in ["parent", "resolution", "status", "issuetype"] {
        if !first.iter().any(|id| id == always) {
            first.push(always.into());
        }
    }
    // My Issues' own search also finds finished subtasks, which stay only while their parent is not finished.
    let filtered = list == List::Mine && param(page_params, "jql").trim().is_empty();
    let search = search_jql(list, page_params, fields);
    let mut issues: Vec<Value> = Vec::new();
    let mut token = token.to_string();
    let first_token = token.clone();
    // Issues left out leave the page short, so further pages are read, each for only as many as are still
    // needed, until the page is full or Jira has no more. A few requests at most, since each costs a round trip.
    let mut found;
    let mut requests = 0;
    loop {
        requests += 1;
        let mut body = json!({
            "jql": search,
            "maxResults": PAGE_SIZE - issues.len() as u64,
            "fields": first,
        });
        if !token.is_empty() {
            body["nextPageToken"] = json!(token);
        }
        found = jira.post("search/jql", &body)?;
        let mut page_issues = issues_of(&found);
        if filtered {
            keep_unfinished_parents(jira, &mut page_issues)?;
        }
        issues.extend(page_issues);
        let next = found
            .get("nextPageToken")
            .and_then(Value::as_str)
            .filter(|_| {
                !found
                    .get("isLast")
                    .and_then(Value::as_bool)
                    .unwrap_or(false)
            });
        match next {
            Some(next)
                if filtered
                    && (issues.len() as u64) < PAGE_SIZE
                    && requests < MAX_FILL_REQUESTS =>
            {
                token = next.to_string();
            }
            _ => break,
        }
    }
    let token = first_token;
    let waiting = if filtered {
        issues.iter().filter(|issue| is_finished(issue)).count()
    } else {
        0
    };
    let want_parents = uses(1) || uses(2);
    let parents = if want_parents {
        let keys = unique(
            issues
                .iter()
                .filter_map(|i| parent_key(i).map(String::from)),
        );
        fetch(jira, &keys, &wanted(1))?
    } else {
        Vec::new()
    };
    let grandparents = if uses(2) {
        let keys = unique(
            parents
                .iter()
                .filter_map(|i| parent_key(i).map(String::from)),
        );
        fetch(jira, &keys, &wanted(2))?
    } else {
        Vec::new()
    };
    // Story points show from the field each project's board estimates with, as Jira shows them.
    fields.learn(
        jira,
        issues
            .iter()
            .chain(&parents)
            .chain(&grandparents)
            .map(key_of),
    );
    let by_key = |pool: &[Value], key: Option<&str>| -> Option<Value> {
        let key = key?;
        pool.iter().find(|issue| key_of(issue) == key).cloned()
    };

    let sort = sort_of(page_params);
    let mut table_columns: Vec<Column> = Vec::new();
    for (column, real) in &resolved {
        let label = column.label(fields);
        if column.layer == 0 && sort_name(real, fields).is_some() {
            table_columns.push(Column::Sortable {
                action: Some(
                    Action::new("list.sort", format!("Sort by {}", label))
                        .param("field", column.field.clone()),
                ),
                sort: sort
                    .as_ref()
                    .filter(|(field, _)| *field == column.field)
                    .map(|(_, dir)| *dir),
                label,
            });
        } else {
            table_columns.push(label.into());
        }
    }
    let rows: Vec<TableRow> = issues
        .iter()
        .map(|issue| {
            let parent = by_key(&parents, parent_key(issue));
            let grandparent = parent
                .as_ref()
                .and_then(|parent| by_key(&grandparents, parent_key(parent)));
            let theme = fields::row_theme(issue, parent.as_ref());
            let at_depth = [Some(issue.clone()), parent, grandparent];
            let cells = resolved
                .iter()
                .map(|(column, real)| {
                    cell(
                        fields,
                        real,
                        &column.label(fields),
                        at_depth[column.layer].as_ref(),
                    )
                })
                .collect();
            TableRow {
                cells,
                action: None,
                theme,
            }
        })
        .collect();

    let options = filter_options(jira)?;
    let mut filter_inputs = Vec::new();
    let mut active = 0;
    for (name, label) in list.filters() {
        let values = chosen(page_params, name);
        if !values.is_empty() {
            active += 1;
        }
        filter_inputs.push(
            Field::new(FieldKind::Multiselect, &format!("f.{}", name), *label)
                .options(filter_choices(jira, &options, name)?)
                .value(json!(values)),
        );
    }
    let split = crate::pages::split_view(host)?;
    // Every button sits in one row above the table, and what they open shows below it, so the issues start high on
    // the page.
    let mut children = vec![Node::Form {
        fields: Vec::new(),
        buttons: vec![
            ui::toggle(
                if active > 0 {
                    format!("Filters ({})", active)
                } else {
                    "Filters".into()
                },
                FILTERS_PANEL,
            ),
            ui::toggle("JQL", JQL_PANEL),
            ui::toggle("Columns", COLUMNS_PANEL),
            ui::toggle("Save search", SAVE_PANEL),
            // Whether an issue opens beside the list, or in its place.
            ui::switch(
                Action::new("list.split", "Split view").icon("table-columns"),
                split,
            ),
        ],
        // Refresh runs the search again as it is, and the button becomes Search once the user changes it. Both
        // search with the form's values, so a host that always shows Refresh still searches for what was typed.
        submit: Action::new("list.search", "Refresh").icon("refresh"),
        cancel: None,
        children: vec![
            ui::panel(
                FILTERS_PANEL,
                false,
                vec![Node::Inputs {
                    fields: filter_inputs,
                }],
            ),
            ui::panel(
                JQL_PANEL,
                false,
                vec![Node::Inputs {
                    fields: vec![
                        Field::new(FieldKind::Textarea, "jql", "JQL")
                            .value(jql)
                            .help("Jira Query Language, as in Jira's advanced search. The filters are added to it."),
                    ],
                }],
            ),
        ],
        inline: false,
        changed: Some(Action::new("list.search", "Search").icon("magnifying-glass")),
    }];
    children.push(ui::panel(
        COLUMNS_PANEL,
        false,
        columns_editor(host, fields, &chosen_columns),
    ));
    children.push(ui::panel(
        SAVE_PANEL,
        false,
        vec![Node::Form {
            fields: vec![
                Field::new(FieldKind::Text, "name", "Save this search as")
                    .placeholder("Sprint bugs"),
            ],
            submit: Action::new("list.save", "Save search"),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: true,
            changed: None,
        }],
    ));

    // The saved searches, and how many finished subtasks were kept, share one line.
    let mut saved_links = vec![ui::link(
        if list == List::Mine {
            "My open issues"
        } else {
            "All open issues"
        },
        Action::new("list.reset", "Clear the search"),
    )];
    for (index, search) in saved(host, list)?.iter().enumerate() {
        saved_links.push(ui::link(
            search.name.clone(),
            Action::new("list.apply", search.name.clone()).param("index", index),
        ));
        saved_links.push(ui::link(
            "✕",
            Action::new("list.delete", format!("Delete {}", search.name))
                .param("index", index)
                .confirm(format!("Delete the saved search '{}'?", search.name)),
        ));
    }
    if waiting > 0 {
        saved_links.push(ui::muted(format!(
            "{} finished {} {} here until {} parent is finished.",
            waiting,
            if waiting == 1 { "subtask" } else { "subtasks" },
            if waiting == 1 { "stays" } else { "stay" },
            if waiting == 1 { "its" } else { "their" },
        )));
    }
    children.push(ui::row(saved_links));
    if rows.is_empty() {
        children.push(ui::empty("No issues match this search."));
    } else {
        children.push(Node::Table {
            columns: table_columns,
            rows,
        });
    }
    let mut paging = Vec::new();
    if !token.is_empty() {
        paging.push(ui::button(Action::new("list.page", "First page"), ""));
    }
    if let Some(next) = found.get("nextPageToken").and_then(Value::as_str)
        && !found
            .get("isLast")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        paging.push(ui::button(
            Action::new("list.page", format!("Next {}", PAGE_SIZE)).param("token", next),
            "",
        ));
    }
    if !paging.is_empty() {
        children.push(ui::toolbar(paging));
    }
    Ok(ui::stack(children))
}

fn columns_editor(host: &dyn Host, fields: &Fields, chosen: &[ListColumn]) -> Vec<Node> {
    let mut choices: Vec<Choice> = fields
        .choices()
        .into_iter()
        .map(|field| {
            let id = portable(&field.id, fields);
            let label = if id == KEY {
                "Key".to_string()
            } else {
                field_label(&id, fields)
            };
            Choice::new(id, label)
        })
        .collect();
    if !choices.iter().any(|choice| choice.value == KEY) {
        choices.insert(0, Choice::new(KEY, "Key"));
    }
    let drag = host_supports(host, "0.1.2");
    let mut nodes = vec![ui::muted(if drag {
        "Choose columns from each issue, its parent, and its grandparent, and drag them into order. They are saved for you."
    } else {
        "Choose columns from each issue, its parent, and its grandparent, and put them in order. They are saved for you."
    })];
    // One row adds a column from any layer: Add a column | Issue | Parent | Grandparent | Add.
    let pickers: Vec<Field> = (0..LAYERS)
        .map(|layer| {
            let mut options = vec![Choice::new("", "Choose…")];
            options.extend(
                choices
                    .iter()
                    .filter(|choice| !chosen.contains(&ListColumn::new(layer, &choice.value)))
                    .cloned(),
            );
            Field::new(
                FieldKind::Select,
                &format!("layer{}", layer),
                layer_name(layer),
            )
            .options(options)
            .value("")
        })
        .collect();
    nodes.push(ui::row(vec![
        ui::strong("Add a column"),
        Node::Form {
            fields: pickers,
            submit: Action::new("columns.add", "Add").icon("plus"),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: true,
            changed: None,
        },
    ]));
    if chosen.is_empty() {
        nodes.push(ui::muted("No columns."));
    } else if drag {
        nodes.push(Node::Reorder {
            items: chosen
                .iter()
                .enumerate()
                .map(|(index, column)| ReorderItem {
                    id: column.id(),
                    children: vec![
                        ui::text(column.label(fields)),
                        ui::link(
                            "✕",
                            Action::new(
                                "columns.remove",
                                format!("Remove {}", column.label(fields)),
                            )
                            .param("index", index),
                        ),
                    ],
                })
                .collect(),
            action: Action::new("columns.order", "Reorder the columns"),
        });
    } else {
        // Hosts before spec 0.1.2 cannot drag, so each column moves with arrows.
        let items = chosen
            .iter()
            .enumerate()
            .map(|(index, column)| {
                let at =
                    |action: &str, label: &str| Action::new(action, label).param("index", index);
                let mut line = vec![ui::text(column.label(fields))];
                if index > 0 {
                    line.push(ui::link("↑", at("columns.move", "Move up").param("by", -1)));
                }
                if index + 1 < chosen.len() {
                    line.push(ui::link(
                        "↓",
                        at("columns.move", "Move down").param("by", 1),
                    ));
                }
                line.push(ui::link("✕", at("columns.remove", "Remove")));
                ui::row(line)
            })
            .collect();
        nodes.push(ui::stack(items));
    }
    nodes.push(ui::toolbar(vec![ui::button(
        Action::new("columns.reset", "Reset columns")
            .confirm("Go back to the columns Task Proxy starts with?"),
        "",
    )]));
    nodes
}

fn form_text<'a>(request: &'a ActionRequest, name: &str) -> &'a str {
    request
        .form
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
}

fn param_i64(params: &Map<String, Value>, name: &str) -> i64 {
    params
        .get(name)
        .and_then(|value| value.as_i64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0)
}

/// The page's address without the page of results, for a new search or sort.
fn search_params(page_params: &Map<String, Value>) -> Map<String, Value> {
    let mut params = page_params.clone();
    params.remove("token");
    params
}

/// Who a comment can mention, from the people who can see the issue.
fn suggest_people(jira: &Jira, key: &str, query: &str) -> Result<Vec<Choice>> {
    let query = query.trim();
    let path = if key.is_empty() {
        format!("user/search?query={}&maxResults=10", encode(query))
    } else {
        format!(
            "user/viewissue/search?issueKey={}&query={}&maxResults=10",
            encode(key),
            encode(query)
        )
    };
    Ok(jira
        .get(&path)?
        .as_array()
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter(|user| user.get("accountType").and_then(Value::as_str) != Some("app"))
        .filter_map(|user| {
            Some(Choice::new(
                user.get("accountId")?.as_str()?,
                user.get("displayName")?.as_str()?,
            ))
        })
        .collect())
}

/// A comment box that suggests people to mention.
pub fn comment_field(key: &str, label: &str) -> Field {
    Field::new(FieldKind::Textarea, "body", label)
        .placeholder(
            "Markdown: **bold**, *italic*, `code`, lists, and links. Type @ to mention someone.",
        )
        .suggest(Action::new("users.suggest", "Mention someone").param("key", key))
}

/// The people a form's text mentions, as `(text written, account id)`.
pub fn mentions(request: &ActionRequest, name: &str) -> Vec<(String, String)> {
    request
        .form
        .get(&format!("{}.mentions", name))
        .and_then(Value::as_object)
        .map(|map| {
            map.iter()
                .filter_map(|(text, id)| Some((text.clone(), id.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default()
}

fn comments_dialog(jira: &Jira, key: &str) -> Result<Node> {
    let found = jira.get(&format!(
        "issue/{}/comment?orderBy=created&maxResults=100",
        encode(key)
    ))?;
    let list = json!({ "comments": found.get("comments").cloned().unwrap_or(json!([])) });
    Ok(ui::stack(vec![
        fields::value_node("comment", &list),
        ui::form(
            vec![comment_field(key, "Add a comment")],
            Action::new("cell.comment", "Comment").param("key", key),
        ),
    ]))
}

fn editmeta_choices(jira: &Jira, key: &str, real: &str) -> Result<Vec<Choice>> {
    let meta = jira.get(&format!("issue/{}/editmeta", encode(key)))?;
    let Some(field) = meta.pointer(&format!("/fields/{}", real)) else {
        return Err(Error(format!(
            "Jira does not let you change this field on {}.",
            key
        )));
    };
    let mut choices: Vec<Choice> = field
        .get("allowedValues")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
        .iter()
        .filter_map(|allowed| {
            let id = allowed.get("id")?.as_str()?;
            let label = allowed
                .get("name")
                .or_else(|| allowed.get("value"))?
                .as_str()?;
            Some(Choice::new(id, label))
        })
        .collect();
    if real != "priority"
        && !field
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
    {
        choices.insert(0, Choice::new("", "None"));
    }
    Ok(choices)
}

fn choices_for(jira: &Jira, fields: &Fields, key: &str, real: &str) -> Result<Vec<Choice>> {
    if real == "status" {
        let transitions = jira.get(&format!("issue/{}/transitions", encode(key)))?;
        return Ok(transitions
            .get("transitions")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
            .iter()
            .filter_map(|transition| {
                let id = transition.get("id")?.as_str()?;
                let to = transition
                    .pointer("/to/name")
                    .or_else(|| transition.get("name"))?
                    .as_str()?;
                Some(Choice::new(id, to))
            })
            .collect());
    }
    if real == "assignee" {
        let mut choices = vec![Choice::new("", "Unassigned")];
        let users = jira.get(&format!(
            "user/assignable/search?issueKey={}&maxResults=100",
            encode(key)
        ))?;
        choices.extend(
            users
                .as_array()
                .map(Vec::as_slice)
                .unwrap_or(&[])
                .iter()
                .filter_map(|user| {
                    Some(Choice::new(
                        user.get("accountId")?.as_str()?,
                        user.get("displayName")?.as_str()?,
                    ))
                }),
        );
        return Ok(choices);
    }
    if fields.sprint().as_deref() == Some(real) {
        let project = key.split('-').next().unwrap_or("");
        let mut choices = vec![Choice::new("", "Backlog (no sprint)")];
        choices.extend(sprints(jira, Some(project)));
        return Ok(choices);
    }
    if fields.is_story_points(real) {
        let mut choices = vec![Choice::new("", "None")];
        choices.extend(POINTS.iter().map(|points| Choice::new(*points, *points)));
        return Ok(choices);
    }
    editmeta_choices(jira, key, real)
}

fn set_choice(jira: &Jira, fields: &Fields, key: &str, real: &str, value: &str) -> Result<()> {
    let issue = format!("issue/{}", encode(key));
    if real == "status" {
        jira.post(
            &format!("{}/transitions", issue),
            &json!({ "transition": { "id": value } }),
        )?;
    } else if real == "assignee" {
        let id = if value.is_empty() {
            Value::Null
        } else {
            json!(value)
        };
        jira.put(&format!("{}/assignee", issue), &json!({ "accountId": id }))?;
    } else if fields.sprint().as_deref() == Some(real) {
        if value.is_empty() {
            jira.agile_post("backlog/issue", &json!({ "issues": [key] }))?;
        } else {
            jira.agile_post(
                &format!("sprint/{}/issue", encode(value)),
                &json!({ "issues": [key] }),
            )?;
        }
    } else if fields.is_story_points(real) {
        set_story_points(jira, fields, key, real, value)?;
    } else {
        let choice = if value.is_empty() {
            Value::Null
        } else {
            json!({ "id": value })
        };
        jira.put(&issue, &json!({ "fields": { real: choice } }))?;
    }
    Ok(())
}

/// Sets an issue's story points, then reads them back, since Jira accepts a change to a story points field it does
/// not show and nothing seems to happen.
fn set_story_points(
    jira: &Jira,
    fields: &Fields,
    key: &str,
    real: &str,
    value: &str,
) -> Result<()> {
    let issue = format!("issue/{}", encode(key));
    let points = if value.is_empty() {
        None
    } else {
        Some(
            value
                .parse::<f64>()
                .map_err(|_| Error::new("Story points must be a number."))?,
        )
    };
    fields.learn(jira, [key]);
    let shown = fields.estimation_of(key);
    // The board's estimation sets the field Jira shows, even when the issue's edit screen does not have it.
    if let Some(board) = fields.estimation_board(key) {
        jira.agile_put(
            &format!("issue/{}/estimation?boardId={}", encode(key), board),
            &json!({ "value": points.map(|points| points.to_string()) }),
        )?;
    }
    // The other story points fields on its edit screen are set too, so none keeps an older value.
    let targets: Vec<String> = story_points_targets(jira, fields, key, real)
        .into_iter()
        .filter(|id| shown.as_deref() != Some(id.as_str()))
        .collect();
    if !targets.is_empty() {
        let values: serde_json::Map<String, Value> = targets
            .into_iter()
            .map(|id| (id, points.map_or(Value::Null, |points| json!(points))))
            .collect();
        let result = jira.put(&issue, &json!({ "fields": values }));
        if shown.is_none() {
            result?;
        }
    }
    let wanted = fields.wanted(&[real.to_string()]);
    let after = jira.get(&format!("{}?fields={}", issue, wanted.join(",")))?;
    let now = fields.value_of(&after, real).as_f64();
    if now != points {
        let name = shown
            .as_deref()
            .map(|id| fields.name(id))
            .unwrap_or_else(|| fields.name(real));
        return Err(Error(format!(
            "Jira did not change {} on {}. It may not be on the issue's edit screen, or you may not be allowed to estimate it.",
            name, key
        )));
    }
    Ok(())
}

/// Which story points fields to set on an issue: every one its edit screen has. A site with several often has more
/// than one on a screen, and Jira shows only the one its board estimates with, so setting just one could change a
/// field Jira does not show, which the list would still show as set.
fn story_points_targets(jira: &Jira, fields: &Fields, key: &str, real: &str) -> Vec<String> {
    let ids = fields.story_points_ids();
    if ids.len() < 2 {
        return vec![real.to_string()];
    }
    let on_screen: Vec<String> = jira
        .get(&format!("issue/{}/editmeta", encode(key)))
        .map(|meta| {
            ids.iter()
                .filter(|id| meta.pointer(&format!("/fields/{}", id)).is_some())
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    if on_screen.is_empty() {
        vec![real.to_string()]
    } else {
        on_screen
    }
}

/// The panel a text field's editor is in, shown by its Edit button.
const EDIT_PANEL: &str = "cell-edit";

/// The editor for a text field: a text box for a summary, or markdown for anything else, saved with `cell.save`.
pub fn text_editor(fields: &Fields, key: &str, real: &str, value: &Value) -> Node {
    let text = if value.get("type").and_then(Value::as_str) == Some("doc") {
        adf::to_markdown_with_media(value).0
    } else {
        value.as_str().unwrap_or("").to_string()
    };
    let field = if real == "summary" {
        Field::new(FieldKind::Text, "value", "Summary").required()
    } else {
        Field::new(FieldKind::Textarea, "value", fields.name(real)).placeholder(
            "Markdown: **bold**, *italic*, `code`, lists, and links. Keep a ![name](media-0) line to keep its image.",
        )
    };
    Node::Form {
        fields: vec![field.value(text)],
        submit: Action::new("cell.save", "Save")
            .param("key", key)
            .param("field", real),
        cancel: None,
        buttons: Vec::new(),
        children: Vec::new(),
        inline: real == "summary",
        changed: None,
    }
}

fn text_dialog(jira: &Jira, fields: &Fields, key: &str, real: &str) -> Result<Node> {
    let issue = jira.get(&format!("issue/{}?fields={}", encode(key), encode(real)))?;
    let value = issue
        .pointer(&format!("/fields/{}", real))
        .cloned()
        .unwrap_or(Value::Null);
    if !is_editable_text(real, fields) {
        return Ok(fields::value_node(real, &value));
    }
    if !is_rich_text(real, fields) {
        return Ok(text_editor(fields, key, real, &value));
    }
    // Rich text shows rendered, with its images and a switch to the markdown as written, and Edit opens its editor.
    Ok(ui::stack(vec![
        images::rich_text(jira, key, real, &value),
        ui::toolbar(vec![
            ui::toggle("Edit", EDIT_PANEL),
            ui::button(Action::new("cell.close", "Close"), "secondary"),
        ]),
        ui::panel(
            EDIT_PANEL,
            false,
            vec![text_editor(fields, key, real, &value)],
        ),
    ]))
}

/// Answers the actions of My Issues and All Issues, or `None` when the action is not one of them.
pub fn action(jira: &Jira, request: &ActionRequest) -> Result<Option<ActionResult>> {
    let host = jira.host;
    let list = List::of(&request.page);
    let p = &request.params;
    let key = param(p, "key");
    let real = param(p, "field");
    let result = match request.action.as_str() {
        "list.split" => {
            let on = !crate::pages::split_view(host)?;
            crate::pages::set_split_view(host, on)?;
            // An issue already open beside the list switches between beside it and covering it.
            if crate::pages::host_covers(host) {
                ActionResult::refresh().cover(!on)
            } else {
                ActionResult::refresh()
            }
        }
        "list.search" => {
            let mut params = search_params(&request.page_params);
            let jql = form_text(request, "jql");
            if jql.is_empty() || jql == list.default_jql() {
                params.remove("jql");
            } else {
                params.insert("jql".into(), json!(jql));
            }
            for (name, _) in list.filters() {
                let key = format!("f.{}", name);
                match request.form.get(&key) {
                    Some(Value::Array(values)) if !values.is_empty() => {
                        params.insert(key, Value::Array(values.clone()));
                    }
                    _ => {
                        params.remove(&key);
                    }
                }
            }
            ActionResult::navigate(list.page(), params)
        }
        "list.sort" => {
            let field = param(p, "field").to_string();
            let mut params = search_params(&request.page_params);
            let dir = match sort_of(&request.page_params) {
                Some((current, Sort::Asc)) if current == field => "desc",
                _ => "asc",
            };
            params.insert("sort".into(), json!(field));
            params.insert("dir".into(), json!(dir));
            ActionResult::navigate(list.page(), params)
        }
        "list.page" => {
            let mut params = search_params(&request.page_params);
            let token = param(p, "token");
            if !token.is_empty() {
                params.insert("token".into(), json!(token));
            }
            ActionResult::navigate(list.page(), params)
        }
        "list.reset" => ActionResult::navigate(list.page(), Map::new()),
        "list.save" => {
            let name = form_text(request, "name");
            if name.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Name the search to save it.", "warning"),
                ));
            }
            let mut all = saved(host, list)?;
            all.retain(|search| search.name != name);
            all.push(Saved {
                name: name.to_string(),
                jql: String::new(),
                params: search_params(&request.page_params),
            });
            host.store_set(list.saved_key(), Some(serde_json::to_value(&all)?))?;
            ActionResult::refresh().with_message(format!("Saved '{}'.", name), "success")
        }
        "list.apply" => {
            let all = saved(host, list)?;
            let search = all
                .get(param_i64(p, "index") as usize)
                .ok_or_else(|| Error::new("That saved search no longer exists."))?;
            let mut params = search.params.clone();
            if params.is_empty() && !search.jql.is_empty() {
                params.insert("jql".into(), json!(search.jql));
            }
            ActionResult::navigate(list.page(), params)
        }
        "list.delete" => {
            let mut all = saved(host, list)?;
            let index = param_i64(p, "index") as usize;
            if index < all.len() {
                all.remove(index);
            }
            host.store_set(list.saved_key(), Some(serde_json::to_value(&all)?))?;
            ActionResult::refresh()
        }
        "columns.add" | "columns.remove" | "columns.move" | "columns.order" | "columns.reset" => {
            let mut chosen = columns(host, list)?;
            let index = param_i64(p, "index") as usize;
            match request.action.as_str() {
                "columns.reset" => chosen = default_columns(),
                "columns.add" => {
                    let picked: Vec<ListColumn> = (0..LAYERS)
                        .filter_map(|layer| {
                            let field = form_text(request, &format!("layer{}", layer));
                            (!field.is_empty()).then(|| ListColumn::new(layer, field))
                        })
                        .collect();
                    if picked.is_empty() {
                        return Ok(Some(
                            ActionResult::default()
                                .with_message("Choose a column to add.", "warning"),
                        ));
                    }
                    for column in picked {
                        if !chosen.contains(&column) {
                            chosen.push(column);
                        }
                    }
                }
                "columns.remove" => {
                    if index < chosen.len() {
                        chosen.remove(index);
                    }
                }
                "columns.order" => {
                    let order: Vec<&str> = request
                        .form
                        .get("order")
                        .and_then(Value::as_array)
                        .map(|ids| ids.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    let mut rest = chosen.clone();
                    let mut ordered = Vec::new();
                    for id in order {
                        if let Some(at) = rest.iter().position(|column| column.id() == id) {
                            ordered.push(rest.remove(at));
                        }
                    }
                    // Columns the order did not name, such as one added in another window, stay at the end.
                    ordered.extend(rest);
                    chosen = ordered;
                }
                _ => {
                    let target = index as i64 + param_i64(p, "by");
                    if index < chosen.len() && (0..chosen.len() as i64).contains(&target) {
                        chosen.swap(index, target as usize);
                    }
                }
            }
            save_columns(host, list, &chosen)?;
            ActionResult::refresh()
        }
        "cell.options" => {
            let fields = Fields::load(jira)?;
            ActionResult::options(choices_for(jira, &fields, key, real)?)
        }
        "cell.set" => {
            let fields = Fields::load(jira)?;
            let value = form_text(request, "value");
            set_choice(jira, &fields, key, real, value)?;
            ActionResult::refresh().with_message(
                format!(
                    "{} changed on {}.",
                    field_label(&portable(real, &fields), &fields),
                    key
                ),
                "success",
            )
        }
        "cell.open" => {
            let fields = Fields::load(jira)?;
            let label = field_label(&portable(real, &fields), &fields);
            ActionResult::dialog(
                format!("{} · {}", key, label),
                text_dialog(jira, &fields, key, real)?,
            )
        }
        "cell.save" => {
            let fields = Fields::load(jira)?;
            let text = request
                .form
                .get("value")
                .and_then(Value::as_str)
                .unwrap_or("");
            if real == "summary" && text.trim().is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("An issue needs a summary.", "warning"),
                ));
            }
            let value = if is_rich_text(real, &fields) {
                if text.trim().is_empty() {
                    Value::Null
                } else {
                    // The images the field had come back where the markdown still names them.
                    let original = jira
                        .get(&format!("issue/{}?fields={}", encode(key), encode(real)))
                        .ok()
                        .and_then(|issue| issue.pointer(&format!("/fields/{}", real)).cloned())
                        .unwrap_or(Value::Null);
                    adf::from_markdown_keeping_media(text, &original)
                }
            } else {
                json!(text.trim())
            };
            jira.put(
                &format!("issue/{}", encode(key)),
                &json!({ "fields": { real: value } }),
            )?;
            ActionResult::default()
                .and_close()
                .and_refresh()
                .with_message(format!("{} saved.", key), "success")
        }
        "cell.close" => ActionResult::default().and_close(),
        "cell.comments" => {
            ActionResult::dialog(format!("{} · Comments", key), comments_dialog(jira, key)?)
        }
        "cell.comment" => {
            let body = form_text(request, "body");
            if body.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Write a comment first.", "warning"),
                ));
            }
            jira.post(
                &format!("issue/{}/comment", encode(key)),
                &json!({ "body": adf::from_markdown_with_mentions(body, &mentions(request, "body")) }),
            )?;
            let mut result = ActionResult::ui(comments_dialog(jira, key)?).and_refresh();
            result.message = Some(taskproxy_plugin_sdk::Message {
                text: "Comment added.".into(),
                theme: "success".into(),
            });
            result
        }
        "users.suggest" => ActionResult::options(suggest_people(jira, key, param(p, "query"))?),
        _ => return Ok(None),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FakeHost, sample_fields};

    fn connected() -> FakeHost {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        for path in ["project/search", "issuetype", "status", "priority"] {
            host.answer("GET", &format!("/rest/api/3/{}", path), 200, json!([]));
        }
        host.answer("GET", "/rest/agile/1.0/board", 200, json!({ "values": [] }));
        host
    }

    fn request(
        page: &str,
        action: &str,
        params: Value,
        form: Value,
        page_params: Value,
    ) -> ActionRequest {
        ActionRequest {
            page: page.into(),
            action: action.into(),
            params: params.as_object().cloned().unwrap_or_default(),
            form: form.as_object().cloned().unwrap_or_default(),
            page_params: page_params.as_object().cloned().unwrap_or_default(),
        }
    }

    fn page_params(value: Value) -> Map<String, Value> {
        value.as_object().cloned().unwrap_or_default()
    }

    #[test]
    fn filters_and_sorting_are_added_to_the_jql() {
        let host = connected();
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        assert_eq!(
            search_jql(List::Mine, &Map::new(), &fields),
            format!(
                "({}) ORDER BY updated DESC",
                MY_JQL.trim_end_matches(" ORDER BY updated DESC")
            )
        );
        let params = page_params(json!({
            "jql": "project = ATL order by created",
            "f.status": "[\"To Do\",\"In \\\"QA\\\"\"]",
            "f.sprint": ["open", "42", "none"],
            "sort": "storyPoints", "dir": "desc",
        }));
        assert_eq!(
            search_jql(List::Mine, &params, &fields),
            "(project = ATL) AND status in (\"To Do\", \"In \\\"QA\\\"\") \
             AND (sprint in openSprints() OR sprint = 42 OR sprint is EMPTY) ORDER BY cf[10016] DESC"
        );
        // My Issues has no assignee filter, so one in the address is ignored there.
        let people = page_params(json!({ "f.assignee": ["me", "none", "abc-1"] }));
        assert!(!search_jql(List::Mine, &people, &fields).contains("assignee is EMPTY"));
        assert_eq!(
            search_jql(List::All, &people, &fields),
            "(resolution = Unresolved) AND (assignee = currentUser() OR assignee is EMPTY OR assignee = \"abc-1\") ORDER BY updated DESC"
        );
        // A text field cannot be sorted, so the JQL's own order stays.
        let text = page_params(json!({ "sort": "customfield_10050" }));
        assert!(search_jql(List::All, &text, &fields).ends_with("ORDER BY updated DESC"));
    }

    #[test]
    fn the_page_shows_chosen_columns_from_parents_with_sortable_titles() {
        let host = connected();
        host.store.borrow_mut().insert(
            "columns.issues".into(),
            json!([
                ["summary", "status", "storyPoints", "comment"],
                ["summary"],
                ["status"]
            ]),
        );
        let long = "A summary that is much too long to fit in a table cell";
        host.answer("POST", "/rest/api/3/search/jql", 200, json!({ "isLast": true, "issues": [
            { "key": "P-3", "fields": {
                "summary": long, "status": { "name": "To Do", "statusCategory": { "key": "new" } },
                "customfield_10016": 3, "parent": { "key": "P-2" },
                "comment": { "comments": [
                    { "author": { "displayName": "Sam" }, "body": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "old" }] }] } },
                    { "author": { "displayName": "Priya" }, "body": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "latest" }] }] } }
                ] }
            } },
            { "key": "P-2", "fields": {
                "summary": "The story", "status": { "name": "Done", "statusCategory": { "key": "done" } }, "parent": { "key": "P-1" }
            } },
            { "key": "P-1", "fields": {
                "summary": "The epic", "status": { "name": "In Progress", "statusCategory": { "key": "indeterminate" } }
            } }
        ] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let node = page(
            &jira,
            &fields,
            List::Mine,
            &page_params(json!({ "sort": "summary", "dir": "asc" })),
        )
        .unwrap();
        let json = serde_json::to_value(&node).unwrap();
        let table = json["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|child| child["type"] == "table")
            .unwrap()
            .clone();
        let titles: Vec<Value> = table["columns"].as_array().unwrap().clone();
        assert_eq!(titles[0]["label"], "Key");
        assert_eq!(titles[1]["label"], "Summary");
        assert_eq!(titles[1]["sort"], "asc");
        assert_eq!(titles[3]["label"], "Story points");
        // Comments cannot be sorted, and nor can a parent's fields.
        assert_eq!(titles[4], "Comment");
        assert_eq!(titles[5], "Parent summary");
        assert_eq!(titles[6], "Grandparent status");
        let row = &table["rows"][0];
        assert!(row.get("action").is_none(), "only the key opens the issue");
        assert_eq!(
            row["cells"][0],
            json!({ "type": "link", "text": "P-3", "action": { "id": "open", "label": "Open P-3", "params": { "key": "P-3" } } })
        );
        assert_eq!(row["cells"][1]["type"], "clip");
        assert_eq!(row["cells"][1]["text"], long);
        assert_eq!(row["cells"][2]["type"], "select");
        assert_eq!(
            row["cells"][2]["options"]["params"],
            json!({ "key": "P-3", "field": "status" })
        );
        assert_eq!(row["cells"][3]["type"], "select");
        assert_eq!(row["cells"][3]["text"], "3");
        assert_eq!(row["cells"][4]["text"], "Priya: latest");
        assert_eq!(row["cells"][4]["always"], true);
        assert_eq!(row["cells"][5]["text"], "The story");
        assert_eq!(row["cells"][6]["text"], "In Progress");
        let searches: Vec<String> = host
            .sent
            .borrow()
            .iter()
            .filter(|sent| sent.url.ends_with("search/jql"))
            .map(|sent| sent.body.clone().unwrap())
            .collect();
        // The search, the parents of finished subtasks (P-2 is done), then the parent and grandparent columns.
        assert_eq!(searches.len(), 4);
        assert!(searches[0].contains("ORDER BY summary ASC"));
        assert!(searches[1].contains("\"fields\":[\"resolution\",\"status\",\"parent\"]"));
        // The fake answers every search alike, so its parents are asked for too.
        assert!(searches[2].contains("key in (\\\"P-2\\\""));
        assert!(searches[3].contains("\\\"P-1\\\""));
    }

    #[test]
    fn issues_open_beside_the_list_until_split_view_is_turned_off() {
        let host = connected();
        host.answer(
            "POST",
            "/rest/api/3/search/jql",
            200,
            json!({ "isLast": true, "issues": [] }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let open = |page: &str| {
            crate::pages::action(
                &jira,
                &request(page, "open", json!({ "key": "P-1" }), json!({}), json!({})),
            )
            .unwrap()
            .unwrap()
            .navigate
            .unwrap()
        };
        let beside = open("issues");
        assert_eq!(beside.page, "issue");
        assert_eq!(beside.params["key"], "P-1");
        assert!(beside.side, "split view is on until turned off");
        assert!(open("all").side);
        // From the issue page itself, a linked issue opens in its place.
        assert!(!open("issue").side);
        // The split view switch, as its label and whether it is on.
        let switch = || {
            let node = page(
                &jira,
                &Fields::load(&jira).unwrap(),
                List::Mine,
                &page_params(json!({})),
            )
            .unwrap();
            fn find(value: &Value) -> Option<(String, Option<bool>)> {
                if value["type"] == "button" && value["action"]["id"] == "list.split" {
                    return Some((
                        value["action"]["label"].as_str()?.to_string(),
                        value["on"].as_bool(),
                    ));
                }
                match value {
                    Value::Array(items) => items.iter().find_map(find),
                    Value::Object(map) => map.values().find_map(find),
                    _ => None,
                }
            }
            find(&serde_json::to_value(&node).unwrap()).unwrap()
        };
        assert_eq!(switch(), ("Split view".to_string(), Some(true)));
        let turned = action(
            &jira,
            &request("issues", "list.split", json!({}), json!({}), json!({})),
        )
        .unwrap()
        .unwrap();
        assert!(turned.refresh);
        assert!(!open("issues").side);
        assert_eq!(switch(), ("Split view".to_string(), Some(false)));
        // Older hosts read no side, and the field is left out when it is off.
        let json = serde_json::to_value(open("issues")).unwrap();
        assert!(json.get("side").is_none());
    }

    #[test]
    fn with_split_view_off_issues_cover_the_list_where_task_proxy_can() {
        let host = connected();
        host.answer(
            "POST",
            "/rest/api/3/search/jql",
            200,
            json!({ "isLast": true, "issues": [] }),
        );
        // Task Proxy reports spec 0.1.3 until 0.1.4 is released, so its version says whether it can cover.
        *host.spec.borrow_mut() = Some("0.1.3".into());
        *host.app.borrow_mut() = Some("0.1.151".into());
        let jira = Jira::connect(&host).unwrap().unwrap();
        let run = |name: &str| {
            action(
                &jira,
                &request(
                    "issues",
                    name,
                    json!({ "key": "P-1" }),
                    json!({}),
                    json!({}),
                ),
            )
            .unwrap()
            .unwrap()
        };
        let open = || {
            crate::pages::action(
                &jira,
                &request(
                    "issues",
                    "open",
                    json!({ "key": "P-1" }),
                    json!({}),
                    json!({}),
                ),
            )
            .unwrap()
            .unwrap()
            .navigate
            .unwrap()
        };
        // On, the issue opens beside the list.
        let beside = open();
        assert!(beside.side && !beside.cover);
        // Turning it off makes an issue already beside the list cover it, and new ones open covering it.
        let off = run("list.split");
        assert!(off.refresh);
        assert_eq!(off.cover, Some(true));
        let covering = open();
        assert!(covering.side && covering.cover);
        assert_eq!(covering.params["key"], "P-1");
        // Turning it back on shows them side by side again.
        assert_eq!(run("list.split").cover, Some(false));
        assert!(!open().cover);
        // Task Proxy 0.1.150 shows a side page but cannot cover, so with split view off the issue opens in place.
        *host.app.borrow_mut() = Some("0.1.150".into());
        assert_eq!(run("list.split").cover, None);
        assert!(!open().side);
        // A Task Proxy that reports spec 0.1.4 can cover, whatever its version.
        *host.spec.borrow_mut() = Some("0.1.4".into());
        assert!(open().cover);
    }

    #[test]
    fn issue_types_are_colored_and_bug_rows_are_tinted() {
        let host = connected();
        host.store
            .borrow_mut()
            .insert("columns.issues".into(), json!([["issuetype"]]));
        let issue = |key: &str, kind: &str, parent: Value| {
            json!({ "key": key, "fields": {
                "issuetype": { "name": kind },
                "status": { "name": "To Do", "statusCategory": { "key": "new" } },
                "parent": parent
            } })
        };
        host.answer("POST", "/rest/api/3/search/jql", 200, json!({ "isLast": true, "issues": [
            issue("P-1", "Bug", Value::Null),
            issue("P-2", "Subtask", json!({ "key": "P-9", "fields": { "issuetype": { "name": "Bug" } } })),
            issue("P-3", "Story", Value::Null),
            issue("P-4", "Mystery", json!({ "key": "P-8", "fields": { "issuetype": { "name": "Story" } } }))
        ] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let node = page(&jira, &fields, List::Mine, &page_params(json!({}))).unwrap();
        let json = serde_json::to_value(&node).unwrap();
        let table = json["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|child| child["type"] == "table")
            .unwrap()
            .clone();
        let rows = table["rows"].as_array().unwrap();
        let type_theme = |row: &Value| row["cells"][1]["theme"].clone();
        assert_eq!(type_theme(&rows[0]), "danger");
        assert_eq!(type_theme(&rows[1]), "info");
        assert_eq!(type_theme(&rows[2]), "success");
        assert!(
            rows[3]["cells"][1].get("theme").is_none(),
            "an unknown type has no theme"
        );
        // A bug, or an issue whose parent is a bug, tints its row.
        assert_eq!(rows[0]["theme"], "danger");
        assert_eq!(rows[1]["theme"], "danger");
        assert!(rows[2].get("theme").is_none());
        assert!(rows[3].get("theme").is_none());
        // The type is fetched for the tint even when no column shows it.
        host.store
            .borrow_mut()
            .insert("columns.issues".into(), json!([["summary"]]));
        host.sent.borrow_mut().clear();
        page(&jira, &fields, List::Mine, &page_params(json!({}))).unwrap();
        let sent = host.sent.borrow();
        let search = sent
            .iter()
            .find(|sent| sent.url.ends_with("search/jql"))
            .and_then(|sent| sent.body.clone())
            .unwrap();
        assert!(search.contains("\"issuetype\""));
    }

    #[test]
    fn searching_sorting_and_paging_change_the_address() {
        let host = connected();
        let jira = Jira::connect(&host).unwrap().unwrap();
        let current =
            json!({ "jql": "project = ATL", "token": "abc", "sort": "summary", "dir": "asc" });
        let result = action(
            &jira,
            &request(
                "issues",
                "list.search",
                json!({}),
                json!({ "jql": MY_JQL, "f.status": ["Done"], "f.project": [] }),
                current.clone(),
            ),
        )
        .unwrap()
        .unwrap();
        let navigate = result.navigate.unwrap();
        assert_eq!(navigate.page, "issues");
        assert_eq!(
            Value::Object(navigate.params),
            json!({ "f.status": ["Done"], "sort": "summary", "dir": "asc" })
        );
        let sorted = action(
            &jira,
            &request(
                "all",
                "list.sort",
                json!({ "field": "summary" }),
                json!({}),
                current.clone(),
            ),
        )
        .unwrap()
        .unwrap()
        .navigate
        .unwrap();
        assert_eq!(sorted.page, "all");
        assert_eq!(sorted.params["dir"], "desc");
        assert!(!sorted.params.contains_key("token"));
        let other = action(
            &jira,
            &request(
                "all",
                "list.sort",
                json!({ "field": "status" }),
                json!({}),
                current,
            ),
        )
        .unwrap()
        .unwrap()
        .navigate
        .unwrap();
        assert_eq!(
            (other.params["sort"].clone(), other.params["dir"].clone()),
            (json!("status"), json!("asc"))
        );
    }

    fn column_ids(host: &FakeHost, list: List) -> Vec<String> {
        columns(host, list)
            .unwrap()
            .iter()
            .map(ListColumn::id)
            .collect()
    }

    #[test]
    fn columns_from_any_layer_are_added_ordered_and_removed_in_one_list() {
        let host = connected();
        let jira = Jira::connect(&host).unwrap().unwrap();
        let run = |name: &str, params: Value, form: Value| {
            action(&jira, &request("all", name, params, form, json!({})))
                .unwrap()
                .unwrap()
        };
        let empty = run(
            "columns.add",
            json!({}),
            json!({ "layer0": "", "layer1": "", "layer2": "" }),
        );
        assert_eq!(empty.message.unwrap().theme, "warning");
        // One Add can take a column from each layer.
        run(
            "columns.add",
            json!({}),
            json!({ "layer0": "", "layer1": KEY, "layer2": STORY_POINTS }),
        );
        run("columns.add", json!({}), json!({ "layer1": STORY_POINTS }));
        // Adding a column that is already there changes nothing.
        run("columns.add", json!({}), json!({ "layer1": KEY }));
        let added = column_ids(&host, List::All);
        assert_eq!(
            added[added.len() - 3..],
            ["1:issuekey", "2:storyPoints", "1:storyPoints"]
        );
        // The report Erik wants: Key, Parent key, Parent story points, then the rest.
        run(
            "columns.order",
            json!({}),
            json!({ "order": ["0:issuekey", "1:issuekey", "1:storyPoints", "0:summary"] }),
        );
        let ordered = column_ids(&host, List::All);
        assert_eq!(
            ordered[..4],
            ["0:issuekey", "1:issuekey", "1:storyPoints", "0:summary"]
        );
        assert_eq!(
            ordered.len(),
            added.len(),
            "columns the order left out stay"
        );
        assert_eq!(ordered.last().unwrap(), "2:storyPoints");
        run("columns.move", json!({ "index": 1, "by": 1 }), json!({}));
        assert_eq!(
            column_ids(&host, List::All)[1..3],
            ["1:storyPoints", "1:issuekey"]
        );
        run("columns.remove", json!({ "index": 0 }), json!({}));
        assert_eq!(column_ids(&host, List::All)[0], "1:storyPoints");
        // Each list keeps its own columns.
        assert_eq!(columns(&host, List::Mine).unwrap(), default_columns());
        run("columns.reset", json!({}), json!({}));
        assert_eq!(columns(&host, List::All).unwrap(), default_columns());
    }

    #[test]
    fn my_issues_keep_finished_subtasks_until_their_parent_is_finished() {
        let host = connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        let done = json!({ "name": "Done", "statusCategory": { "key": "done" } });
        let review = json!({ "name": "Code Review", "statusCategory": { "key": "indeterminate" } });
        // The parents: IR-1 is still in Code Review, and IR-2 is finished.
        host.answer_when(
            "POST",
            "/rest/api/3/search/jql",
            "key in",
            200,
            json!({ "isLast": true, "issues": [
            { "key": "IR-1", "fields": { "status": review, "resolution": null } },
            { "key": "IR-2", "fields": { "status": done, "resolution": { "name": "Done" } } }
        ] }),
        );
        host.answer("POST", "/rest/api/3/search/jql", 200, json!({ "isLast": true, "issues": [
            { "key": "IR-10", "fields": { "summary": "Development", "status": done, "resolution": { "name": "Done" }, "parent": { "key": "IR-1" } } },
            { "key": "IR-20", "fields": { "summary": "Development", "status": done, "resolution": { "name": "Done" }, "parent": { "key": "IR-2" } } },
            { "key": "IR-30", "fields": { "summary": "Open work", "status": review, "resolution": null } }
        ] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let shown = |list: List, params: Value| {
            let node = page(&jira, &fields, list, params.as_object().unwrap()).unwrap();
            serde_json::to_value(&node).unwrap().to_string()
        };
        let mine = shown(List::Mine, json!({}));
        assert!(mine.contains("IR-10"), "its parent is still in code review");
        assert!(!mine.contains("IR-20"), "its parent is finished");
        assert!(mine.contains("IR-30"));
        assert!(mine.contains("1 finished subtask stays here until its parent is finished."));
        let sent = host
            .sent
            .borrow()
            .iter()
            .find_map(|sent| {
                sent.body
                    .clone()
                    .filter(|body| body.contains("currentUser"))
            })
            .unwrap();
        assert!(sent.contains("subTaskIssueTypes()"));
        drop(sent);
        // A search the user wrote shows what it finds.
        let own = shown(List::Mine, json!({ "jql": "project = IR" }));
        assert!(own.contains("IR-20"));
    }

    #[test]
    fn my_issues_reads_on_when_finished_subtasks_leave_the_page_short() {
        let host = connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        let done = json!({ "name": "Done", "statusCategory": { "key": "done" } });
        let open = json!({ "name": "To Do", "statusCategory": { "key": "new" } });
        host.answer_when(
            "POST",
            "/rest/api/3/search/jql",
            "key in",
            200,
            json!({ "isLast": true, "issues": [
            { "key": "IR-2", "fields": { "status": done, "resolution": { "name": "Done" } } }
        ] }),
        );
        // The second page, asked for only once the first came up short.
        host.answer_when("POST", "/rest/api/3/search/jql", r#""nextPageToken":"p2""#, 200, json!({ "isLast": true, "issues": [
            { "key": "IR-40", "fields": { "summary": "Later work", "status": open, "resolution": null } }
        ] }));
        host.answer("POST", "/rest/api/3/search/jql", 200, json!({ "isLast": false, "nextPageToken": "p2", "issues": [
            { "key": "IR-20", "fields": { "summary": "Development", "status": done, "resolution": { "name": "Done" }, "parent": { "key": "IR-2" } } },
            { "key": "IR-30", "fields": { "summary": "Open work", "status": open, "resolution": null } }
        ] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let node = page(&jira, &fields, List::Mine, json!({}).as_object().unwrap()).unwrap();
        let shown = serde_json::to_value(&node).unwrap().to_string();
        assert!(shown.contains("IR-30") && shown.contains("IR-40"));
        assert!(!shown.contains("IR-20"), "its parent is finished");
        assert!(!shown.contains("Next 50"), "Jira has no more");
        let searches: Vec<String> = host
            .sent
            .borrow()
            .iter()
            .filter_map(|sent| sent.body.clone())
            .filter(|body| body.contains("currentUser"))
            .collect();
        assert_eq!(searches.len(), 2);
        assert!(searches[0].contains(r#""maxResults":50"#));
        // Only as many as the page still needs.
        assert!(searches[1].contains(r#""maxResults":49"#));
    }

    #[test]
    fn columns_saved_per_layer_become_one_list_after_the_key() {
        let host = connected();
        host.store.borrow_mut().insert(
            "columns.issues".into(),
            json!([["summary", "status"], ["summary"], ["status", "summary"]]),
        );
        assert_eq!(
            column_ids(&host, List::Mine),
            [
                "0:issuekey",
                "0:summary",
                "0:status",
                "1:summary",
                "2:status",
                "2:summary"
            ]
        );
        host.store
            .borrow_mut()
            .insert("columns.issues".into(), json!({ "not": "columns" }));
        assert_eq!(columns(&host, List::Mine).unwrap(), default_columns());
    }

    #[test]
    fn the_editor_drags_columns_where_task_proxy_can_and_uses_arrows_elsewhere() {
        let host = connected();
        let jira = Jira::connect(&host).unwrap().unwrap();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        let fields = Fields::load(&jira).unwrap();
        let chosen = vec![ListColumn::new(0, KEY), ListColumn::new(1, STORY_POINTS)];
        let editor =
            |host: &FakeHost| serde_json::to_value(columns_editor(host, &fields, &chosen)).unwrap();

        // Task Proxy 0.1.121 and earlier answer no hostInfo, and cannot draw a reorder list.
        let old = editor(&host);
        assert!(!old.to_string().contains("\"reorder\""));
        assert!(old.to_string().contains("Move down"));

        *host.spec.borrow_mut() = Some("0.1.2".into());
        let new = editor(&host);
        let list = new
            .as_array()
            .unwrap()
            .iter()
            .find(|node| node["type"] == "reorder")
            .unwrap();
        assert_eq!(list["items"][0]["id"], "0:issuekey");
        assert_eq!(list["items"][1]["id"], "1:storyPoints");
        assert_eq!(
            list["items"][1]["children"][0]["text"],
            "Parent story points"
        );
        assert_eq!(list["action"]["id"], "columns.order");
        // One row: Add a column, then a dropdown for each layer, then Add.
        let row = &new[1];
        assert_eq!(row["children"][0]["text"], "Add a column");
        let add = &row["children"][1];
        assert_eq!(add["inline"], true);
        let names: Vec<&str> = add["fields"]
            .as_array()
            .unwrap()
            .iter()
            .map(|field| field["label"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Issue", "Parent", "Grandparent"]);
        // A column already chosen is not offered again for its layer, but is for the others.
        let offered = |layer: usize, id: &str| {
            add["fields"][layer]["options"]
                .as_array()
                .unwrap()
                .iter()
                .any(|option| option["value"] == id)
        };
        assert!(!offered(0, KEY));
        assert!(offered(1, KEY));
        assert!(!offered(1, STORY_POINTS));
        assert!(offered(2, STORY_POINTS));
    }

    #[test]
    fn story_points_are_set_through_the_board_and_on_the_edit_screen_then_checked() {
        // Jira shows "Story Points" on IR, whose board estimates with it, but its edit screen only has "Story point
        // estimate", so the board's estimation sets it.
        let setup = |after: Value| {
            let host = connected();
            host.answer("GET", "/rest/api/3/field", 200, sample_fields());
            host.answer(
                "GET",
                "/rest/agile/1.0/board?projectKeyOrId=IR&maxResults=20",
                200,
                json!({ "values": [{ "id": 8 }] }),
            );
            host.answer(
                "GET",
                "/rest/agile/1.0/board/8/configuration",
                200,
                json!({ "estimation": {
                "type": "field", "field": { "fieldId": "customfield_10026" }
            } }),
            );
            host.answer(
                "GET",
                "/rest/api/3/issue/IR-10/editmeta",
                200,
                json!({ "fields": { "customfield_10016": {}, "summary": {} } }),
            );
            host.answer(
                "PUT",
                "/rest/agile/1.0/issue/IR-10/estimation?boardId=8",
                200,
                json!({ "fieldId": "customfield_10026" }),
            );
            host.answer("PUT", "/rest/api/3/issue/IR-10", 204, Value::Null);
            host.answer(
                "GET",
                "/rest/api/3/issue/IR-10?fields=customfield_10016,customfield_10026",
                200,
                after,
            );
            host
        };
        let host = setup(
            json!({ "key": "IR-10", "fields": { "customfield_10016": 8, "customfield_10026": 8 } }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        set_choice(&jira, &fields, "IR-10", "customfield_10016", "8").unwrap();
        let writes: Vec<(String, Value)> = host
            .sent
            .borrow()
            .iter()
            .filter(|sent| sent.method == "PUT")
            .map(|sent| {
                (
                    sent.url.clone(),
                    serde_json::from_str(sent.body.as_deref().unwrap()).unwrap(),
                )
            })
            .collect();
        assert_eq!(
            writes,
            vec![
                (
                    "https://team.atlassian.net/rest/agile/1.0/issue/IR-10/estimation?boardId=8"
                        .to_string(),
                    json!({ "value": "8" })
                ),
                (
                    "https://team.atlassian.net/rest/api/3/issue/IR-10".to_string(),
                    json!({ "fields": { "customfield_10016": 8.0 } })
                ),
            ]
        );

        // Clearing sends no value.
        let host = setup(
            json!({ "key": "IR-10", "fields": { "customfield_10016": null, "customfield_10026": null } }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        set_choice(&jira, &fields, "IR-10", "customfield_10016", "").unwrap();
        let sent = host.sent.borrow();
        let estimate = sent
            .iter()
            .find(|sent| sent.url.contains("/estimation"))
            .unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(estimate.body.as_deref().unwrap()).unwrap(),
            json!({ "value": null })
        );
        drop(sent);

        // Jira took the change but the points it shows did not change, which is an error, not silence.
        let host = setup(
            json!({ "key": "IR-10", "fields": { "customfield_10016": 3, "customfield_10026": 2 } }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let error = set_choice(&jira, &fields, "IR-10", "customfield_10016", "3").unwrap_err();
        assert!(
            error
                .0
                .contains("Jira did not change Story Points on IR-10"),
            "{}",
            error.0
        );

        // With no board that estimates with story points, every one on the edit screen is set.
        let host = connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.answer(
            "GET",
            "/rest/agile/1.0/board?projectKeyOrId=P&maxResults=20",
            200,
            json!({ "values": [] }),
        );
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1/editmeta",
            200,
            json!({ "fields": { "customfield_10016": {}, "customfield_10026": {} } }),
        );
        host.answer("PUT", "/rest/api/3/issue/P-1", 204, Value::Null);
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1?fields=customfield_10016,customfield_10026",
            200,
            json!({ "key": "P-1", "fields": { "customfield_10016": 5, "customfield_10026": 5 } }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        set_choice(&jira, &fields, "P-1", "customfield_10026", "5").unwrap();
        let sent = host.sent.borrow();
        let put = sent.iter().find(|sent| sent.method == "PUT").unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(put.body.as_deref().unwrap()).unwrap(),
            json!({ "fields": { "customfield_10016": 5.0, "customfield_10026": 5.0 } })
        );
    }

    #[test]
    fn parent_keys_and_story_points_come_from_the_parent() {
        let host = connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.store.borrow_mut().insert(
            "columns.issues".into(),
            json!([
                { "layer": 0, "field": KEY },
                { "layer": 1, "field": KEY },
                { "layer": 1, "field": STORY_POINTS }
            ]),
        );
        // The child has no points. Its company-managed parent keeps them in "Story Points", not "Story point
        // estimate", which the site also has.
        host.answer_when("POST", "/rest/api/3/search/jql", "key in", 200, json!({ "isLast": true, "issues": [
            { "key": "IR-10", "fields": { "summary": "Story", "customfield_10016": null, "customfield_10026": 8 } }
        ] }));
        host.answer(
            "POST",
            "/rest/api/3/search/jql",
            200,
            json!({ "isLast": true, "issues": [
            { "key": "IR-11", "fields": { "parent": { "key": "IR-10" } } }
        ] }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let node = page(&jira, &fields, List::Mine, &Map::new()).unwrap();
        let json = serde_json::to_value(&node).unwrap();
        let table = json["children"]
            .as_array()
            .unwrap()
            .iter()
            .find(|child| child["type"] == "table")
            .unwrap()
            .clone();
        assert_eq!(table["columns"][0]["label"], "Key");
        assert_eq!(table["columns"][1], "Parent key");
        assert_eq!(table["columns"][2], "Parent story points");
        let cells = &table["rows"][0]["cells"];
        assert_eq!(cells[0]["text"], "IR-11");
        assert_eq!(cells[1]["type"], "link");
        assert_eq!(cells[1]["text"], "IR-10");
        assert_eq!(cells[2]["text"], "8");
        // The parents were asked for both story points fields.
        let sent = host.sent.borrow();
        let parents = sent
            .iter()
            .find(|sent| sent.body.as_deref().unwrap_or("").contains("key in"))
            .unwrap();
        let body: Value = serde_json::from_str(parents.body.as_deref().unwrap()).unwrap();
        let asked: Vec<&str> = body["fields"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        assert!(asked.contains(&"customfield_10016") && asked.contains(&"customfield_10026"));
    }

    #[test]
    fn cells_change_their_issue() {
        let host = connected();
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1/transitions",
            200,
            json!({ "transitions": [
            { "id": "31", "name": "Finish", "to": { "name": "Done" } }
        ] }),
        );
        host.answer(
            "POST",
            "/rest/api/3/issue/P-1/transitions",
            204,
            Value::Null,
        );
        host.answer("PUT", "/rest/api/3/issue/P-1", 204, Value::Null);
        // Story points are read back after they are set.
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1?fields=customfield_10016,customfield_10026",
            200,
            json!({ "key": "P-1", "fields": { "customfield_10016": 5 } }),
        );
        host.answer("POST", "/rest/agile/1.0/sprint/42/issue", 204, Value::Null);
        let jira = Jira::connect(&host).unwrap().unwrap();
        let run = |name: &str, params: Value, form: Value| {
            action(&jira, &request("issues", name, params, form, json!({})))
                .unwrap()
                .unwrap()
        };
        let options = run(
            "cell.options",
            json!({ "key": "P-1", "field": "status" }),
            json!({}),
        )
        .options
        .unwrap();
        assert_eq!(options, vec![Choice::new("31", "Done")]);
        let points = run(
            "cell.options",
            json!({ "key": "P-1", "field": "customfield_10016" }),
            json!({}),
        )
        .options
        .unwrap();
        assert_eq!(points[0], Choice::new("", "None"));
        assert!(points.iter().any(|choice| choice.value == "13"));
        assert!(
            run(
                "cell.set",
                json!({ "key": "P-1", "field": "status" }),
                json!({ "value": "31" })
            )
            .refresh
        );
        run(
            "cell.set",
            json!({ "key": "P-1", "field": "customfield_10016" }),
            json!({ "value": "5" }),
        );
        run(
            "cell.set",
            json!({ "key": "P-1", "field": "customfield_10020" }),
            json!({ "value": "42" }),
        );
        let sent = host.sent.borrow();
        let bodies: Vec<(String, String)> = sent
            .iter()
            .filter(|sent| sent.method != "GET")
            .map(|sent| (sent.url.clone(), sent.body.clone().unwrap_or_default()))
            .collect();
        assert!(bodies.contains(&(
            "https://team.atlassian.net/rest/api/3/issue/P-1/transitions".into(),
            json!({ "transition": { "id": "31" } }).to_string()
        )));
        assert!(bodies.contains(&(
            "https://team.atlassian.net/rest/api/3/issue/P-1".into(),
            json!({ "fields": { "customfield_10016": 5.0 } }).to_string()
        )));
        assert!(bodies.contains(&(
            "https://team.atlassian.net/rest/agile/1.0/sprint/42/issue".into(),
            json!({ "issues": ["P-1"] }).to_string()
        )));
    }

    #[test]
    fn long_text_opens_in_a_dialog_where_it_can_be_edited() {
        let host = connected();
        *host.spec.borrow_mut() = Some("0.1.3".into());
        host.answer("GET", "/rest/api/3/issue/P-1", 200, json!({ "key": "P-1", "fields": {
            "description": { "type": "doc", "version": 1, "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "All of it" }] }] }
        } }));
        host.answer("PUT", "/rest/api/3/issue/P-1", 204, Value::Null);
        let jira = Jira::connect(&host).unwrap().unwrap();
        let opened = action(
            &jira,
            &request(
                "issues",
                "cell.open",
                json!({ "key": "P-1", "field": "description" }),
                json!({}),
                json!({}),
            ),
        )
        .unwrap()
        .unwrap();
        let dialog = opened.dialog.unwrap();
        assert_eq!(dialog.title, "P-1 · Description");
        // A description shows rendered, with a switch to the markdown as written, and Edit opens its editor.
        let shown = serde_json::to_value(&dialog.ui).unwrap();
        assert_eq!(shown["children"][0]["type"], "markdown");
        assert_eq!(shown["children"][0]["text"], "All of it");
        assert_eq!(shown["children"][0]["toggle"], true);
        assert_eq!(
            shown["children"][1]["children"][0]["action"]["host"]["type"],
            "toggle"
        );
        let form = &shown["children"][2]["children"][0];
        assert_eq!(form["type"], "form");
        assert_eq!(form["fields"][0]["value"], "All of it");
        let saved = action(
            &jira,
            &request(
                "issues",
                "cell.save",
                json!({ "key": "P-1", "field": "description" }),
                json!({ "value": "**New** text" }),
                json!({}),
            ),
        )
        .unwrap()
        .unwrap();
        assert!(saved.close_dialog && saved.refresh);
        let sent = host.sent.borrow();
        let put = sent.iter().find(|sent| sent.method == "PUT").unwrap();
        assert!(put.body.as_ref().unwrap().contains("\"type\":\"doc\""));
    }

    #[test]
    fn comments_open_with_every_comment_and_mention_people() {
        let host = connected();
        host.answer("GET", "/rest/api/3/issue/P-1/comment", 200, json!({ "comments": [
            { "author": { "displayName": "Sam" }, "created": "2026-11-10T09:12:00.000+0000",
              "body": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "First" }] }] } }
        ] }));
        host.answer("POST", "/rest/api/3/issue/P-1/comment", 201, json!({}));
        host.answer(
            "GET",
            "/rest/api/3/user/viewissue/search",
            200,
            json!([
                { "accountId": "sam-1", "displayName": "Sam Lee", "accountType": "atlassian" },
                { "accountId": "bot", "displayName": "Automation", "accountType": "app" }
            ]),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let run = |name: &str, params: Value, form: Value| {
            action(&jira, &request("issues", name, params, form, json!({})))
                .unwrap()
                .unwrap()
        };
        let opened = run("cell.comments", json!({ "key": "P-1" }), json!({}));
        let ui = serde_json::to_string(&opened.dialog.unwrap().ui).unwrap();
        assert!(ui.contains("First") && ui.contains("users.suggest"));
        let people = run(
            "users.suggest",
            json!({ "key": "P-1", "query": "sa" }),
            json!({}),
        )
        .options
        .unwrap();
        assert_eq!(people, vec![Choice::new("sam-1", "Sam Lee")]);
        assert!(
            host.sent
                .borrow()
                .iter()
                .any(|sent| sent.url.ends_with("issueKey=P-1&query=sa&maxResults=10"))
        );
        let added = run(
            "cell.comment",
            json!({ "key": "P-1" }),
            json!({ "body": "Over to you @Sam Lee", "body.mentions": { "@Sam Lee": "sam-1" } }),
        );
        assert!(added.ui.is_some() && added.refresh && !added.close_dialog);
        let sent = host.sent.borrow();
        let post = sent
            .iter()
            .find(|sent| sent.method == "POST" && sent.url.ends_with("/comment"))
            .unwrap();
        assert!(
            post.body.as_ref().unwrap().contains(
                "{\"attrs\":{\"id\":\"sam-1\",\"text\":\"@Sam Lee\"},\"type\":\"mention\"}"
            )
        );
    }
}
