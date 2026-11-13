//! Jira Cloud for Task Proxy: My Issues, each issue with its parents through Views, Create Issue, a branch for each
//! issue, and the current branch's issue in the Git Controller.
mod adf;
mod client;
mod fields;
mod list;
mod notify;
mod pages;
mod views;

use client::Jira;
use fields::Fields;
use taskproxy_plugin_sdk::ui::{self, Action, Node};
use taskproxy_plugin_sdk::*;

pub struct JiraPlugin;

/// Shown on every page until the site, email, and API token are set.
fn setup(host: &dyn Host) -> Result<Node> {
    let missing = client::missing_settings(host)?;
    Ok(ui::stack(vec![
        ui::notice(
            format!(
                "Jira needs a few settings before it can connect. Missing: {}.",
                missing.join(", ")
            ),
            "info",
        ),
        ui::markdown(
            "1. Open [Atlassian's API tokens page](https://id.atlassian.com/manage-profile/security/api-tokens) \
             and create a token.\n\
             2. In the plugin's settings, enter your Jira site (such as `https://your-team.atlassian.net`), \
             the email you sign in with, and the token.\n\n\
             The token is saved encrypted with your other Task Proxy secrets, and only this plugin can read it.",
        ),
        ui::toolbar(vec![ui::button(
            Action::host("Open Jira settings", HostAction::OpenSettings).icon("gear"),
            "primary",
        )]),
    ]))
}

impl Plugin for JiraPlugin {
    fn render(&self, host: &dyn Host, request: RenderRequest) -> Result<Node> {
        let Some(jira) = Jira::connect(host)? else {
            return setup(host);
        };
        match request.page.as_str() {
            "issues" | "all" => list::page(
                &jira,
                &Fields::load(&jira)?,
                list::List::of(&request.page),
                &request.params,
            ),
            "issue" => pages::issue(&jira, &Fields::load(&jira)?, &request.params),
            "create" => pages::create(&jira, &request.params),
            "notifications" => notify::page(&jira),
            "views" => views::page(host, &Fields::load(&jira)?, &request.params),
            other => Err(Error(format!("Jira has no page named {}.", other))),
        }
    }

    fn action(&self, host: &dyn Host, request: ActionRequest) -> Result<ActionResult> {
        let Some(jira) = Jira::connect(host)? else {
            return Ok(ActionResult::ui(setup(host)?));
        };
        if let Some(result) = pages::action(&jira, &request)? {
            return Ok(result);
        }
        if let Some(result) = list::action(&jira, &request)? {
            return Ok(result);
        }
        if let Some(result) = notify::action(&jira, &request)? {
            return Ok(result);
        }
        if request.action.starts_with("views.") || request.action.starts_with("draft.") {
            let fields = Fields::load(&jira)?;
            if let Some(result) = views::action(host, &fields, &request)? {
                return Ok(result);
            }
        }
        Err(Error(format!(
            "Jira does not know the action {}.",
            request.action
        )))
    }

    fn branch_info(&self, host: &dyn Host, request: BranchInfoRequest) -> Result<Option<Node>> {
        match Jira::connect(host)? {
            Some(jira) => pages::branch_card(&jira, &request.branch),
            None => Ok(None),
        }
    }

    fn notifications(
        &self,
        host: &dyn Host,
        _request: NotificationsRequest,
    ) -> Result<Vec<Notification>> {
        match Jira::connect(host)? {
            Some(jira) => Ok(notify::list(&jira)?.1),
            None => Ok(Vec::new()),
        }
    }

    fn notification_read(&self, host: &dyn Host, request: NotificationReadRequest) -> Result<()> {
        match Jira::connect(host)? {
            Some(jira) => notify::mark_read(&jira, &[request.id]),
            None => Ok(()),
        }
    }
}

export_plugin!(JiraPlugin);

/// A stand-in for Task Proxy, for tests and the demo, which never run as WebAssembly.
#[cfg(not(target_arch = "wasm32"))]
pub mod testing {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;

    /// Answers HTTP requests from canned responses by method and path, and keeps storage and project files in
    /// memory, so pages and actions can be tested without Task Proxy or Jira.
    #[derive(Default)]
    pub struct FakeHost {
        pub vars: HashMap<String, String>,
        pub answers: RefCell<HashMap<String, (u16, Value)>>,
        /// Answers chosen by a piece of the request body, checked first, for searches that differ only by JQL.
        pub body_answers: RefCell<Vec<(String, String, u16, Value)>>,
        pub sent: RefCell<Vec<HttpRequest>>,
        pub store: RefCell<HashMap<String, Value>>,
        pub files: RefCell<HashMap<String, String>>,
        pub project_open: Cell<bool>,
        pub now: Cell<u64>,
    }

    impl FakeHost {
        pub fn connected() -> Self {
            let host = FakeHost {
                vars: HashMap::from([
                    (client::SITE.to_string(), "team".to_string()),
                    (client::EMAIL.to_string(), "me@team.com".to_string()),
                    (client::TOKEN.to_string(), "secret".to_string()),
                ]),
                ..Default::default()
            };
            host.now.set(1_700_000_000_000);
            host
        }

        /// The answer for a method and path, such as `GET /rest/api/3/myself`, including any query.
        pub fn answer(&self, method: &str, path: &str, status: u16, body: Value) {
            self.answers
                .borrow_mut()
                .insert(format!("{} {}", method, path), (status, body));
        }

        /// The answer for a method and path whose body contains `needle`.
        pub fn answer_when(
            &self,
            method: &str,
            path: &str,
            needle: &str,
            status: u16,
            body: Value,
        ) {
            self.body_answers.borrow_mut().push((
                format!("{} {}", method, path),
                needle.to_string(),
                status,
                body,
            ));
        }
    }

