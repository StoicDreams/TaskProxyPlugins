//! Views: which fields to show for an issue, its parent, the parent's parent, and so on. Each level is a layer,
//! drawn as its own collapsible section.
//!
//! Personal Views are kept in the plugin's storage. Project Views are kept in `.taskproxy/plugins/jira/views.json`,
//! so a team shares them through git. Story points and sprint are saved as `storyPoints` and `sprint`, since their
//! real field ids differ between Jira sites.
use crate::client::{Jira, encode};
use crate::fields::{self, Fields};
use crate::{images, list};
use serde::{Deserialize, Serialize};
use taskproxy_plugin_sdk::ui::{self, Action, Choice, Field, FieldKind, Node};
use taskproxy_plugin_sdk::{ActionRequest, ActionResult, Error, Host, Map, Result, Value, json};

pub const DEFAULT_ID: &str = "default";
const PERSONAL_KEY: &str = "views";
const PROJECT_FILE: &str = "views.json";
const DRAFT_KEY: &str = "viewDraft";
pub const STORY_POINTS: &str = "storyPoints";
pub const SPRINT: &str = "sprint";
/// Jira hierarchies are rarely deeper than this, and each layer is one more request.
const MAX_LAYERS: usize = 6;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct View {
    pub id: String,
    pub name: String,
    /// The field ids for each layer: the issue first, then its parent, and so on.
    pub layers: Vec<Vec<String>>,
    /// The fields shown for each layer's subtasks: missing or `None` shows the fields Jira shows, and an empty list
    /// hides them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subtasks: Vec<Option<Vec<String>>>,
    /// The fields shown for each layer's linked work items, as `subtasks` is for subtasks.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Option<Vec<String>>>,
}

/// The fields Jira shows for subtasks, after the key and summary.
pub const SUBTASK_FIELDS: [&str; 4] = ["priority", STORY_POINTS, "assignee", "status"];
/// The fields Jira shows for linked work items, after the key and summary.
pub const LINK_FIELDS: [&str; 3] = ["status", "assignee", "priority"];

/// Work listed under each layer of a View, each kind with its own fields.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Related {
    Subtasks,
    Links,
}

impl Related {
    const ALL: [Related; 2] = [Related::Subtasks, Related::Links];

    /// Jira's own fields for this kind.
    pub fn defaults(self) -> Vec<String> {
        match self {
            Related::Subtasks => SUBTASK_FIELDS.map(String::from).to_vec(),
            Related::Links => LINK_FIELDS.map(String::from).to_vec(),
        }
    }

    /// The name in section keys and action params.
    fn id(self) -> &'static str {
        match self {
            Related::Subtasks => "subtasks",
            Related::Links => "links",
        }
    }

    fn title(self) -> &'static str {
        match self {
            Related::Subtasks => "Subtasks",
            Related::Links => "Linked work items",
        }
    }

    /// The kind an editor action is for: `sub` marks subtasks, and `related` names either.
    fn from_params(params: &serde_json::Map<String, Value>) -> Option<Related> {
        if params.get("sub").and_then(Value::as_bool) == Some(true) {
            return Some(Related::Subtasks);
        }
        match params.get("related").and_then(Value::as_str) {
            Some("subtasks") => Some(Related::Subtasks),
            Some("links") => Some(Related::Links),
            _ => None,
        }
    }
}

impl View {
    fn related(&self, kind: Related) -> &Vec<Option<Vec<String>>> {
        match kind {
            Related::Subtasks => &self.subtasks,
            Related::Links => &self.links,
        }
    }

    fn related_mut(&mut self, kind: Related) -> &mut Vec<Option<Vec<String>>> {
        match kind {
            Related::Subtasks => &mut self.subtasks,
            Related::Links => &mut self.links,
        }
    }

    /// The fields shown for a layer's subtasks or linked work items, or none when they are hidden.
    pub fn related_fields(&self, kind: Related, depth: usize) -> Vec<String> {
        match self.related(kind).get(depth).cloned().flatten() {
            Some(fields) => fields,
            None => kind.defaults(),
        }
    }

    /// The fields chosen for a layer's subtasks or linked work items, or `None` while they are Jira's.
    fn custom_related(&self, kind: Related, depth: usize) -> Option<&Vec<String>> {
        self.related(kind).get(depth).and_then(Option::as_ref)
    }

    fn set_related(&mut self, kind: Related, depth: usize, fields: Option<Vec<String>>) {
        let list = self.related_mut(kind);
        if list.len() <= depth {
            list.resize(depth + 1, None);
        }
        list[depth] = fields;
        while list.last().is_some_and(Option::is_none) {
            list.pop();
        }
    }

    #[cfg(test)]
    fn subtask_fields(&self, depth: usize) -> Vec<String> {
        self.related_fields(Related::Subtasks, depth)
    }

    #[cfg(test)]
    fn set_subtasks(&mut self, depth: usize, fields: Option<Vec<String>>) {
        self.set_related(Related::Subtasks, depth, fields);
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Scope {
    Builtin,
    Personal,
    Project,
}

impl Scope {
    fn label(self) -> &'static str {
        match self {
            Scope::Builtin => "Built in",
            Scope::Personal => "Personal",
            Scope::Project => "Project",
        }
    }
}

/// A View being edited, kept in storage between clicks.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Draft {
    /// The View it replaces, or `None` for a new one.
    original: Option<(Scope, String)>,
    view: View,
}

pub fn default_view() -> View {
    View {
        id: DEFAULT_ID.into(),
        name: "Default".into(),
        layers: vec![
            [
                "status",
                "assignee",
                "reporter",
                "priority",
                STORY_POINTS,
                "description",
                "comment",
            ]
            .map(String::from)
            .to_vec(),
            ["status", "assignee", "description"]
                .map(String::from)
                .to_vec(),
        ],
        subtasks: Vec::new(),
        links: Vec::new(),
    }
}

/// One of an issue's fields in a View. A field picked from a list changes when its value is clicked, and a text
/// field switches to its editor when its label is clicked.
fn property(jira: &Jira, fields: &Fields, issue: &Value, real: &str, label: &str) -> ui::Property {
    let value = fields.value_of(issue, real);
    let key = issue.get("key").and_then(Value::as_str).unwrap_or("");
    if list::is_choice(real, fields) {
        return ui::property(label, list::cell(fields, real, label, Some(issue)));
    }
    if list::is_editable_text(real, fields) {
        let shown = if list::is_rich_text(real, fields) {
            images::rich_text(jira, key, real, &value)
        } else {
            fields::value_node(real, &value)
        };
        let mut property = ui::property(label, shown);
        property.edit = Some(list::text_editor(fields, key, real, &value));
        return property;
    }
    if real == "comment" {
        return ui::property(label, images::comments(jira, key, &value));
    }
    ui::property(label, fields::value_node(real, &value))
}

