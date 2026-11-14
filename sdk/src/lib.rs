//! Everything a Rust plugin for Task Proxy needs.
//!
//! A plugin implements [`Plugin`] and exports it with [`export_plugin!`]. Task Proxy calls three functions:
//!
//! - `render` draws a page, returning a [`ui::Node`] tree that Task Proxy turns into its own components.
//! - `action` answers a button, link, or form the plugin drew.
//! - `branch_info` returns a small card about a git branch for the Git Controller, when the plugin's manifest
//!   contributes `gitBranchInfo`.
//!
//! A plugin reaches nothing on its own. Variables, its own storage, project files, the current branch, the clock,
//! and HTTP all go through [`Host`], and Task Proxy refuses anything the plugin's `plugin.json` did not declare.
//! Every call is JSON, so the protocol in [`protocol`] is the whole contract, and plugins can be written in any
//! language with an Extism PDK.
//!
//! Unit tests build natively and pass their own [`Host`], so plugin logic can be tested without WebAssembly.

pub mod host;
pub mod protocol;
pub mod ui;

pub use host::{Host, HostInfo, HttpRequest, HttpResponse, host_supports};
pub use protocol::{
    ActionRequest, ActionResult, BranchInfoRequest, Dialog, HostAction, Message, Navigate,
    Notification, NotificationLink, NotificationReadRequest, NotificationsRequest, RenderRequest,
};
pub use serde_json::{self, Map, Value, json};

/// The newest plugin spec version this SDK speaks. A manifest lists the spec versions a plugin supports as `spec`,
/// each the lowest patch of its line the plugin needs, which is often lower than this. See `spec/` in this repo.
pub const SPEC_VERSION: &str = "0.1.2";

/// What a manifest names as `apiVersion` while it supports spec line 0.1, for Task Proxy 0.1.121 and earlier,
/// which read no `spec`.
pub const API_VERSION: u32 = 1;

/// An error a plugin returns. Task Proxy shows the message to the user.
#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);

impl Error {
    pub fn new(message: impl Into<String>) -> Self {
        Error(message.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error(format!("Invalid JSON: {}", err))
    }
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Error(message)
    }
}

impl From<&str> for Error {
    fn from(message: &str) -> Self {
        Error(message.to_string())
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// A Task Proxy plugin.
pub trait Plugin {
    /// Draws one of the pages the manifest lists.
    fn render(&self, host: &dyn Host, request: RenderRequest) -> Result<ui::Node>;

    /// Answers an action from something the plugin drew.
    fn action(&self, host: &dyn Host, request: ActionRequest) -> Result<ActionResult>;

    /// A card about a git branch, for the Git Controller. `None` when the plugin has nothing to say about it.
    fn branch_info(
        &self,
        _host: &dyn Host,
        _request: BranchInfoRequest,
    ) -> Result<Option<ui::Node>> {
        Ok(None)
    }

    /// The notifications to show in Task Proxy's Alerts panel, when the manifest contributes `notifications`.
    /// Task Proxy asks every few minutes, and removes those no longer listed.
    fn notifications(
        &self,
        _host: &dyn Host,
        _request: NotificationsRequest,
    ) -> Result<Vec<Notification>> {
        Ok(Vec::new())
    }

    /// The user closed a notification in the Alerts panel, so it should be marked read.
    fn notification_read(&self, _host: &dyn Host, _request: NotificationReadRequest) -> Result<()> {
        Ok(())
    }
}

/// What every export returns: `{"ok": ...}` or `{"error": "..."}`.
pub fn envelope<T: serde::Serialize>(result: Result<T>) -> Vec<u8> {
    let value = match result.and_then(|value| Ok(serde_json::to_value(value)?)) {
        Ok(value) => json!({ "ok": value }),
        Err(err) => json!({ "error": err.0 }),
    };
    serde_json::to_vec(&value)
        .unwrap_or_else(|_| br#"{"error":"Could not encode the result."}"#.to_vec())
}

/// Reads an export's JSON input, runs it, and wraps the answer in an [`envelope`].
pub fn run_export<I, O>(input: &[u8], call: impl FnOnce(I) -> Result<O>) -> Vec<u8>
where
    I: serde::de::DeserializeOwned,
    O: serde::Serialize,
{
    envelope(
        serde_json::from_slice::<I>(input)
            .map_err(Error::from)
            .and_then(call),
    )
}

#[doc(hidden)]
#[cfg(target_arch = "wasm32")]
pub use extism_pdk as __pdk;

/// Exports a [`Plugin`] for Task Proxy. Pass an expression that builds it, as in `export_plugin!(Jira)`.
#[macro_export]
macro_rules! export_plugin {
    ($plugin:expr) => {
        #[cfg(target_arch = "wasm32")]
        mod __taskproxy_exports {
            use super::*;
            use $crate::Plugin as _;

            fn respond(output: Vec<u8>) -> i32 {
                match $crate::__pdk::Memory::from_bytes(output) {
                    Ok(memory) => {
                        memory.set_output();
                        0
                    }
                    Err(_) => 1,
                }
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn render() -> i32 {
                let input = $crate::__pdk::input_bytes();
                respond($crate::run_export(&input, |request| {
                    ($plugin).render(&$crate::host::TaskProxy, request)
                }))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn action() -> i32 {
                let input = $crate::__pdk::input_bytes();
                respond($crate::run_export(&input, |request| {
                    ($plugin).action(&$crate::host::TaskProxy, request)
                }))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn branch_info() -> i32 {
                let input = $crate::__pdk::input_bytes();
                respond($crate::run_export(&input, |request| {
                    ($plugin).branch_info(&$crate::host::TaskProxy, request)
                }))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn notifications() -> i32 {
                let input = $crate::__pdk::input_bytes();
                respond($crate::run_export(&input, |request| {
                    ($plugin).notifications(&$crate::host::TaskProxy, request)
                }))
            }

            #[unsafe(no_mangle)]
            pub extern "C" fn notification_read() -> i32 {
                let input = $crate::__pdk::input_bytes();
                respond($crate::run_export(&input, |request| {
                    ($plugin).notification_read(&$crate::host::TaskProxy, request)
                }))
            }
        }
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_answer_with_an_envelope() {
        let ok = run_export(
            br#"{"page":"home","params":{}}"#,
            |request: RenderRequest| Ok(ui::text(request.page)),
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&ok).unwrap(),
            json!({ "ok": { "type": "text", "text": "home" } })
        );
        let failed = run_export(b"{}", |_: RenderRequest| Ok(ui::text("unreachable")));
        let failed: Value = serde_json::from_slice(&failed).unwrap();
        assert!(
            failed["error"].as_str().unwrap().contains("Invalid JSON"),
            "{}",
            failed
        );
        let refused = run_export(br#"{"page":"x"}"#, |_: RenderRequest| -> Result<ui::Node> {
            Err(Error::new("No such page"))
        });
        assert_eq!(
            serde_json::from_slice::<Value>(&refused).unwrap(),
            json!({ "error": "No such page" })
        );
    }
}
