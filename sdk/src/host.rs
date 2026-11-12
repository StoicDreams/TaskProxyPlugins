//! What Task Proxy does for a plugin. Each call is one JSON request through the `taskproxy_host` import,
//! answered with `{"ok": ...}` or `{"error": "..."}`, so new calls never need a new import.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// An HTTPS request. Task Proxy only sends it to a host the manifest lists in `permissions.hosts`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    /// The body as text, usually JSON.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
}

impl HttpRequest {
    pub fn new(method: &str, url: impl Into<String>) -> Self {
        HttpRequest {
            method: method.to_string(),
            url: url.into(),
            ..Default::default()
        }
    }

    pub fn header(mut self, name: &str, value: impl Into<String>) -> Self {
        self.headers.insert(name.to_string(), value.into());
        self
    }

    /// Sends a JSON body, with its content type.
    pub fn json(mut self, body: &Value) -> Self {
        self.headers
            .insert("Content-Type".into(), "application/json".into());
        self.body = Some(body.to_string());
        self
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HttpResponse {
    pub status: u16,
    #[serde(default)]
    pub body: String,
}

impl HttpResponse {
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    pub fn json(&self) -> Result<Value> {
        if self.body.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&self.body)?)
    }
}

/// Everything a plugin can ask of Task Proxy.
pub trait Host {
    /// A variable the manifest declares, from the page, then the project, then global scope.
    fn var(&self, key: &str) -> Result<Option<String>>;
    /// A value from the plugin's own storage, which Task Proxy keeps encrypted with the rest of its data.
    fn store_get(&self, key: &str) -> Result<Option<Value>>;
    /// Saves a value in the plugin's own storage, or removes it with `None`.
    fn store_set(&self, key: &str, value: Option<Value>) -> Result<()>;
    /// A file under `.taskproxy/plugins/<plugin id>/` in the open project. `None` when it does not exist.
    fn project_file_read(&self, path: &str) -> Result<Option<String>>;
    /// Writes a file under `.taskproxy/plugins/<plugin id>/` in the open project, so it is shared through git.
    fn project_file_write(&self, path: &str, text: &str) -> Result<()>;
    /// The branch checked out in the open project's selected repo.
    fn current_branch(&self) -> Result<Option<String>>;
    /// Milliseconds since the Unix epoch. WebAssembly has no clock of its own.
    fn now_ms(&self) -> Result<u64>;
    /// Sends an HTTPS request to a host the manifest allows.
    fn http(&self, request: &HttpRequest) -> Result<HttpResponse>;
}

/// The requests a plugin sends through the `taskproxy_host` import, by `op`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum HostRequest {
    Var { key: String },
    StoreGet { key: String },
    StoreSet { key: String, value: Option<Value> },
    ProjectFileRead { path: String },
    ProjectFileWrite { path: String, text: String },
    CurrentBranch,
    Now,
    Http { request: HttpRequest },
}

/// Turns a host's `{"ok": ...}` or `{"error": "..."}` answer into a result.
pub fn read_answer<T: serde::de::DeserializeOwned>(answer: &[u8]) -> Result<T> {
    let mut value: Value = serde_json::from_slice(answer)?;
    if let Some(error) = value.get("error") {
        return Err(Error(
            error
                .as_str()
                .unwrap_or("Task Proxy refused the request.")
                .to_string(),
        ));
    }
    Ok(serde_json::from_value(
        value.get_mut("ok").map(Value::take).unwrap_or(Value::Null),
    )?)
}

/// The real host, inside Task Proxy.
#[cfg(target_arch = "wasm32")]
pub struct TaskProxy;

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "extism:host/user")]
unsafe extern "C" {
    fn taskproxy_host(request: u64) -> u64;
}

#[cfg(target_arch = "wasm32")]
impl TaskProxy {
    fn call<T: serde::de::DeserializeOwned>(&self, request: HostRequest) -> Result<T> {
        use extism_pdk::Memory;
        let input = Memory::from_bytes(serde_json::to_vec(&request)?)
            .map_err(|err| Error(err.to_string()))?;
        let offset = unsafe { taskproxy_host(input.offset()) };
        input.free();
        let output =
            Memory::find(offset).ok_or_else(|| Error::new("Task Proxy did not answer."))?;
        let answer = output.to_vec();
        output.free();
        read_answer(&answer)
    }
}

#[cfg(target_arch = "wasm32")]
impl Host for TaskProxy {
    fn var(&self, key: &str) -> Result<Option<String>> {
        self.call(HostRequest::Var { key: key.into() })
    }
    fn store_get(&self, key: &str) -> Result<Option<Value>> {
        self.call(HostRequest::StoreGet { key: key.into() })
    }
    fn store_set(&self, key: &str, value: Option<Value>) -> Result<()> {
        self.call::<Value>(HostRequest::StoreSet {
            key: key.into(),
            value,
        })
        .map(|_| ())
    }
    fn project_file_read(&self, path: &str) -> Result<Option<String>> {
        self.call(HostRequest::ProjectFileRead { path: path.into() })
    }
    fn project_file_write(&self, path: &str, text: &str) -> Result<()> {
        self.call::<Value>(HostRequest::ProjectFileWrite {
            path: path.into(),
            text: text.into(),
        })
        .map(|_| ())
    }
    fn current_branch(&self) -> Result<Option<String>> {
        self.call(HostRequest::CurrentBranch)
    }
    fn now_ms(&self) -> Result<u64> {
        self.call(HostRequest::Now)
    }
    fn http(&self, request: &HttpRequest) -> Result<HttpResponse> {
        self.call(HostRequest::Http {
            request: request.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn host_requests_are_tagged_by_op() {
        assert_eq!(
            serde_json::to_value(HostRequest::Var {
                key: "jira_site".into()
            })
            .unwrap(),
            json!({ "op": "var", "key": "jira_site" })
        );
        assert_eq!(
            serde_json::to_value(HostRequest::CurrentBranch).unwrap(),
            json!({ "op": "currentBranch" })
        );
        let http = HostRequest::Http {
            request: HttpRequest::new("POST", "https://x.atlassian.net/a").json(&json!({ "a": 1 })),
        };
        assert_eq!(
            serde_json::to_value(http).unwrap(),
            json!({ "op": "http", "request": {
                "method": "POST", "url": "https://x.atlassian.net/a",
                "headers": { "Content-Type": "application/json" }, "body": "{\"a\":1}"
            } })
        );
    }

    #[test]
    fn answers_become_results() {
        assert_eq!(
            read_answer::<Option<String>>(br#"{"ok":"main"}"#),
            Ok(Some("main".into()))
        );
        assert_eq!(read_answer::<Option<String>>(br#"{"ok":null}"#), Ok(None));
        assert_eq!(
            read_answer::<Option<String>>(br#"{"error":"jira_token is not declared"}"#),
            Err(Error::new("jira_token is not declared"))
        );
    }
}
