//! What Task Proxy sends to a plugin's exports, and what an action returns.
use crate::ui::{Choice, Node};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Asks for a page. `params` come from the page's address, as in `/plugin/jira/issue?key=PROJ-1`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RenderRequest {
    pub page: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

/// An action from something the plugin drew: the action's id and params, the page it was on, and the values of
/// the form it submitted, if any.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActionRequest {
    pub page: String,
    #[serde(default)]
    pub page_params: Map<String, Value>,
    pub action: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    #[serde(default)]
    pub form: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BranchInfoRequest {
    pub branch: String,
}

/// What an action does next. Leave everything empty to change nothing.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ActionResult {
    /// Replaces the page's content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ui: Option<Node>,
    /// Opens another of the plugin's pages, or the same page with other params.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub navigate: Option<Navigate>,
    /// Draws the current page again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub refresh: bool,
    /// Shown briefly to the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
    /// Opens a dialog. Actions from inside it answer with `ui` to change what it shows, `refresh` to draw the page
    /// behind it again, and `close_dialog` to close it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialog: Option<Dialog>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub close_dialog: bool,
    /// The choices for a `Select` node or a field's `suggest`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<Choice>>,
    /// Changes whether the page shown beside this one covers it, keeping the page that shows there. Since spec
    /// 0.1.4.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<bool>,
}

impl ActionResult {
    pub fn ui(node: Node) -> Self {
        ActionResult {
            ui: Some(node),
            ..Default::default()
        }
    }

    pub fn navigate(page: impl Into<String>, params: Map<String, Value>) -> Self {
        ActionResult {
            navigate: Some(Navigate {
                page: page.into(),
                params,
                side: false,
                cover: false,
            }),
            ..Default::default()
        }
    }

    pub fn refresh() -> Self {
        ActionResult {
            refresh: true,
            ..Default::default()
        }
    }

    pub fn dialog(title: impl Into<String>, ui: Node) -> Self {
        ActionResult {
            dialog: Some(Dialog {
                title: title.into(),
                ui,
            }),
            ..Default::default()
        }
    }

    pub fn options(options: Vec<Choice>) -> Self {
        ActionResult {
            options: Some(options),
            ..Default::default()
        }
    }

    /// Also draws the page behind a dialog again.
    pub fn and_refresh(mut self) -> Self {
        self.refresh = true;
        self
    }

    /// Also closes the dialog the action came from.
    pub fn and_close(mut self) -> Self {
        self.close_dialog = true;
        self
    }

    /// Opens the page navigated to beside the current one, in a split the user resizes. Since spec 0.1.4: older
    /// hosts open it in place of the current page.
    pub fn beside(mut self) -> Self {
        if let Some(navigate) = self.navigate.as_mut() {
            navigate.side = true;
        }
        self
    }

    /// Whether a page beside another covers it at any width, with a button on each to switch between them, as in a
    /// narrow window. On a result that navigates beside, it is how that page opens. On any other result, it changes
    /// the page already beside. Since spec 0.1.4: hosts that support `side` but not this show both side by side.
    pub fn cover(mut self, cover: bool) -> Self {
        match self.navigate.as_mut() {
            Some(navigate) => navigate.cover = cover,
            None => self.cover = Some(cover),
        }
        self
    }

    pub fn with_message(mut self, text: impl Into<String>, theme: &str) -> Self {
        self.message = Some(Message {
            text: text.into(),
            theme: theme.to_string(),
        });
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Dialog {
    pub title: String,
    pub ui: Node,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Navigate {
    pub page: String,
    #[serde(default)]
    pub params: Map<String, Value>,
    /// Opens the page beside the current one. Since spec 0.1.4.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub side: bool,
    /// With `side`, the page covers the current one at any width, with a button on each to switch between them.
    /// Since spec 0.1.4.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cover: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub text: String,
    /// info, success, warning, or danger.
    pub theme: String,
}

/// Asks for the plugin's notifications, which Task Proxy shows in its Alerts panel.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationsRequest {}

/// One notification. Task Proxy keeps it in the Alerts panel until the plugin stops listing it, or the user closes
/// it, which calls the plugin's `notification_read` with its `id`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Notification {
    /// Unique among the plugin's notifications, and the same each time it is listed.
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// info, success, warning, or danger.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub theme: String,
    /// When it happened, as Jira or the plugin writes it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub time: String,
    /// Each place in `title` or `text` where this text appears becomes a link to one of the plugin's pages.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<NotificationLink>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationLink {
    pub text: String,
    pub page: String,
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub params: Map<String, Value>,
}

/// The user closed a notification.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NotificationReadRequest {
    pub id: String,
}

/// Something Task Proxy does itself when the user clicks a button that names it. A plugin cannot run these on
/// its own, so it cannot change the user's repo or open pages without a click that says so.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "type")]
pub enum HostAction {
    /// Creates a branch from the current one in the open project's selected repo and switches to it. Needs the
    /// `createBranch` git permission.
    #[serde(rename = "git.createBranch")]
    GitCreateBranch { name: String },
    /// Opens an https address in the user's browser.
    #[serde(rename = "openUrl")]
    OpenUrl { url: String },
    /// Copies text to the clipboard.
    #[serde(rename = "copy")]
    Copy { text: String },
    /// Opens the plugin's settings in Task Proxy, where the user enters the variables it declares.
    #[serde(rename = "openSettings")]
    OpenSettings,
    /// Shows or hides the `Panel` with this key, on the page or in its dialog.
    #[serde(rename = "toggle")]
    Toggle { key: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn requests_read_what_task_proxy_sends() {
        let request: ActionRequest = serde_json::from_value(json!({
            "page": "issue", "pageParams": { "key": "P-1" }, "action": "comment",
            "params": { "key": "P-1" }, "form": { "body": "Looks good" }
        }))
        .unwrap();
        assert_eq!(request.page_params["key"], "P-1");
        assert_eq!(request.form["body"], "Looks good");
        let request: RenderRequest = serde_json::from_value(json!({ "page": "issues" })).unwrap();
        assert!(request.params.is_empty());
    }

    #[test]
    fn empty_results_send_nothing_extra() {
        assert_eq!(
            serde_json::to_value(ActionResult::default()).unwrap(),
            json!({})
        );
        assert_eq!(
            serde_json::to_value(ActionResult::refresh().with_message("Saved", "success")).unwrap(),
            json!({ "refresh": true, "message": { "text": "Saved", "theme": "success" } })
        );
    }

    #[test]
    fn host_actions_are_tagged_by_type() {
        assert_eq!(
            serde_json::to_value(HostAction::GitCreateBranch {
                name: "P-1-fix".into()
            })
            .unwrap(),
            json!({ "type": "git.createBranch", "name": "P-1-fix" })
        );
    }
}
