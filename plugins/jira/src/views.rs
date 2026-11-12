//! Views: which fields to show for an issue, its parent, the parent's parent, and so on. Each level is a layer,
//! drawn as its own collapsible section.
//!
//! Personal Views are kept in the plugin's storage. Project Views are kept in `.taskproxy/plugins/jira/views.json`,
//! so a team shares them through git. Story points and sprint are saved as `storyPoints` and `sprint`, since their
//! real field ids differ between Jira sites.
use crate::client::{Jira, encode};
use crate::fields::{self, Fields};
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
    }
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
fn portable(id: &str, fields: &Fields) -> String {
    if fields.story_points().as_deref() == Some(id) {
        STORY_POINTS.into()
    } else if fields.sprint().as_deref() == Some(id) {
        SPRINT.into()
    } else {
        id.to_string()
    }
}

fn field_label(id: &str, fields: &Fields) -> String {
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
        let mut wanted: Vec<String> = layer.iter().filter_map(|id| resolve(id, fields)).collect();
        for always in ["summary", "issuetype", "parent", "status"] {
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
        let summary = issue_fields
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let items: Vec<ui::Property> = layer
            .iter()
            .filter_map(|id| {
                let real = resolve(id, fields)?;
                Some(ui::property(
                    field_label(id, fields),
                    fields::value_node(&real, issue_fields.get(&real).unwrap_or(&Value::Null)),
                ))
            })
            .collect();
        let mut children = vec![Node::Properties { items }];
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
        },
    ]))
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
        let mut items: Vec<Node> = Vec::new();
        for (index, id) in layer.iter().enumerate() {
            let at = |action: &str, label: &str| {
                Action::new(action, label)
                    .param("layer", depth)
                    .param("index", index)
            };
            let mut controls = Vec::new();
            if index > 0 {
                controls.push(ui::link("Up", at("draft.move", "Move up").param("by", -1)));
            }
            if index + 1 < layer.len() {
                controls.push(ui::link(
                    "Down",
                    at("draft.move", "Move down").param("by", 1),
                ));
            }
            controls.push(ui::link("Remove", at("draft.remove", "Remove")));
            let mut line = vec![ui::text(field_label(id, fields))];
            line.extend(controls);
            items.push(ui::row(line));
        }
        if items.is_empty() {
            items.push(ui::empty("No fields yet."));
        }
        let available: Vec<Choice> = choices
            .iter()
            .filter(|choice| !layer.contains(&choice.value))
            .cloned()
            .collect();
        items.push(Node::Form {
            fields: vec![Field::new(FieldKind::Select, "field", "Add a field").options(available)],
            submit: Action::new("draft.add", "Add").param("layer", depth),
            cancel: None,
        });
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
            let layer = draft
                .view
                .layers
                .get_mut(param_usize(params, "layer"))
                .ok_or_else(|| Error::new("That layer no longer exists."))?;
            if field.is_empty() {
                return Ok(Some(
                    ActionResult::default().with_message("Choose a field to add.", "warning"),
                ));
            }
            if !layer.contains(&field) {
                layer.push(field);
            }
            save_draft(host, Some(&draft))?;
            ActionResult::ui(editor(host, fields, &draft))
        }
        "draft.move" | "draft.remove" => {
            let mut draft = draft()?;
            let index = param_usize(params, "index");
            if let Some(layer) = draft.view.layers.get_mut(param_usize(params, "layer"))
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
            "/rest/api/3/issue/P-3?fields=status,customfield_10016,summary,issuetype,parent",
            200,
            json!({ "key": "P-3", "fields": {
                "summary": "Fix login", "issuetype": { "name": "Task" },
                "status": { "name": "To Do", "statusCategory": { "key": "new" } },
                "customfield_10016": 3, "parent": { "key": "P-2" }
            } }),
        );
        host.answer(
            "GET",
            "/rest/api/3/issue/P-2?fields=description,summary,issuetype,parent,status",
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
        assert_eq!(
            children[0],
            Node::Properties {
                items: vec![
                    ui::property("Status", ui::badge("To Do", "")),
                    ui::property("Story points", ui::text("3")),
                ]
            }
        );
        let Node::Section {
            title, children, ..
        } = &sections[1]
        else {
            panic!()
        };
        assert_eq!(title, "Parent · Story P-2");
        assert_eq!(
            children[0],
            Node::Properties {
                items: vec![ui::property("Description", ui::markdown("As a user"))]
            }
        );
    }

    #[test]
    fn an_issue_without_a_parent_shows_only_its_own_layer() {
        let host = FakeHost::connected();
        host.answer("GET", "/rest/api/3/field", 200, sample_fields());
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1?fields=status,assignee,description,summary,issuetype,parent",
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
