//! My Issues and All Issues: a search built from filters and JQL, columns chosen from the issue, its parent, and
//! its grandparent, titles that sort, and cells that change their issue in place.
//!
//! The page's address holds the search, so Back returns to it: `jql`, each filter as `f.<name>` (a JSON list),
//! `sort` and `dir`, and `token` for the page of results.
use crate::adf;
use crate::client::{Jira, encode};
use crate::fields::{self, Fields};
use crate::pages::{open_issue, param};
use crate::views::{SPRINT, STORY_POINTS, field_label, layer_name, portable, resolve};
use serde::{Deserialize, Serialize};
use taskproxy_plugin_sdk::ui::{
    self, Action, Choice, Column, Field, FieldKind, Node, Sort, TableRow,
};
use taskproxy_plugin_sdk::{ActionRequest, ActionResult, Error, Host, Map, Result, Value, json};

const PAGE_SIZE: u64 = 50;
/// The issue, its parent, and its grandparent.
pub const LAYERS: usize = 3;
const FILTER_OPTIONS_KEY: &str = "filterOptions";
const FILTER_OPTIONS_MS: u64 = 60 * 60 * 1000;
const USERS_KEY: &str = "users";
const USERS_MS: u64 = 24 * 60 * 60 * 1000;
pub const MY_JQL: &str =
    "assignee = currentUser() AND resolution = Unresolved ORDER BY updated DESC";
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

fn default_columns() -> Vec<Vec<String>> {
    vec![
        [
            "summary",
            "status",
            "priority",
            SPRINT,
            STORY_POINTS,
            "updated",
        ]
        .map(String::from)
        .to_vec(),
        Vec::new(),
        Vec::new(),
    ]
}

/// The fields shown for each layer, as Views save them, so story points and sprint work on any site.
fn columns(host: &dyn Host, list: List) -> Result<Vec<Vec<String>>> {
    let mut layers: Vec<Vec<String>> = host
        .store_get(list.columns_key())?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_else(default_columns);
    layers.resize(LAYERS, Vec::new());
    layers.truncate(LAYERS);
    Ok(layers)
}