pub fn layer_name(depth: usize) -> String {
    match depth {
        0 => "Issue".into(),
        1 => "Parent".into(),
        2 => "Grandparent".into(),
        3 => "Great-grandparent".into(),
        _ => format!("Level {} parent", depth),
    }
}

/// A View's field id as the site's own id.
pub fn resolve(id: &str, fields: &Fields) -> Option<String> {
    match id {
        STORY_POINTS => fields.story_points(),
        SPRINT => fields.sprint(),
        _ => Some(id.to_string()),
    }
}

/// The site's field id as a View saves it.
pub fn portable(id: &str, fields: &Fields) -> String {
    if fields.is_story_points(id) {
        STORY_POINTS.into()
    } else if fields.sprint().as_deref() == Some(id) {
        SPRINT.into()
    } else {
        id.to_string()
    }
}

pub fn field_label(id: &str, fields: &Fields) -> String {
    match id {
        STORY_POINTS => "Story points".into(),
        SPRINT => "Sprint".into(),
        _ => fields.name(id),
    }
}

fn read_personal(host: &dyn Host) -> Result<Vec<View>> {
    Ok(host
        .store_get(PERSONAL_KEY)?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

/// The project's Views, or `None` when no project is open.
fn read_project(host: &dyn Host) -> Option<Vec<View>> {
    match host.project_file_read(PROJECT_FILE) {
        Ok(Some(text)) => Some(serde_json::from_str(&text).unwrap_or_default()),
        Ok(None) => Some(Vec::new()),
        Err(_) => None,
    }
}

fn write(host: &dyn Host, scope: Scope, views: &[View]) -> Result<()> {
    match scope {
        Scope::Personal => host.store_set(PERSONAL_KEY, Some(serde_json::to_value(views)?)),
        Scope::Project => {
            host.project_file_write(PROJECT_FILE, &serde_json::to_string_pretty(views)?)
        }
        Scope::Builtin => Err(Error::new("The built-in View cannot be changed.")),
    }
}

/// Every View: the built-in one, then the project's, then personal ones.
pub fn all(host: &dyn Host) -> Result<Vec<(Scope, View)>> {
    let mut views = vec![(Scope::Builtin, default_view())];
    views.extend(
        read_project(host)
            .unwrap_or_default()
            .into_iter()
            .map(|view| (Scope::Project, view)),
    );
    views.extend(
        read_personal(host)?
            .into_iter()
            .map(|view| (Scope::Personal, view)),
    );
    Ok(views)
}

/// A View by `scope:id`, or the default.
pub fn find(host: &dyn Host, key: &str) -> Result<View> {
    Ok(all(host)?
        .into_iter()
        .find(|(scope, view)| view_key(*scope, &view.id) == key)
        .map(|(_, view)| view)
        .unwrap_or_else(default_view))
}

pub fn view_key(scope: Scope, id: &str) -> String {
    match scope {
        Scope::Builtin => id.to_string(),
        Scope::Personal => format!("personal:{}", id),
        Scope::Project => format!("project:{}", id),
    }
}

/// The issue and each parent, as a section per layer, following `parent` up as far as the View has layers.
/// Returns the issue's own fields too, for the page's heading and actions.
pub fn layers(jira: &Jira, fields: &Fields, view: &View, key: &str) -> Result<(Value, Vec<Node>)> {
    let mut sections = Vec::new();
    let mut first = Value::Null;
    let mut next = Some(key.to_string());
    for (depth, layer) in view.layers.iter().take(MAX_LAYERS).enumerate() {
        let Some(current) = next.take() else {
            break;
        };
        let mut wanted: Vec<String> = fields.wanted(
            &layer
                .iter()
                .filter_map(|id| resolve(id, fields))
                .collect::<Vec<_>>(),
        );
        for always in [
            "summary",
            "issuetype",
            "parent",
            "status",
            "subtasks",
            "issuelinks",
        ] {
            if !wanted.iter().any(|id| id == always) {
                wanted.push(always.to_string());
            }
        }
        let query: Vec<String> = wanted.iter().map(|id| encode(id)).collect();
        let issue = jira.get(&format!(
            "issue/{}?fields={}",
            encode(&current),
            query.join(",")
        ))?;
        let issue_fields = issue.get("fields").cloned().unwrap_or(Value::Null);
        let kind = issue_fields
            .pointer("/issuetype/name")
            .and_then(Value::as_str)
            .unwrap_or("Issue");
        let issue_key = issue
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or(&current)
            .to_string();
        fields.learn(jira, [issue_key.as_str()]);
        let summary = issue_fields
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let items: Vec<ui::Property> = layer
            .iter()
            .filter_map(|id| {
                let real = resolve(id, fields)?;
                Some(property(
                    jira,
                    fields,
                    &issue,
                    &real,
                    &field_label(id, fields),
                ))
            })
            .collect();
        let mut children = vec![Node::Properties { items }];
        if let Some(table) = subtasks(jira, fields, view, depth, &issue_key, &issue_fields)? {
            children.push(table);
        }
        if let Some(links) = linked(jira, fields, view, depth, &issue_fields)? {
            children.push(links);
        }
        if depth > 0 {
            children.push(ui::link(
                format!("Open {}", issue_key),
                Action::new("open", format!("Open {}", issue_key)).param("key", issue_key.clone()),
            ));
        }
        sections.push(Node::Section {
            key: format!("layer-{}", depth),
            title: format!("{} · {} {}", layer_name(depth), kind, issue_key),
            subtitle: Some(summary),
            open: true,
            children,
        });
        next = issue_fields
            .pointer("/parent/key")
            .and_then(Value::as_str)
            .map(String::from);
        if depth == 0 {
            first = issue;
        }
    }
    Ok((first, sections))
}

fn read_draft(host: &dyn Host) -> Result<Option<Draft>> {
    Ok(host
        .store_get(DRAFT_KEY)?
        .and_then(|value| serde_json::from_value(value).ok()))
}

fn save_draft(host: &dyn Host, draft: Option<&Draft>) -> Result<()> {
    host.store_set(DRAFT_KEY, draft.map(serde_json::to_value).transpose()?)
}

fn param_str<'a>(params: &'a Map<String, Value>, name: &str) -> &'a str {
    params.get(name).and_then(Value::as_str).unwrap_or("")
}

