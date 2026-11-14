//! The screens a plugin describes, which Task Proxy draws with its own components. Plugins never send HTML or
//! scripts: markdown is the only rich text, and Task Proxy sanitizes it. A node type that Task Proxy does not know
//! yet is drawn as a placeholder, so new node types can be added without breaking older apps.
use crate::protocol::HostAction;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum Node {
    /// Children one above another.
    Stack { children: Vec<Node> },
    /// Children side by side, wrapping when there is no room.
    Row { children: Vec<Node> },
    Heading {
        text: String,
        /// 1 to 4. Pages start at 2, below the page title.
        #[serde(default = "default_level")]
        level: u8,
    },
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "is_false")]
        muted: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        strong: bool,
    },
    /// Markdown, sanitized by Task Proxy before it is shown.
    Markdown { text: String },
    Badge {
        text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        theme: String,
    },
    /// Runs an action when clicked.
    Link { text: String, action: Action },
    /// Labels and values, as for an issue's fields.
    Properties { items: Vec<Property> },
    Table {
        columns: Vec<Column>,
        rows: Vec<TableRow>,
    },
    /// A value the user can change in place, such as an issue's status in a table. Clicking it asks the plugin
    /// for the choices with `options`, whose result lists them in `options`, so a long table does not load the
    /// choices of every row. Picking one runs `action` with the choice as `value` in the form.
    Select {
        text: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        theme: String,
        options: Action,
        action: Action,
    },
    /// Text cut to `max` characters, with an ellipsis. Text that was cut shows its first `tip` characters as a
    /// tooltip, and runs `action` when clicked, such as to show all of it. With `always`, `action` runs even when
    /// nothing was cut.
    Clip {
        text: String,
        #[serde(default = "default_clip")]
        max: usize,
        #[serde(default = "default_tip")]
        tip: usize,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<Action>,
        #[serde(default, skip_serializing_if = "is_false")]
        always: bool,
        #[serde(default, skip_serializing_if = "is_false")]
        muted: bool,
    },
    /// A part of the page with no title, shown or hidden by a button whose host action is `toggle` with the same
    /// `key`. Task Proxy remembers whether the user left it open, by `key`.
    Panel {
        key: String,
        #[serde(default)]
        open: bool,
        children: Vec<Node>,
    },
    /// More of a form's fields, placed anywhere among its `children`.
    Inputs { fields: Vec<Field> },
    /// A collapsible section. Task Proxy remembers whether the user opened or closed it, by `key`.
    Section {
        key: String,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subtitle: Option<String>,
        /// Whether it starts open, before the user opens or closes it.
        #[serde(default)]
        open: bool,
        children: Vec<Node>,
    },
    Form {
        fields: Vec<Field>,
        submit: Action,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cancel: Option<Action>,
        /// Buttons shown before the submit button, such as ones that toggle panels of more fields.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        buttons: Vec<Node>,
        /// Shown below the buttons. The values of `Inputs` among them are sent with the form.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        children: Vec<Node>,
        /// Lays the fields and buttons out in one row, wrapping when there is no room. Since spec 0.1.2: older
        /// hosts stack them.
        #[serde(default, skip_serializing_if = "is_false")]
        inline: bool,
    },
    /// Items the user puts in order by dragging them, or with Alt+Up and Alt+Down. When the order changes, Task
    /// Proxy runs `action` with the items' ids in their new order as `order` in the form. Since spec 0.1.2: check
    /// [`crate::host_supports`] and draw something else for older hosts, which show a placeholder.
    Reorder {
        items: Vec<ReorderItem>,
        action: Action,
    },
    Button {
        action: Action,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        theme: String,
    },
    /// Buttons and links in a bar.
    Toolbar { children: Vec<Node> },
    /// A message in a colored box: info, success, warning, or danger.
    Notice { text: String, theme: String },
    /// Shown when a list has nothing in it.
    Empty { text: String },
}

fn default_level() -> u8 {
    2
}

fn default_clip() -> usize {
    30
}

fn default_tip() -> usize {
    100
}

/// A table's column title. Its `action` runs when the title is clicked, as to sort by it, and `sort` marks the
/// column the table is sorted by. A plain string is a title with neither.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Column {
    Title(String),
    Sortable {
        label: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        action: Option<Action>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sort: Option<Sort>,
    },
}

impl From<&str> for Column {
    fn from(text: &str) -> Self {
        Column::Title(text.to_string())
    }
}

