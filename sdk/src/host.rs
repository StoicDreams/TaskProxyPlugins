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

/// What `Host::host_info` reports. Since spec 0.1.1.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct HostInfo {
    /// Task Proxy's version, such as `0.1.122`.
    pub app: String,
    /// The plugin spec version Task Proxy runs this plugin with: the newest both support.
    pub spec: String,
    /// The operating system: `linux`, `macos`, or `windows`.
    pub platform: String,
}

impl HostInfo {
    /// Whether the host runs at least this spec version on the same line, as `0.1.2`.
    pub fn supports(&self, version: &str) -> bool {
        spec_at_least(&self.spec, version)
    }
}

fn spec_parts(version: &str) -> Option<(u32, u32, u32)> {
    let mut parts = version
        .trim()
        .split('.')
        .map(|part| part.parse::<u32>().ok());
    let found = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(found)
}

/// Whether `have` is `want` or a later patch of the same spec line.
pub(crate) fn spec_at_least(have: &str, want: &str) -> bool {
    match (spec_parts(have), spec_parts(want)) {
        (Some(have), Some(want)) => have.0 == want.0 && have.1 == want.1 && have.2 >= want.2,
        _ => false,
    }
}

/// Whether the host supports this spec version, for drawing a newer node only where it is known. A host that
/// cannot say, such as Task Proxy 0.1.121 and earlier, runs spec 0.1.0.
pub fn host_supports(host: &dyn Host, version: &str) -> bool {
    match host.host_info() {
        Ok(info) => info.supports(version),
        Err(_) => spec_at_least("0.1.0", version),
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
    /// Task Proxy's version, the spec version it runs the plugin with, and the platform. Since spec 0.1.1: Task
    /// Proxy 0.1.121 and earlier answer with an error, which means spec 0.1.0.
    fn host_info(&self) -> Result<HostInfo> {
        Err(Error::new("This host does not report its details."))
    }
}

/// The requests a plugin sends through the `taskproxy_host` import, by `op`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum HostRequest {
    Var {
        key: String,
    },
    StoreGet {
        key: String,
    },
    StoreSet {
        key: String,
        value: Option<Value>,
    },
    ProjectFileRead {
        path: String,
    },
    ProjectFileWrite {
        path: String,
        text: String,
    },
    CurrentBranch,
    Now,
    Http {
        request: HttpRequest,
    },
    /// Since spec 0.1.1.
    HostInfo,
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
    fn host_info(&self) -> Result<HostInfo> {
        self.call(HostRequest::HostInfo)
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
        assert_eq!(
            serde_json::to_value(HostRequest::HostInfo).unwrap(),
            json!({ "op": "hostInfo" })
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
    fn spec_versions_compare_within_a_line() {
        let info = |spec: &str| HostInfo {
            spec: spec.into(),
            ..Default::default()
        };
        assert!(info("0.1.2").supports("0.1.2"));
        assert!(info("0.1.3").supports("0.1.0"));
        assert!(!info("0.1.1").supports("0.1.2"));
        assert!(!info("0.2.5").supports("0.1.0"));
        assert!(!info("0.1").supports("0.1.0"));
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