fn param_usize(params: &Map<String, Value>, name: &str) -> usize {
    params
        .get(name)
        .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
        .unwrap_or(0) as usize
}

fn scope_of(text: &str) -> Scope {
    match text {
        "project" => Scope::Project,
        "builtin" => Scope::Builtin,
        _ => Scope::Personal,
    }
}

fn scope_text(scope: Scope) -> &'static str {
    match scope {
        Scope::Builtin => "builtin",
        Scope::Personal => "personal",
        Scope::Project => "project",
    }
}

/// The Views page: the list of Views, or the editor while one is being edited.
pub fn page(host: &dyn Host, fields: &Fields, params: &Map<String, Value>) -> Result<Node> {
    if !param_str(params, "edit").is_empty()
        && let Some(draft) = read_draft(host)?
    {
        return Ok(editor(host, fields, &draft));
    }
    let mut rows = Vec::new();
    for (scope, view) in all(host)? {
        let layers = view
            .layers
            .iter()
            .enumerate()
            .map(|(depth, layer)| format!("{}: {}", layer_name(depth), layer.len()))
            .collect::<Vec<_>>()
            .join(", ");
        let mut actions = vec![ui::link(
            "Edit",
            Action::new("views.edit", "Edit")
                .param("scope", scope_text(scope))
                .param("id", view.id.clone()),
        )];
        if scope == Scope::Builtin {
            actions = vec![ui::link(
                "Copy",
                Action::new("views.edit", "Copy")
                    .param("scope", "builtin")
                    .param("id", view.id.clone()),
            )];
        } else {
            actions.push(ui::link(
                "Delete",
                Action::new("views.delete", "Delete")
                    .param("scope", scope_text(scope))
                    .param("id", view.id.clone())
                    .confirm(format!("Delete the View '{}'?", view.name)),
            ));
        }
        rows.push(ui::TableRow {
            cells: vec![
                ui::strong(view.name.clone()),
                ui::text(scope.label()),
                ui::muted(layers),
                ui::row(actions),
            ],
            action: None,
            theme: String::new(),
        });
    }
    Ok(ui::stack(vec![
        ui::markdown(
            "A View chooses the fields shown for an issue and for each parent above it. \
             Project Views are saved in this project's `.taskproxy` folder, so everyone on the project gets them.",
        ),
        ui::toolbar(vec![ui::button(
            Action::new("views.new", "New View").icon("plus"),
            "primary",
        )]),
        Node::Table {
            columns: vec![
                "View".into(),
                "Saved".into(),
                "Fields per layer".into(),
                "".into(),
            ],
            rows,
            angled: false,
        },
    ]))
}

/// A layer's subtasks, as Jira shows them: how many are done, then a table with the View's fields for them, which
/// change in place. `None` when the issue has no subtasks or the View hides them.
fn subtasks(
    jira: &Jira,
    fields: &Fields,
    view: &View,
    depth: usize,
    key: &str,
    issue_fields: &Value,
) -> Result<Option<Node>> {
    let listed = issue_fields
        .get("subtasks")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0);
    let chosen = view.related_fields(Related::Subtasks, depth);
    if listed == 0 || chosen.is_empty() {
        return Ok(None);
    }
    let (resolved, wanted) = related_fields(fields, &chosen, &["summary", "status", "resolution"]);
    let found = jira.post(
        "search/jql",
        &json!({ "jql": format!("parent = \"{}\" ORDER BY created ASC", key), "fields": wanted, "maxResults": 100 }),
    )?;
    let issues: Vec<Value> = found
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let done = issues
        .iter()
        .filter(|issue| list::is_finished(issue))
        .count();
    let total = issues.len();
    Ok(Some(Node::Section {
        key: format!("layer-{}-subtasks", depth),
        title: "Subtasks".into(),
        subtitle: Some(format!(
            "{} of {} done ({}%)",
            done,
            total,
            (done * 100).checked_div(total).unwrap_or(0)
        )),
        open: true,
        children: vec![work_table(fields, &resolved, issues.iter())],
    }))
}

/// The ids chosen for a list of related work, with the site's id for each, and the fields to ask Jira for.
fn related_fields(
    fields: &Fields,
    chosen: &[String],
    always: &[&str],
) -> (Vec<(String, String)>, Vec<String>) {
    let resolved: Vec<(String, String)> = chosen
        .iter()
        .filter_map(|id| Some((id.clone(), resolve(id, fields)?)))
        .collect();
    let mut wanted = fields.wanted(
        &resolved
            .iter()
            .map(|(_, real)| real.clone())
            .collect::<Vec<_>>(),
    );
    for always in always {
        if !wanted.iter().any(|id| id == always) {
            wanted.push((*always).into());
        }
    }
    (resolved, wanted)
}

/// A table of related work as Jira lists it: the key and summary, then the chosen fields, which change in place.
fn work_table<'a>(
    fields: &Fields,
    resolved: &[(String, String)],
    issues: impl Iterator<Item = &'a Value>,
) -> Node {
    let mut columns: Vec<ui::Column> = vec!["Work".into()];
    columns.extend(
        resolved
            .iter()
            .map(|(id, _)| field_label(id, fields).into()),
    );
    let rows =
        issues
            .map(|issue| {
                let key = issue.get("key").and_then(Value::as_str).unwrap_or("");
                let summary = issue
                    .pointer("/fields/summary")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let mut cells = vec![ui::row(vec![
                    ui::link(key, crate::pages::open_issue(key)),
                    ui::clip(summary, None),
                ])];
                cells.extend(resolved.iter().map(|(id, real)| {
                    list::cell(fields, real, &field_label(id, fields), Some(issue))
                }));
                ui::TableRow {
                    cells,
                    action: None,
                    theme: crate::fields::row_theme(issue, None),
                }
            })
            .collect();
    Node::Table {
        columns,
        rows,
        angled: false,
    }
}