impl From<String> for Column {
    fn from(text: String) -> Self {
        Column::Title(text)
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum Sort {
    Asc,
    Desc,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Property {
    pub label: String,
    pub value: Node,
}

/// One item of a [`Node::Reorder`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReorderItem {
    /// Unique in the list, and sent back in `order`.
    pub id: String,
    /// What the item shows beside its handle, such as its name and a remove link.
    pub children: Vec<Node>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TableRow {
    pub cells: Vec<Node>,
    /// Runs when the row is clicked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
}

/// Something the user can click. Task Proxy sends `id` and `params` to the plugin's `action` export, or, when
/// `host` is set, does that itself instead.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
    /// Asks the user this question first, and only continues if they agree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostAction>,
    /// An icon name from Task Proxy's icon set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl Action {
    pub fn new(id: &str, label: impl Into<String>) -> Self {
        Action {
            id: id.to_string(),
            label: label.into(),
            ..Default::default()
        }
    }

    /// An action Task Proxy runs itself, such as creating a branch.
    pub fn host(label: impl Into<String>, host: HostAction) -> Self {
        Action {
            label: label.into(),
            host: Some(host),
            ..Default::default()
        }
    }

    pub fn param(mut self, name: &str, value: impl Into<Value>) -> Self {
        self.params.insert(name.to_string(), value.into());
        self
    }

    pub fn confirm(mut self, question: impl Into<String>) -> Self {
        self.confirm = Some(question.into());
        self
    }

    pub fn icon(mut self, icon: &str) -> Self {
        self.icon = Some(icon.to_string());
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub enum FieldKind {
    #[default]
    Text,
    Password,
    Textarea,
    Number,
    Select,
    Multiselect,
    Checkbox,
    Hidden,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub struct Field {
    pub name: String,
    #[serde(default)]
    pub label: String,
    pub kind: FieldKind,
    /// A string, a number, a boolean for a checkbox, or a list of strings for a multiselect.
    #[serde(default)]
    pub value: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<Choice>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub required: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    /// For text: suggests people or other things to mention as the user types `@` and a few letters. Task Proxy
    /// runs this with the letters as `query` in the params, and the result's `options` are the suggestions. A
    /// picked one is written as `@` and its label, and the form also sends `<name>.mentions`, an object of each
    /// label written, with its `@`, to its value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suggest: Option<Action>,
}

impl Field {
    pub fn new(kind: FieldKind, name: &str, label: impl Into<String>) -> Self {
        Field {
            name: name.to_string(),
            label: label.into(),
            kind,
            ..Default::default()
        }
    }

    pub fn value(mut self, value: impl Into<Value>) -> Self {
        self.value = value.into();
        self
    }

    pub fn options(mut self, options: Vec<Choice>) -> Self {
        self.options = options;
        self
    }

    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn help(mut self, text: impl Into<String>) -> Self {
        self.help = text.into();
        self
    }

    pub fn suggest(mut self, action: Action) -> Self {
        self.suggest = Some(action);
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Choice {
    pub value: String,
    pub label: String,
}

impl Choice {
    pub fn new(value: impl Into<String>, label: impl Into<String>) -> Self {
        Choice {
            value: value.into(),
            label: label.into(),
        }
    }
}

pub fn stack(children: Vec<Node>) -> Node {
    Node::Stack { children }
}

pub fn row(children: Vec<Node>) -> Node {
    Node::Row { children }
}

pub fn toolbar(children: Vec<Node>) -> Node {
    Node::Toolbar { children }
}

pub fn heading(text: impl Into<String>, level: u8) -> Node {
    Node::Heading {
        text: text.into(),
        level,
    }
}

pub fn text(text: impl Into<String>) -> Node {
    Node::Text {
        text: text.into(),
        muted: false,
        strong: false,
    }
}

pub fn muted(text: impl Into<String>) -> Node {
    Node::Text {
        text: text.into(),
        muted: true,
        strong: false,
    }
}

pub fn strong(text: impl Into<String>) -> Node {
    Node::Text {
        text: text.into(),
        muted: false,
        strong: true,
    }
}

pub fn markdown(text: impl Into<String>) -> Node {
    Node::Markdown { text: text.into() }
}

pub fn badge(text: impl Into<String>, theme: &str) -> Node {
    Node::Badge {
        text: text.into(),
        theme: theme.to_string(),
    }
}

pub fn link(text: impl Into<String>, action: Action) -> Node {
    Node::Link {
        text: text.into(),
        action,
    }
}

pub fn button(action: Action, theme: &str) -> Node {
    Node::Button {
        action,
        theme: theme.to_string(),
    }
}

pub fn notice(text: impl Into<String>, theme: &str) -> Node {
    Node::Notice {
        text: text.into(),
        theme: theme.to_string(),
    }
}

pub fn empty(text: impl Into<String>) -> Node {
    Node::Empty { text: text.into() }
}

/// Text cut to 30 characters, showing up to 100 as a tooltip, that runs `action` when it was cut and is clicked.
pub fn clip(text: impl Into<String>, action: Option<Action>) -> Node {
    Node::Clip {
        text: text.into(),
        max: default_clip(),
        tip: default_tip(),
        action,
        always: false,
        muted: false,
    }
}

/// A part of the page shown and hidden by a button made with [`toggle`].
pub fn panel(key: impl Into<String>, open: bool, children: Vec<Node>) -> Node {
    Node::Panel {
        key: key.into(),
        open,
        children,
    }
}

/// A button that shows or hides the panel with this key.
pub fn toggle(label: impl Into<String>, key: impl Into<String>) -> Node {
    button(
        Action::host(label, HostAction::Toggle { key: key.into() }),
        "secondary",
    )
}

/// A plain form: fields and a submit button.
pub fn form(fields: Vec<Field>, submit: Action) -> Node {
    Node::Form {
        fields,
        submit,
        cancel: None,
        buttons: Vec::new(),
        children: Vec::new(),
        inline: false,
    }
}

pub fn property(label: impl Into<String>, value: Node) -> Property {
    Property {
        label: label.into(),
        value,
    }
}

pub fn section(
    key: impl Into<String>,
    title: impl Into<String>,
    open: bool,
    children: Vec<Node>,
) -> Node {
    Node::Section {
        key: key.into(),
        title: title.into(),
        subtitle: None,
        open,
        children,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nodes_are_tagged_by_type_and_leave_out_defaults() {
        let node = stack(vec![
            heading("Issues", 2),
            Node::Table {
                columns: vec!["Key".into()],
                rows: vec![TableRow {
                    cells: vec![text("P-1")],
                    action: Some(Action::new("open", "Open").param("key", "P-1")),
                }],
            },
            button(Action::new("refresh", "Refresh").icon("rotate"), ""),
        ]);
        assert_eq!(
            serde_json::to_value(&node).unwrap(),
            json!({ "type": "stack", "children": [
                { "type": "heading", "text": "Issues", "level": 2 },
                { "type": "table", "columns": ["Key"], "rows": [{
                    "cells": [{ "type": "text", "text": "P-1" }],
                    "action": { "id": "open", "label": "Open", "params": { "key": "P-1" } }
                }] },
                { "type": "button", "action": { "id": "refresh", "label": "Refresh", "icon": "rotate" } }
            ] })
        );
    }

    #[test]
    fn columns_are_plain_titles_or_sortable_and_new_nodes_read_back() {
        let columns: Vec<Column> = vec![
            "Key".into(),
            Column::Sortable {
                label: "Summary".into(),
                action: Some(Action::new("sort", "Sort by Summary")),
                sort: Some(Sort::Desc),
            },
        ];
        assert_eq!(
            serde_json::to_value(&columns).unwrap(),
            json!(["Key", { "label": "Summary", "action": { "id": "sort", "label": "Sort by Summary" }, "sort": "desc" }])
        );
        let read: Vec<Column> =
            serde_json::from_value(json!(["Key", { "label": "Status" }])).unwrap();
        assert_eq!(
            read[1],
            Column::Sortable {
                label: "Status".into(),
                action: None,
                sort: None
            }
        );
        let clip = clip("Long text", Some(Action::new("open", "Open")));
        assert_eq!(
            serde_json::to_value(&clip).unwrap(),
            json!({ "type": "clip", "text": "Long text", "max": 30, "tip": 100, "action": { "id": "open", "label": "Open" } })
        );
        // Older plugins send no max or tip.
        let read: Node = serde_json::from_value(json!({ "type": "clip", "text": "x" })).unwrap();
        assert!(matches!(
            read,
            Node::Clip {
                max: 30,
                tip: 100,
                ..
            }
        ));
        assert_eq!(
            serde_json::to_value(toggle("JQL", "jql")).unwrap(),
            json!({ "type": "button", "action": { "id": "", "label": "JQL", "host": { "type": "toggle", "key": "jql" } }, "theme": "secondary" })
        );
        let old_form: Node = serde_json::from_value(
            json!({ "type": "form", "fields": [], "submit": { "id": "go", "label": "Go" } }),
        )
        .unwrap();
        assert_eq!(old_form, form(Vec::new(), Action::new("go", "Go")));
    }

    #[test]
    fn reorder_lists_send_their_items_and_inline_forms_say_so() {
        let list = Node::Reorder {
            items: vec![ReorderItem {
                id: "0:summary".into(),
                children: vec![text("Summary")],
            }],
            action: Action::new("order", "Reorder"),
        };
        assert_eq!(
            serde_json::to_value(&list).unwrap(),
            json!({ "type": "reorder", "items": [{ "id": "0:summary", "children": [{ "type": "text", "text": "Summary" }] }],
                "action": { "id": "order", "label": "Reorder" } })
        );
        let inline_form = Node::Form {
            fields: Vec::new(),
            submit: Action::new("go", "Go"),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: true,
        };
        assert_eq!(serde_json::to_value(&inline_form).unwrap()["inline"], true);
    }

    #[test]
    fn forms_describe_their_fields() {
        let form = Node::Form {
            fields: vec![
                Field::new(FieldKind::Select, "project", "Project")
                    .options(vec![Choice::new("P", "Platform")])
                    .required(),
            ],
            submit: Action::new("create", "Create"),
            cancel: None,
            buttons: Vec::new(),
            children: Vec::new(),
            inline: false,
        };
        assert_eq!(
            serde_json::to_value(&form).unwrap(),
            json!({ "type": "form",
                "fields": [{ "name": "project", "label": "Project", "kind": "select", "value": null,
                    "options": [{ "value": "P", "label": "Platform" }], "required": true }],
                "submit": { "id": "create", "label": "Create" } })
        );
        let read: Node = serde_json::from_value(serde_json::to_value(&form).unwrap()).unwrap();
        assert_eq!(read, form);
    }
}
