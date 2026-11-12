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
        columns: Vec<String>,
        rows: Vec<TableRow>,
    },
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

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Property {
    pub label: String,
    pub value: Node,
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
    fn forms_describe_their_fields() {
        let form = Node::Form {
            fields: vec![
                Field::new(FieldKind::Select, "project", "Project")
                    .options(vec![Choice::new("P", "Platform")])
                    .required(),
            ],
            submit: Action::new("create", "Create"),
            cancel: None,
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