/// A layer's linked work items, as Jira shows them: grouped by how they are linked, such as "is blocked by", each
/// group a table with the View's fields, which change in place. `None` when the issue has no links or the View
/// hides them.
fn linked(
    jira: &Jira,
    fields: &Fields,
    view: &View,
    depth: usize,
    issue_fields: &Value,
) -> Result<Option<Node>> {
    let links = issue_fields
        .get("issuelinks")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let chosen = view.related_fields(Related::Links, depth);
    if links.is_empty() || chosen.is_empty() {
        return Ok(None);
    }
    // Each link names the other issue on its inward or outward side, with words for that direction.
    let mut groups: Vec<(String, Vec<Value>)> = Vec::new();
    for link in &links {
        let (side, words) = if let Some(other) = link.get("inwardIssue") {
            (other, "/type/inward")
        } else if let Some(other) = link.get("outwardIssue") {
            (other, "/type/outward")
        } else {
            continue;
        };
        let relation = link
            .pointer(words)
            .and_then(Value::as_str)
            .unwrap_or("relates to")
            .to_string();
        match groups.iter_mut().find(|(name, _)| *name == relation) {
            Some((_, items)) => items.push(side.clone()),
            None => groups.push((relation, vec![side.clone()])),
        }
    }
    if groups.is_empty() {
        return Ok(None);
    }
    // Links carry only a few fields, so the chosen ones are read for every linked issue at once.
    let (resolved, wanted) = related_fields(fields, &chosen, &["summary", "status"]);
    let keys: Vec<String> = groups
        .iter()
        .flat_map(|(_, items)| items.iter())
        .filter_map(|item| item.get("key").and_then(Value::as_str))
        .map(|key| format!("\"{}\"", key))
        .collect();
    let found = jira.post(
        "search/jql",
        &json!({ "jql": format!("key in ({})", keys.join(", ")), "fields": wanted, "maxResults": 100 }),
    )?;
    let issues: Vec<Value> = found
        .get("issues")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let full = |item: &Value| -> Value {
        let key = item.get("key").and_then(Value::as_str);
        issues
            .iter()
            .find(|issue| issue.get("key").and_then(Value::as_str) == key)
            // An issue the search leaves out, such as one the user cannot see, shows what the link carries.
            .cloned()
            .unwrap_or_else(|| item.clone())
    };
    let mut children = Vec::new();
    for (relation, items) in &groups {
        children.push(ui::heading(relation.clone(), 4));
        let rows: Vec<Value> = items.iter().map(full).collect();
        children.push(work_table(fields, &resolved, rows.iter()));
    }
    let total: usize = groups.iter().map(|(_, items)| items.len()).sum();
    Ok(Some(Node::Section {
        key: format!("layer-{}-links", depth),
        title: Related::Links.title().into(),
        subtitle: Some(if total == 1 {
            "1 item".into()
        } else {
            format!("{} items", total)
        }),
        open: true,
        children,
    }))
}

/// The field list an editor action changes: a layer's own fields, or its subtasks' or linked work items', which
/// start as Jira's.
fn edited_list(draft: &mut Draft, depth: usize, kind: Option<Related>) -> Option<&mut Vec<String>> {
    if depth >= draft.view.layers.len() {
        return None;
    }
    let Some(kind) = kind else {
        return draft.view.layers.get_mut(depth);
    };
    if draft.view.custom_related(kind, depth).is_none() {
        let fields = draft.view.related_fields(kind, depth);
        draft.view.set_related(kind, depth, Some(fields));
    }
    draft.view.related_mut(kind).get_mut(depth)?.as_mut()
}

/// The editor's list of fields, with buttons to order and remove them, and a form to add one.
fn field_list(
    choices: &[Choice],
    fields: &Fields,
    depth: usize,
    list: &[String],
    kind: Option<Related>,
    empty: &str,
) -> Vec<Node> {
    let mark = |action: Action| match kind {
        Some(kind) => action.param("related", kind.id()),
        None => action,
    };
    let mut items: Vec<Node> = Vec::new();
    for (index, id) in list.iter().enumerate() {
        let at = |action: &str, label: &str| {
            mark(
                Action::new(action, label)
                    .param("layer", depth)
                    .param("index", index),
            )
        };
        let mut line = vec![ui::text(field_label(id, fields))];
        if index > 0 {
            line.push(ui::link("Up", at("draft.move", "Move up").param("by", -1)));
        }
        if index + 1 < list.len() {
            line.push(ui::link(
                "Down",
                at("draft.move", "Move down").param("by", 1),
            ));
        }
        line.push(ui::link("Remove", at("draft.remove", "Remove")));
        items.push(ui::row(line));
    }
    if items.is_empty() {
        items.push(ui::empty(empty));
    }
    let available: Vec<Choice> = choices
        .iter()
        .filter(|choice| !list.contains(&choice.value))
        .cloned()
        .collect();
    let add = Action::new("draft.add", "Add").param("layer", depth);
    items.push(Node::Form {
        fields: vec![Field::new(FieldKind::Select, "field", "Add a field").options(available)],
        submit: mark(add),
        cancel: None,
        buttons: Vec::new(),
        children: Vec::new(),
        inline: false,
        changed: None,
    });
    items
}

/// The editor for a layer's subtasks or linked work items: Jira's fields until others are chosen, or hidden.
fn related_editor(
    choices: &[Choice],
    fields: &Fields,
    draft: &Draft,
    depth: usize,
    kind: Related,
) -> Node {
    let (one, many) = match kind {
        Related::Subtasks => ("subtask", "Subtasks"),
        Related::Links => ("linked work item", "Linked work items"),
    };
    let chosen = draft.view.related_fields(kind, depth);
    let custom = draft.view.custom_related(kind, depth).is_some();
    let mut items = if chosen.is_empty() {
        vec![ui::muted(format!("{} are hidden for this layer.", many))]
    } else {
        let mut note = vec![ui::muted(if custom {
            format!("Shown after each {}'s key and summary.", one)
        } else {
            format!(
                "The fields Jira shows, after each {}'s key and summary. Change them to choose your own.",
                one
            )
        })];
        note.extend(field_list(
            choices,
            fields,
            depth,
            &chosen,
            Some(kind),
            "No fields.",
        ));
        note
    };
    let at = |action: &str, label: &str| {
        Action::new(action, label)
            .param("layer", depth)
            .param("related", kind.id())
    };
    let lower = many.to_lowercase();
    let mut links = Vec::new();
    if custom && !chosen.is_empty() {
        links.push(ui::link(
            "Use Jira's fields",
            at("draft.relatedReset", "Use Jira's fields"),
        ));
    }
    links.push(if chosen.is_empty() {
        ui::link(
            format!("Show {}", lower),
            at("draft.relatedReset", &format!("Show {}", lower)),
        )
    } else {
        ui::link(
            format!("Hide {}", lower),
            at("draft.relatedHide", &format!("Hide {}", lower)),
        )
    });
    items.push(ui::row(links));
    ui::section(
        format!("edit-layer-{}-{}", depth, kind.id()),
        kind.title(),
        false,
        items,
    )
}