fn save_columns(host: &dyn Host, list: List, layers: &[Vec<String>]) -> Result<()> {
    host.store_set(list.columns_key(), Some(serde_json::to_value(layers)?))
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
fn is_choice(real: &str, fields: &Fields) -> bool {
    matches!(real, "status" | "priority" | "assignee")
        || fields.sprint().as_deref() == Some(real)
        || fields.story_points().as_deref() == Some(real)
        || fields
            .find(real)
            .is_some_and(|field| field.kind == "option")
}

/// Fields of text the user can edit in a dialog.
fn is_editable_text(real: &str, fields: &Fields) -> bool {
    matches!(real, "summary" | "description" | "environment")
        || (real.starts_with("customfield_")
            && fields
                .find(real)
                .is_some_and(|field| field.kind == "string"))
}

/// Rich text fields hold Jira documents rather than plain strings.
fn is_rich_text(real: &str, fields: &Fields) -> bool {
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

fn cell(fields: &Fields, real: &str, label: &str, issue: Option<&Value>) -> Node {
    let Some(issue) = issue else {
        return ui::muted("");
    };
    let key = key_of(issue);
    let value = issue
        .pointer(&format!("/fields/{}", real))
        .unwrap_or(&Value::Null);
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

/// The page: the search form, the column chooser, saved searches, the table, and paging.
pub fn page(
    jira: &Jira,
    fields: &Fields,
    list: List,
    page_params: &Map<String, Value>,
) -> Result<Node> {
    let host = jira.host;
    let layers = columns(host, list)?;
    let jql = match param(page_params, "jql") {
        "" => list.default_jql(),
        jql => jql,
    };
    let token = param(page_params, "token");
    let resolved: Vec<Vec<(String, String)>> = layers
        .iter()
        .map(|layer| {
            layer
                .iter()
                .filter_map(|id| Some((id.clone(), resolve(id, fields)?)))
                .collect()
        })
        .collect();
    let wanted = |depth: usize| -> Vec<String> {
        resolved[depth]
            .iter()
            .map(|(_, real)| real.clone())
            .collect()
    };
    let mut first = wanted(0);
    first.push("parent".into());
    let mut body = json!({
        "jql": search_jql(list, page_params, fields),
        "maxResults": PAGE_SIZE,
        "fields": first,
    });
    if !token.is_empty() {
        body["nextPageToken"] = json!(token);
    }
    let found = jira.post("search/jql", &body)?;
    let issues = issues_of(&found);
    let want_parents = !resolved[1].is_empty() || !resolved[2].is_empty();
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
    let grandparents = if !resolved[2].is_empty() {
        let keys = unique(
            parents
                .iter()
                .filter_map(|i| parent_key(i).map(String::from)),
        );
        fetch(jira, &keys, &wanted(2))?
    } else {
        Vec::new()
    };
    let by_key = |pool: &[Value], key: Option<&str>| -> Option<Value> {
        let key = key?;
        pool.iter().find(|issue| key_of(issue) == key).cloned()
    };

    let sort = sort_of(page_params);
    let mut table_columns: Vec<Column> = vec!["Key".into()];
    for (depth, layer) in resolved.iter().enumerate() {
        for (id, real) in layer {
            let name = field_label(id, fields);
            let label = if depth == 0 {
                name
            } else {
                format!("{} {}", layer_name(depth), name.to_lowercase())
            };
            if depth == 0 && sort_name(real, fields).is_some() {
                table_columns.push(Column::Sortable {
                    action: Some(
                        Action::new("list.sort", format!("Sort by {}", label))
                            .param("field", id.clone()),
                    ),
                    sort: sort
                        .as_ref()
                        .filter(|(field, _)| field == id)
                        .map(|(_, dir)| *dir),
                    label,
                });
            } else {
                table_columns.push(label.into());
            }
        }
    }
    let rows: Vec<TableRow> = issues
        .iter()
        .map(|issue| {
            let key = key_of(issue).to_string();
            let parent = by_key(&parents, parent_key(issue));
            let grandparent = parent
                .as_ref()
                .and_then(|parent| by_key(&grandparents, parent_key(parent)));
            let at_depth = [Some(issue.clone()), parent, grandparent];
            let mut cells = vec![ui::link(key.clone(), open_issue(&key))];
            for (depth, layer) in resolved.iter().enumerate() {
                for (id, real) in layer {
                    cells.push(cell(
                        fields,
                        real,
                        &field_label(id, fields),
                        at_depth[depth].as_ref(),
                    ));
                }
            }
            TableRow {
                cells,
                action: None,
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
        ],
        submit: Action::new("list.search", "Search").icon("magnifying-glass"),
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
    }];

    let mut saved_links = vec![ui::toggle("Columns", COLUMNS_PANEL)];
    saved_links.push(ui::link(
        if list == List::Mine {
            "My open issues"
        } else {
            "All open issues"
        },
        Action::new("list.reset", "Clear the search"),
    ));
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
    children.push(ui::row(saved_links));
    children.push(ui::panel(
        COLUMNS_PANEL,
        false,
        columns_editor(fields, &layers),
    ));
    children.push(ui::form(
        vec![Field::new(FieldKind::Text, "name", "Save this search as").placeholder("Sprint bugs")],
        Action::new("list.save", "Save search"),
    ));
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

fn columns_editor(fields: &Fields, layers: &[Vec<String>]) -> Vec<Node> {
    let choices: Vec<Choice> = fields
        .choices()
        .into_iter()
        .map(|field| {
            let id = portable(&field.id, fields);
            let label = field_label(&id, fields);
            Choice::new(id, label)
        })
        .collect();
    let mut nodes = vec![ui::muted(
        "Choose the columns for each issue, its parent, and its grandparent. They are saved for you.",
    )];
    for (depth, layer) in layers.iter().enumerate() {
        let mut items = Vec::new();
        for (index, id) in layer.iter().enumerate() {
            let at = |action: &str, label: &str| {
                Action::new(action, label)
                    .param("layer", depth)
                    .param("index", index)
            };
            let mut line = vec![ui::text(field_label(id, fields))];
            if index > 0 {
                line.push(ui::link(
                    "←",
                    at("columns.move", "Move left").param("by", -1),
                ));
            }
            if index + 1 < layer.len() {
                line.push(ui::link(
                    "→",
                    at("columns.move", "Move right").param("by", 1),
                ));
            }
            line.push(ui::link("✕", at("columns.remove", "Remove")));
            items.push(ui::row(line));
        }
        if items.is_empty() {
            items.push(ui::muted("No columns."));
        }
        let available: Vec<Choice> = choices
            .iter()
            .filter(|choice| !layer.contains(&choice.value))
            .cloned()
            .collect();
        items.push(ui::form(
            vec![Field::new(FieldKind::Select, "field", "Add a column").options(available)],
            Action::new("columns.add", "Add").param("layer", depth),
        ));
        nodes.push(ui::section(
            format!("columns-{}", depth),
            layer_name(depth),
            depth == 0,
            items,
        ));
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
    if fields.story_points().as_deref() == Some(real) {
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
    } else if fields.story_points().as_deref() == Some(real) {
        let points = if value.is_empty() {
            Value::Null
        } else {
            json!(
                value
                    .parse::<f64>()
                    .map_err(|_| Error::new("Story points must be a number."))?
            )
        };
        jira.put(&issue, &json!({ "fields": { real: points } }))?;
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

fn text_dialog(jira: &Jira, fields: &Fields, key: &str, real: &str) -> Result<Node> {
    let issue = jira.get(&format!("issue/{}?fields={}", encode(key), encode(real)))?;
    let value = issue
        .pointer(&format!("/fields/{}", real))
        .unwrap_or(&Value::Null);
    if !is_editable_text(real, fields) {
        return Ok(fields::value_node(real, value));
    }
    let text = if value.get("type").and_then(Value::as_str) == Some("doc") {
        adf::to_markdown(value)
    } else {
        value.as_str().unwrap_or("").to_string()
    };
    let field = if real == "summary" {
        Field::new(FieldKind::Text, "value", "Summary").required()
    } else {
        Field::new(FieldKind::Textarea, "value", fields.name(real))
            .placeholder("Markdown: **bold**, *italic*, `code`, lists, and links")
    };
    Ok(Node::Form {
        fields: vec![field.value(text)],
        submit: Action::new("cell.save", "Save")
            .param("key", key)
            .param("field", real),
        cancel: Some(Action::new("cell.close", "Cancel")),
        buttons: Vec::new(),
        children: Vec::new(),
    })
}

/// Answers the actions of My Issues and All Issues, or `None` when the action is not one of them.
pub fn action(jira: &Jira, request: &ActionRequest) -> Result<Option<ActionResult>> {
    let host = jira.host;
    let list = List::of(&request.page);
    let p = &request.params;
    let key = param(p, "key");
    let real = param(p, "field");
    let result = match request.action.as_str() {
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
        "columns.add" | "columns.remove" | "columns.move" | "columns.reset" => {
            let mut layers = columns(host, list)?;
            let depth = param_i64(p, "layer") as usize;
            let index = param_i64(p, "index") as usize;
            match request.action.as_str() {
                "columns.reset" => layers = default_columns(),
                "columns.add" => {
                    let field = form_text(request, "field");
                    if field.is_empty() {
                        return Ok(Some(
                            ActionResult::default()
                                .with_message("Choose a column to add.", "warning"),
                        ));
                    }
                    if let Some(layer) = layers.get_mut(depth)
                        && !layer.iter().any(|id| id == field)
                    {
                        layer.push(field.to_string());
                    }
                }
                "columns.remove" => {
                    if let Some(layer) = layers.get_mut(depth)
                        && index < layer.len()
                    {
                        layer.remove(index);
                    }
                }
                _ => {
                    if let Some(layer) = layers.get_mut(depth) {
                        let target = index as i64 + param_i64(p, "by");
                        if index < layer.len() && (0..layer.len() as i64).contains(&target) {
                            layer.swap(index, target as usize);
                        }
                    }
                }
            }
            save_columns(host, list, &layers)?;
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
                    adf::from_markdown(text)
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
            "(assignee = currentUser() AND resolution = Unresolved) ORDER BY updated DESC"
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
        assert_eq!(titles[0], "Key");
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
        assert_eq!(searches.len(), 3);
        assert!(searches[0].contains("ORDER BY summary ASC"));
        // The fake answers every search alike, so its parents are asked for too.
        assert!(searches[1].contains("key in (\\\"P-2\\\""));
        assert!(searches[2].contains("\\\"P-1\\\""));
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

    #[test]
    fn columns_are_added_moved_and_removed_per_page() {
        let host = connected();
        let jira = Jira::connect(&host).unwrap().unwrap();
        let run = |name: &str, params: Value, form: Value| {
            action(&jira, &request("all", name, params, form, json!({})))
                .unwrap()
                .unwrap()
        };
        run(
            "columns.add",
            json!({ "layer": 1 }),
            json!({ "field": "assignee" }),
        );
        run(
            "columns.add",
            json!({ "layer": 1 }),
            json!({ "field": "comment" }),
        );
        run(
            "columns.move",
            json!({ "layer": 1, "index": 1, "by": -1 }),
            json!({}),
        );
        run(
            "columns.remove",
            json!({ "layer": 0, "index": 0 }),
            json!({}),
        );
        let layers = columns(&host, List::All).unwrap();
        assert_eq!(layers[1], vec!["comment", "assignee"]);
        assert_eq!(layers[0][0], "status");
        assert_eq!(columns(&host, List::Mine).unwrap(), default_columns());
        run("columns.reset", json!({}), json!({}));
        assert_eq!(columns(&host, List::All).unwrap(), default_columns());
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
        let form = serde_json::to_value(&dialog.ui).unwrap();
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