    impl Host for FakeHost {
        fn var(&self, key: &str) -> Result<Option<String>> {
            Ok(self.vars.get(key).cloned())
        }
        fn store_get(&self, key: &str) -> Result<Option<Value>> {
            Ok(self.store.borrow().get(key).cloned())
        }
        fn store_set(&self, key: &str, value: Option<Value>) -> Result<()> {
            match value {
                Some(value) => self.store.borrow_mut().insert(key.to_string(), value),
                None => self.store.borrow_mut().remove(key),
            };
            Ok(())
        }
        fn project_file_read(&self, path: &str) -> Result<Option<String>> {
            if !self.project_open.get() {
                return Err(Error::new("No project is open."));
            }
            Ok(self.files.borrow().get(path).cloned())
        }
        fn project_file_write(&self, path: &str, text: &str) -> Result<()> {
            if !self.project_open.get() {
                return Err(Error::new("No project is open."));
            }
            self.files
                .borrow_mut()
                .insert(path.to_string(), text.to_string());
            Ok(())
        }
        fn current_branch(&self) -> Result<Option<String>> {
            Ok(None)
        }
        fn now_ms(&self) -> Result<u64> {
            Ok(self.now.get())
        }
        fn http(&self, request: &HttpRequest) -> Result<HttpResponse> {
            self.sent.borrow_mut().push(request.clone());
            let path = request
                .url
                .strip_prefix("https://team.atlassian.net")
                .unwrap_or(&request.url);
            let key = format!("{} {}", request.method, path);
            // An answer set without a query answers every query, for data that does not depend on it.
            let without_query = key.split('?').next().unwrap_or(&key).to_string();
            let sent_body = request.body.clone().unwrap_or_default();
            let by_body = self
                .body_answers
                .borrow()
                .iter()
                .find(|(at, needle, ..)| at == &key && sent_body.contains(needle.as_str()))
                .map(|(_, _, status, body)| (*status, body.clone()));
            let answers = self.answers.borrow();
            let (status, body) = by_body
                .or_else(|| answers.get(&key).cloned())
                .map(Some)
                .unwrap_or_else(|| answers.get(&without_query).cloned())
                .ok_or_else(|| Error(format!("No answer set for {}", key)))?;
            Ok(HttpResponse {
                status,
                body: if body.is_null() {
                    String::new()
                } else {
                    body.to_string()
                },
            })
        }
    }

    pub fn sample_fields() -> Value {
        json!([
            { "id": "summary", "name": "Summary", "custom": false, "schema": { "type": "string" } },
            { "id": "status", "name": "Status", "custom": false, "schema": { "type": "status" } },
            { "id": "description", "name": "Description", "custom": false, "schema": { "type": "string" } },
            { "id": "comment", "name": "Comment", "custom": false, "schema": { "type": "comments-page" } },
            { "id": "assignee", "name": "Assignee", "custom": false, "schema": { "type": "user" } },
            { "id": "attachment", "name": "Attachment", "custom": false },
            { "id": "customfield_10016", "name": "Story point estimate", "custom": true, "schema": { "type": "number" } },
            { "id": "customfield_10020", "name": "Sprint", "custom": true, "schema": { "type": "array" } },
            { "id": "customfield_10050", "name": "Acceptance criteria", "custom": true, "schema": { "type": "string" } },
            { "id": "customfield_10040", "name": "Team", "custom": true, "schema": { "type": "team" } }
        ])
    }
}

#[cfg(not(target_arch = "wasm32"))]
pub mod demo;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub use crate::testing::{FakeHost, sample_fields};

    #[test]
    fn without_settings_every_page_shows_setup() {
        let host = FakeHost::default();
        for page in ["issues", "all", "issue", "create", "views", "notifications"] {
            let node = JiraPlugin
                .render(
                    &host,
                    RenderRequest {
                        page: page.into(),
                        ..Default::default()
                    },
                )
                .unwrap();
            let text = serde_json::to_string(&node).unwrap();
            assert!(
                text.contains("Missing: Jira site, Email, API token"),
                "{}",
                text
            );
            assert!(text.contains("openSettings"), "{}", text);
        }
        assert_eq!(
            JiraPlugin
                .branch_info(
                    &host,
                    BranchInfoRequest {
                        branch: "P-1".into()
                    }
                )
                .unwrap(),
            None
        );
    }

    #[test]
    fn unknown_pages_and_actions_are_errors() {
        let host = FakeHost::connected();
        assert!(
            JiraPlugin
                .render(
                    &host,
                    RenderRequest {
                        page: "nope".into(),
                        ..Default::default()
                    }
                )
                .is_err()
        );
        let request = ActionRequest {
            action: "nope".into(),
            ..Default::default()
        };
        assert!(JiraPlugin.action(&host, request).is_err());
    }

    #[test]
    fn the_manifest_matches_the_plugin() {
        let manifest: Value = serde_json::from_str(include_str!("../plugin.json")).unwrap();
        assert_eq!(manifest["apiVersion"], API_VERSION);
        assert_eq!(manifest["version"], env!("CARGO_PKG_VERSION"));
        let declared: Vec<&str> = manifest["permissions"]["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|variable| variable["key"].as_str().unwrap())
            .collect();
        assert_eq!(declared, vec![client::SITE, client::EMAIL, client::TOKEN]);
        for page in ["issues", "all", "issue", "create", "views", "notifications"] {
            assert!(
                manifest["pages"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|p| p["id"] == page),
                "{}",
                page
            );
        }
    }
}