fn editor(host: &dyn Host, fields: &Fields, draft: &Draft) -> Node {
    let choices: Vec<Choice> = fields
        .choices()
        .into_iter()
        .map(|field| {
            let id = portable(&field.id, fields);
            let label = field_label(&id, fields);
            Choice::new(id, label)
        })
        .collect();
    let mut children = vec![ui::heading(
        if draft.original.is_some() {
            "Edit View"
        } else {
            "New View"
        },
        2,
    )];
    for (depth, layer) in draft.view.layers.iter().enumerate() {
        let mut items = field_list(&choices, fields, depth, layer, None, "No fields yet.");
        for kind in Related::ALL {
            items.push(related_editor(&choices, fields, draft, depth, kind));
        }
        if depth > 0 {
            items.push(ui::link(
                format!("Remove the {} layer", layer_name(depth).to_lowercase()),
                Action::new("draft.removeLayer", "Remove layer").param("layer", depth),
            ));
        }
        children.push(ui::section(
            format!("edit-layer-{}", depth),
            layer_name(depth),
            true,
            items,
        ));
    }
    if draft.view.layers.len() < MAX_LAYERS {
        children.push(ui::toolbar(vec![ui::button(
            Action::new(
                "draft.addLayer",
                format!(
                    "Add the {} layer",
                    layer_name(draft.view.layers.len()).to_lowercase()
                ),
            )
            .icon("plus"),
            "",
        )]));
    }
    let mut scopes = vec![Choice::new("personal", "Personal: only you")];
    if read_project(host).is_some() {
        scopes.push(Choice::new(
            "project",
            "Project: everyone on this project, through git",
        ));
    }
    let scope = match &draft.original {
        Some((Scope::Project, _)) => "project",
        _ => "personal",
    };
    children.push(Node::Form {
        fields: vec![
            Field::new(FieldKind::Text, "name", "Name")
                .value(draft.view.name.clone())
                .required(),
            Field::new(FieldKind::Select, "scope", "Save for")
                .options(scopes)
                .value(scope),
        ],
        submit: Action::new("draft.save", "Save View"),
        cancel: Some(Action::new("draft.cancel", "Cancel")),
        buttons: Vec::new(),
        children: Vec::new(),
        inline: false,
        changed: None,
    });
    ui::stack(children)
}

fn new_id(host: &dyn Host, name: &str) -> Result<String> {
    let slug: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    Ok(format!(
        "{}-{:x}",
        if slug.is_empty() { "view" } else { &slug },
        host.now_ms()? % 0xffffff
    ))
}

fn edit_page() -> ActionResult {
    let mut params = Map::new();
    params.insert("edit".into(), json!("1"));
    ActionResult::navigate("views", params)
}

