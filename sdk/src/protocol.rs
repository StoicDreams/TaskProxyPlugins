//! What Task Proxy sends to a plugin's exports, and what an action returns.
use crate::ui::Node;
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

    pub fn with_message(mut self, text: impl Into<String>, theme: &str) -> Self {
        self.message = Some(Message {
            text: text.into(),
            theme: theme.to_string(),
        });
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Navigate {
    pub page: String,
    #[serde(default)]
    pub params: Map<String, Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    pub text: String,
    /// info, success, warning, or danger.
    pub theme: String,
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
