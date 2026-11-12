//! A plugin for Task Proxy's own tests. Its `call` action makes whatever host call the test names and returns the
//! answer, or the error, as text, so the host's tests can check what a plugin is and is not allowed to do.
use taskproxy_plugin_sdk::ui::{self, Action, Node};
use taskproxy_plugin_sdk::*;

pub struct TestPlugin;

fn param<'a>(request: &'a ActionRequest, name: &str) -> &'a str {
    request
        .params
        .get(name)
        .and_then(Value::as_str)
        .unwrap_or("")
}

impl Plugin for TestPlugin {
    fn render(&self, _host: &dyn Host, request: RenderRequest) -> Result<Node> {
        match request.page.as_str() {
            "home" => Ok(ui::stack(vec![
                ui::heading("Test Home", 2),
                ui::text(format!("params: {}", Value::Object(request.params))),
                ui::button(
                    Action::new("call", "Call")
                        .param("op", "var")
                        .param("key", "test_site"),
                    "",
                ),
            ])),
            "panic" => panic!("render panicked"),
            "spin" => loop {
                std::hint::spin_loop();
            },
            other => Err(Error(format!("No page named {}", other))),
        }
    }

    fn action(&self, host: &dyn Host, request: ActionRequest) -> Result<ActionResult> {
        if request.action != "call" {
            return Err(Error(format!("No action named {}", request.action)));
        }
        let key = param(&request, "key");
        let answer = match param(&request, "op") {
            "var" => host.var(key).map(|value| json!(value)),
            "storeGet" => host.store_get(key).map(|value| json!(value)),
            "storeSet" => host
                .store_set(key, request.params.get("value").cloned())
                .map(|_| json!("saved")),
            "fileRead" => host.project_file_read(key).map(|value| json!(value)),
            "fileWrite" => host
                .project_file_write(key, param(&request, "text"))
                .map(|_| json!("written")),
            "branch" => host.current_branch().map(|value| json!(value)),
            "now" => host.now_ms().map(|value| json!(value > 0)),
            "http" => host
                .http(&HttpRequest::new("GET", key))
                .map(|response| json!({ "status": response.status, "body": response.body })),
            other => Err(Error(format!("No op named {}", other))),
        };
        let text = match answer {
            Ok(value) => format!("ok: {}", value),
            Err(err) => format!("error: {}", err),
        };
        Ok(ActionResult::ui(ui::text(text)))
    }

    fn branch_info(&self, _host: &dyn Host, request: BranchInfoRequest) -> Result<Option<Node>> {
        Ok(request
            .branch
            .starts_with("TEST-")
            .then(|| ui::text(format!("Branch {}", request.branch))))
    }
}

export_plugin!(TestPlugin);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_pages_and_actions_are_errors() {
        struct NoHost;
        impl Host for NoHost {
            fn var(&self, _: &str) -> Result<Option<String>> {
                Ok(None)
            }
            fn store_get(&self, _: &str) -> Result<Option<Value>> {
                Ok(None)
            }
            fn store_set(&self, _: &str, _: Option<Value>) -> Result<()> {
                Ok(())
            }
            fn project_file_read(&self, _: &str) -> Result<Option<String>> {
                Ok(None)
            }
            fn project_file_write(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
            fn current_branch(&self) -> Result<Option<String>> {
                Ok(None)
            }
            fn now_ms(&self) -> Result<u64> {
                Ok(1)
            }
            fn http(&self, _: &HttpRequest) -> Result<HttpResponse> {
                Err(Error::new("offline"))
            }
        }
        let request = RenderRequest {
            page: "nope".into(),
            ..Default::default()
        };
        assert!(TestPlugin.render(&NoHost, request).is_err());
        let request = ActionRequest {
            action: "call".into(),
            params: json!({ "op": "http", "key": "https://a.example.com" })
                .as_object()
                .unwrap()
                .clone(),
            ..Default::default()
        };
        assert_eq!(
            TestPlugin.action(&NoHost, request).unwrap().ui,
            Some(ui::text("error: offline"))
        );
    }
}