/// Answers the Views page's actions, or `None` when the action is not one of them.
pub fn action(
    host: &dyn Host,
    fields: &Fields,
    request: &ActionRequest,
) -> Result<Option<ActionResult>> {
    let params = &request.params;
    let draft = || -> Result<Draft> {
        read_draft(host)?
            .ok_or_else(|| Error::new("The View being edited was lost. Start again from Views."))
    };
    let result = match request.action.as_str() {
        "views.new" => {
            let mut view = default_view();
            view.id.clear();
            view.name = String::new();
            save_draft(
                host,
                Some(&Draft {
                    original: None,
                    view,
                }),
            )?;
            edit_page()
        }
        "views.edit" => {
            let scope = scope_of(param_str(params, "scope"));
            let id = param_str(params, "id");
            let view = all(host)?
                .into_iter()
                .find(|(found_scope, view)| *found_scope == scope && view.id == id)
                .map(|(_, view)| view)
                .ok_or_else(|| Error::new("That View no longer exists."))?;
            let original = (scope != Scope::Builtin).then(|| (scope, view.id.clone()));
            let mut view = view;
            if scope == Scope::Builtin {
                view.id.clear();
                view.name = format!("{} copy", view.name);
            }
            save_draft(host, Some(&Draft { original, view }))?;
            edit_page()
        }
        "views.delete" => {
            let scope = scope_of(param_str(params, "scope"));
            let id = param_str(params, "id");
            let mut views = match scope {
                Scope::Project => read_project(host).unwrap_or_default(),
                _ => read_personal(host)?,
            };
            views.retain(|view| view.id != id);
            write(host, scope, &views)?;
            ActionResult::refresh().with_message("View deleted.", "success")
        }
        "draft.add" => {
            let mut draft = draft()?;
            let field = request
                .form
                .get("field")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let kind = Related::from_params(params);
            if field.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Choose a field to add.", "warning"),
                ));
            }
            let layer = edited_list(&mut draft, param_usize(params, "layer"), kind)
                .ok_or_else(|| Error::new("That layer no longer exists."))?;
            if !layer.contains(&field) {
                layer.push(field);
            }
            save_draft(host, Some(&draft))?;
            ActionResult::ui(editor(host, fields, &draft))
        }
        "draft.move" | "draft.remove" => {
            let mut draft = draft()?;
            let index = param_usize(params, "index");
            let kind = Related::from_params(params);
            if let Some(layer) = edited_list(&mut draft, param_usize(params, "layer"), kind)
                && index < layer.len()
            {
                if request.action == "draft.remove" {
                    layer.remove(index);
                } else {
                    let by = params.get("by").and_then(Value::as_i64).unwrap_or(0);
                    let target = index as i64 + by;
                    if (0..layer.len() as i64).contains(&target) {
                        layer.swap(index, target as usize);
                    }
                }
            }
            save_draft(host, Some(&draft))?;
            ActionResult::ui(editor(host, fields, &draft))
        }
        "draft.addLayer" => {
            let mut draft = draft()?;
            if draft.view.layers.len() < MAX_LAYERS {
                draft
                    .view
                    .layers
                    .push(vec!["status".into(), "description".into()]);
            }
            save_draft(host, Some(&draft))?;
            ActionResult::ui(editor(host, fields, &draft))
        }
        "draft.removeLayer" => {
            let mut draft = draft()?;
            let layer = param_usize(params, "layer");
            if layer > 0 && layer < draft.view.layers.len() {
                draft.view.layers.truncate(layer);
                draft.view.subtasks.truncate(layer);
                draft.view.links.truncate(layer);
            }
            save_draft(host, Some(&draft))?;
            ActionResult::ui(editor(host, fields, &draft))
        }
        // The subtasks names come from before linked work items, when they were the only kind.
        "draft.subtasksHide"
        | "draft.subtasksReset"
        | "draft.relatedHide"
        | "draft.relatedReset" => {
            let mut draft = draft()?;
            let layer = param_usize(params, "layer");
            let kind = Related::from_params(params).unwrap_or(Related::Subtasks);
            if layer < draft.view.layers.len() {
                let hidden = request.action.ends_with("Hide");
                draft.view.set_related(kind, layer, hidden.then(Vec::new));
            }
            save_draft(host, Some(&draft))?;
            ActionResult::ui(editor(host, fields, &draft))
        }
        "draft.save" => {
            let mut draft = draft()?;
            let name = request
                .form
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string();
            if name.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Give the View a name.", "warning"),
                ));
            }
            let scope = scope_of(
                request
                    .form
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("personal"),
            );
            if scope == Scope::Project && read_project(host).is_none() {
                return Err(Error::new("Open a project to save a View for it."));
            }
            draft.view.name = name;
            if let Some((old_scope, old_id)) = &draft.original {
                draft.view.id = old_id.clone();
                if *old_scope != scope {
                    let mut old = match old_scope {
                        Scope::Project => read_project(host).unwrap_or_default(),
                        _ => read_personal(host)?,
                    };
                    old.retain(|view| view.id != *old_id);
                    write(host, *old_scope, &old)?;
                }
            }
            if draft.view.id.is_empty() {
                draft.view.id = new_id(host, &draft.view.name)?;
            }
            let mut views = match scope {
                Scope::Project => read_project(host).unwrap_or_default(),
                _ => read_personal(host)?,
            };
            match views.iter_mut().find(|view| view.id == draft.view.id) {
                Some(existing) => *existing = draft.view.clone(),
                None => views.push(draft.view.clone()),
            }
            write(host, scope, &views)?;
            save_draft(host, None)?;
            ActionResult::navigate("views", Map::new()).with_message("View saved.", "success")
        }
        "draft.cancel" => {
            save_draft(host, None)?;
            ActionResult::navigate("views", Map::new())
        }
        _ => return Ok(None),
    };
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{FakeHost, sample_fields};

    fn fields() -> Fields {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        Fields::load(&Jira::connect(&host).unwrap().unwrap()).unwrap()
    }

    fn act(host: &FakeHost, action: &str, params: Value, form: Value) -> ActionResult {
        let request = ActionRequest {
            page: "views".into(),
            action: action.into(),
            params: params.as_object().cloned().unwrap_or_default(),
            form: form.as_object().cloned().unwrap_or_default(),
            ..Default::default()
        };
        super::action(host, &fields(), &request).unwrap().unwrap()
    }

    #[test]
    fn layers_follow_parents_and_show_the_chosen_fields() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.answer(
            "GET",
            "/rest/api/3/issue/P-3?fields=status,customfield_10016,customfield_10026,summary,issuetype,parent,subtasks,issuelinks",
            200,
            json!({ "key": "P-3", "fields": {
                "summary": "Fix login", "issuetype": { "name": "Task" },
                "status": { "name": "To Do", "statusCategory": { "key": "new" } },
                "customfield_10016": 3, "parent": { "key": "P-2" }
            } }),
        );
        host.answer(
            "GET",
            "/rest/api/3/issue/P-2?fields=description,summary,issuetype,parent,status,subtasks,issuelinks",
            200,
            json!({ "key": "P-2", "fields": {
                "summary": "Sign in", "issuetype": { "name": "Story" }, "parent": { "key": "P-1" },
                "description": { "type": "doc", "content": [{ "type": "paragraph", "content": [{ "type": "text", "text": "As a user" }] }] }
            } }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let view = View {
            id: "v".into(),
            name: "V".into(),
            layers: vec![
                vec!["status".into(), STORY_POINTS.into()],
                vec!["description".into()],
            ],
            subtasks: Vec::new(),
            links: Vec::new(),
        };
        let (issue, sections) = layers(&jira, &fields, &view, "P-3").unwrap();
        assert_eq!(issue["key"], "P-3");
        assert_eq!(
            sections.len(),
            2,
            "the View has two layers, so P-1 is not read"
        );
        let Node::Section {
            title,
            subtitle,
            children,
            ..
        } = &sections[0]
        else {
            panic!()
        };
        assert_eq!(title, "Issue · Task P-3");
        assert_eq!(subtitle.as_deref(), Some("Fix login"));
        // Fields picked from a list change in place when their value is clicked.
        let Node::Properties { items } = &children[0] else {
            panic!()
        };
        assert_eq!(items[0].label, "Status");
        assert!(matches!(&items[0].value, Node::Select { text, .. } if text == "To Do"));
        assert!(
            matches!(&items[1].value, Node::Select { text, options, .. } if text == "3" && options.id == "cell.options")
        );
        assert!(items.iter().all(|item| item.edit.is_none()));
        let Node::Section {
            title, children, ..
        } = &sections[1]
        else {
            panic!()
        };
        assert_eq!(title, "Parent · Story P-2");
        // A text field shows its value, and clicking its label switches to an editor that saves it.
        let Node::Properties { items } = &children[0] else {
            panic!()
        };
        assert_eq!(items[0].value, ui::markdown("As a user"));
        let Some(Node::Form { fields, submit, .. }) = &items[0].edit else {
            panic!("the description has an editor")
        };
        assert_eq!(fields[0].value, json!("As a user"));
        assert_eq!(submit.id, "cell.save");
        assert_eq!(submit.params["key"], "P-2");
    }

    #[test]
    fn subtasks_show_with_jiras_fields_or_the_views_own_or_not_at_all() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.answer("GET", "/rest/api/3/issue/IR-1", 200, json!({ "key": "IR-1", "fields": {
            "summary": "Story", "issuetype": { "name": "Story" }, "status": { "name": "Code Review" },
            "subtasks": [{ "key": "IR-2" }, { "key": "IR-3" }, { "key": "IR-4" }]
        } }));
        let done = json!({ "name": "Done", "statusCategory": { "key": "done" } });
        host.answer_when("POST", "/rest/api/3/search/jql", "parent = ", 200, json!({ "isLast": true, "issues": [
            { "key": "IR-2", "fields": { "summary": "Build Requirements", "status": done, "priority": { "name": "Highest" } } },
            { "key": "IR-3", "fields": { "summary": "Development", "status": done, "assignee": { "displayName": "Erik" } } },
            { "key": "IR-4", "fields": { "summary": "Test Execution", "status": { "name": "To Do", "statusCategory": { "key": "new" } }, "customfield_10026": 3 } }
        ] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let mut view = View {
            id: "v".into(),
            name: "V".into(),
            layers: vec![vec!["status".into()]],
            subtasks: Vec::new(),
            links: Vec::new(),
        };
        let table = |view: &View| -> Option<Value> {
            let (_, sections) = layers(&jira, &fields, view, "IR-1").unwrap();
            let section = serde_json::to_value(&sections[0]).unwrap();
            section["children"]
                .as_array()
                .unwrap()
                .iter()
                .find(|child| child["title"] == "Subtasks")
                .cloned()
        };
        // Jira's own fields: the work, then priority, story points, assignee, and status.
        let jira_like = table(&view).unwrap();
        assert_eq!(jira_like["subtitle"], "2 of 3 done (66%)");
        let grid = &jira_like["children"][0];
        assert_eq!(
            grid["columns"],
            json!(["Work", "Priority", "Story points", "Assignee", "Status"])
        );
        assert_eq!(grid["rows"][0]["cells"][0]["children"][0]["text"], "IR-2");
        assert_eq!(
            grid["rows"][0]["cells"][0]["children"][1]["text"],
            "Build Requirements"
        );
        // Story points come from whichever field holds them, and change in place.
        assert_eq!(grid["rows"][2]["cells"][2]["type"], "select");
        assert_eq!(grid["rows"][2]["cells"][2]["text"], "3");
        // The View's own fields for this layer.
        view.set_subtasks(0, Some(vec!["status".into()]));
        let own = table(&view).unwrap();
        assert_eq!(own["children"][0]["columns"], json!(["Work", "Status"]));
        // Hidden.
        view.set_subtasks(0, Some(Vec::new()));
        assert!(table(&view).is_none());
        // Jira's fields again, which saves nothing for the layer.
        view.set_subtasks(0, None);
        assert!(view.subtasks.is_empty());
        let saved = serde_json::to_value(&view).unwrap();
        assert!(
            saved.get("subtasks").is_none(),
            "older Views read back unchanged"
        );
    }

    #[test]
    fn linked_work_items_show_by_how_they_are_linked_with_jiras_fields_or_the_views_own() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        let blocks = json!({ "name": "Blocks", "inward": "is blocked by", "outward": "blocks" });
        host.answer("GET", "/rest/api/3/issue/IR-1", 200, json!({ "key": "IR-1", "fields": {
            "summary": "Story", "issuetype": { "name": "Story" }, "status": { "name": "Backlog" },
            "issuelinks": [
                { "type": blocks, "inwardIssue": { "key": "IR-9604", "fields": { "summary": "Quick Links" } } },
                { "type": blocks, "outwardIssue": { "key": "IR-7", "fields": { "summary": "Release", "status": { "name": "To Do" } } } },
                { "type": blocks, "inwardIssue": { "key": "IR-11612", "fields": { "summary": "Provider Role" } } }
            ]
        } }));
        // IR-7 is not found, as for an issue the user cannot see, so it shows what the link carries.
        host.answer_when("POST", "/rest/api/3/search/jql", "key in", 200, json!({ "isLast": true, "issues": [
            { "key": "IR-9604", "fields": { "summary": "Create Client Configurable Quick Links", "status": { "name": "Backlog" }, "assignee": { "displayName": "Erik" }, "priority": { "name": "Medium" } } },
            { "key": "IR-11612", "fields": { "summary": "Create Provider Role for Quick Links", "status": { "name": "Backlog" } } }
        ] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let mut view = View {
            id: "v".into(),
            name: "V".into(),
            layers: vec![vec!["status".into()]],
            subtasks: Vec::new(),
            links: Vec::new(),
        };
        let section = |view: &View| -> Option<Value> {
            let (_, sections) = layers(&jira, &fields, view, "IR-1").unwrap();
            let section = serde_json::to_value(&sections[0]).unwrap();
            section["children"]
                .as_array()
                .unwrap()
                .iter()
                .find(|child| child["title"] == "Linked work items")
                .cloned()
        };
        let linked = section(&view).unwrap();
        assert_eq!(linked["subtitle"], "3 items");
        let children = linked["children"].as_array().unwrap();
        assert_eq!(children[0]["text"], "is blocked by");
        assert_eq!(
            children[1]["columns"],
            json!(["Work", "Status", "Assignee", "Priority"])
        );
        let rows = children[1]["rows"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["cells"][0]["children"][0]["text"], "IR-9604");
        assert_eq!(
            rows[0]["cells"][0]["children"][1]["text"],
            "Create Client Configurable Quick Links"
        );
        assert_eq!(rows[1]["cells"][0]["children"][0]["text"], "IR-11612");
        assert_eq!(children[2]["text"], "blocks");
        assert_eq!(
            children[3]["rows"][0]["cells"][0]["children"][1]["text"],
            "Release"
        );
        // One search finds every linked issue.
        let searches: Vec<String> = host
            .sent
            .borrow()
            .iter()
            .filter_map(|sent| sent.body.clone())
            .filter(|body| body.contains("key in"))
            .collect();
        assert_eq!(searches.len(), 1);
        assert!(searches[0].contains(r#"key in (\"IR-9604\", \"IR-11612\", \"IR-7\")"#));
        view.set_related(Related::Links, 0, Some(vec!["assignee".into()]));
        assert_eq!(
            section(&view).unwrap()["children"][1]["columns"],
            json!(["Work", "Assignee"])
        );
        view.set_related(Related::Links, 0, Some(Vec::new()));
        assert!(section(&view).is_none());
    }

    #[test]
    fn the_editor_chooses_each_layers_linked_work_item_fields() {
        let host = FakeHost::connected();
        act(&host, "views.new", json!({}), json!({}));
        let shown = act(
            &host,
            "draft.add",
            json!({ "layer": 0, "related": "links" }),
            json!({ "field": "summary" }),
        );
        assert!(
            serde_json::to_string(&shown.ui)
                .unwrap()
                .contains("edit-layer-0-links")
        );
        let draft = read_draft(&host).unwrap().unwrap();
        let mut expected: Vec<String> = LINK_FIELDS.map(String::from).to_vec();
        expected.push("summary".into());
        assert_eq!(draft.view.related_fields(Related::Links, 0), expected);
        assert_eq!(
            draft.view.subtask_fields(0),
            SUBTASK_FIELDS.map(String::from).to_vec(),
            "subtasks are unchanged"
        );
        let hidden = act(
            &host,
            "draft.relatedHide",
            json!({ "layer": 0, "related": "links" }),
            json!({}),
        );
        assert!(
            serde_json::to_string(&hidden.ui)
                .unwrap()
                .contains("Linked work items are hidden")
        );
        act(
            &host,
            "draft.relatedReset",
            json!({ "layer": 0, "related": "links" }),
            json!({}),
        );
        assert_eq!(
            read_draft(&host)
                .unwrap()
                .unwrap()
                .view
                .related_fields(Related::Links, 0),
            LINK_FIELDS.map(String::from).to_vec()
        );
    }

    #[test]
    fn the_editor_chooses_each_layers_subtask_fields() {
        let host = FakeHost::connected();
        act(&host, "views.new", json!({}), json!({}));
        // Changing Jira's fields starts from them.
        act(
            &host,
            "draft.add",
            json!({ "layer": 0, "sub": true }),
            json!({ "field": "summary" }),
        );
        let draft = read_draft(&host).unwrap().unwrap();
        let mut expected: Vec<String> = SUBTASK_FIELDS.map(String::from).to_vec();
        expected.push("summary".into());
        assert_eq!(draft.view.subtask_fields(0), expected);
        assert!(
            !draft.view.layers[0].contains(&"summary".to_string()),
            "the layer's own fields are unchanged"
        );
        act(
            &host,
            "draft.remove",
            json!({ "layer": 0, "index": 0, "sub": true }),
            json!({}),
        );
        assert_eq!(
            read_draft(&host).unwrap().unwrap().view.subtask_fields(0)[0],
            STORY_POINTS
        );
        act(
            &host,
            "draft.subtasksHide",
            json!({ "layer": 0 }),
            json!({}),
        );
        let hidden = act(
            &host,
            "draft.subtasksHide",
            json!({ "layer": 0 }),
            json!({}),
        );
        assert!(
            serde_json::to_string(&hidden.ui)
                .unwrap()
                .contains("Subtasks are hidden")
        );
        assert!(
            read_draft(&host)
                .unwrap()
                .unwrap()
                .view
                .subtask_fields(0)
                .is_empty()
        );
        act(
            &host,
            "draft.subtasksReset",
            json!({ "layer": 0 }),
            json!({}),
        );
        assert_eq!(
            read_draft(&host).unwrap().unwrap().view.subtask_fields(0),
            SUBTASK_FIELDS.map(String::from).to_vec()
        );
    }

    #[test]
    fn an_issue_without_a_parent_shows_only_its_own_layer() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1?fields=status,assignee,description,summary,issuetype,parent,subtasks,issuelinks",
            200,
            json!({ "key": "P-1", "fields": { "summary": "Epic", "issuetype": { "name": "Epic" } } }),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let fields = Fields::load(&jira).unwrap();
        let mut view = default_view();
        view.layers = vec![
            vec!["status".into(), "assignee".into(), "description".into()],
            vec!["summary".into()],
        ];
        let (_, sections) = layers(&jira, &fields, &view, "P-1").unwrap();
        assert_eq!(sections.len(), 1);
    }

    #[test]
    fn a_view_is_built_saved_and_found_again() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.project_open.set(true);
        act(&host, "views.new", json!({}), json!({}));
        act(
            &host,
            "draft.add",
            json!({ "layer": 0 }),
            json!({ "field": "customfield_10050" }),
        );
        act(
            &host,
            "draft.move",
            json!({ "layer": 0, "index": 7, "by": -1 }),
            json!({}),
        );
        act(
            &host,
            "draft.remove",
            json!({ "layer": 0, "index": 0 }),
            json!({}),
        );
        act(&host, "draft.addLayer", json!({}), json!({}));
        let warned = act(&host, "draft.save", json!({}), json!({ "name": " " }));
        assert_eq!(warned.message.unwrap().theme, "warning");
        let saved = act(
            &host,
            "draft.save",
            json!({}),
            json!({ "name": "Team view", "scope": "project" }),
        );
        assert_eq!(saved.navigate.unwrap().page, "views");
        let stored: Vec<View> = serde_json::from_str(&host.files.borrow()["views.json"]).unwrap();
        assert_eq!(stored.len(), 1);
        assert_eq!(stored[0].name, "Team view");
        assert_eq!(
            stored[0].layers,
            vec![
                vec![
                    "assignee",
                    "reporter",
                    "priority",
                    STORY_POINTS,
                    "description",
                    "customfield_10050",
                    "comment"
                ]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>(),
                vec![
                    "status".to_string(),
                    "assignee".into(),
                    "description".into()
                ],
                vec!["status".to_string(), "description".into()],
            ]
        );
        let key = view_key(Scope::Project, &stored[0].id);
        assert_eq!(find(&host, &key).unwrap().name, "Team view");
        assert_eq!(find(&host, "personal:missing").unwrap().id, DEFAULT_ID);
        // Moving it to personal removes the project copy.
        act(
            &host,
            "views.edit",
            json!({ "scope": "project", "id": stored[0].id }),
            json!({}),
        );
        act(
            &host,
            "draft.save",
            json!({}),
            json!({ "name": "Mine now", "scope": "personal" }),
        );
        assert_eq!(host.files.borrow()["views.json"].trim(), "[]");
        assert_eq!(read_personal(&host).unwrap()[0].name, "Mine now");
    }

    #[test]
    fn the_builtin_view_is_copied_rather_than_changed() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        act(
            &host,
            "views.edit",
            json!({ "scope": "builtin", "id": DEFAULT_ID }),
            json!({}),
        );
        let page = page(
            &host,
            &fields(),
            &json!({ "edit": "1" }).as_object().unwrap().clone(),
        )
        .unwrap();
        let text = serde_json::to_string(&page).unwrap();
        assert!(text.contains("Default copy"), "{}", text);
        assert!(
            !text.contains("Project: everyone"),
            "no project is open, so only personal is offered"
        );
        act(
            &host,
            "draft.save",
            json!({}),
            json!({ "name": "Default copy" }),
        );
        assert_eq!(all(&host).unwrap().len(), 2);
        assert_eq!(all(&host).unwrap()[0].1, default_view());
    }
}
