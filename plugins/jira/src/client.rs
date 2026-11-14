//! Talks to Jira Cloud's REST API, version 3, with the user's email and API token.
use taskproxy_plugin_sdk::{Error, Host, HttpRequest, Result, Value};

pub const SITE: &str = "jira_site";
pub const EMAIL: &str = "jira_email";
pub const TOKEN: &str = "jira_api_token";

pub struct Jira<'a> {
    pub host: &'a dyn Host,
    /// https://team.atlassian.net, with no slash at the end.
    pub site: String,
    auth: String,
}

/// The settings that are missing, by label, for the setup notice.
pub fn missing_settings(host: &dyn Host) -> Result<Vec<&'static str>> {
    let mut missing = Vec::new();
    for (key, label) in [(SITE, "Jira site"), (EMAIL, "Email"), (TOKEN, "API token")] {
        if host.var(key)?.is_none_or(|value| value.trim().is_empty()) {
            missing.push(label);
        }
    }
    Ok(missing)
}

/// `team`, `team.atlassian.net`, or `https://team.atlassian.net/jira/...` all become https://team.atlassian.net.
pub fn normalize_site(site: &str) -> Result<String> {
    let site = site.trim();
    let without_scheme = site
        .strip_prefix("https://")
        .or_else(|| site.strip_prefix("http://"))
        .unwrap_or(site);
    let host = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let host = if host.contains('.') {
        host
    } else {
        format!("{}.atlassian.net", host)
    };
    let name = host.strip_suffix(".atlassian.net").unwrap_or("");
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.')
    {
        return Err(Error(format!(
            "The Jira site '{}' should look like https://your-team.atlassian.net.",
            site
        )));
    }
    Ok(format!("https://{}", host))
}

pub fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let bytes = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (bytes[0] as u32) << 16 | (bytes[1] as u32) << 8 | bytes[2] as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Encodes text for a URL path segment or query value.
pub fn encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{:02X}", byte)),
        }
    }
    out
}

impl<'a> Jira<'a> {
    /// `None` when a setting is missing.
    pub fn connect(host: &'a dyn Host) -> Result<Option<Self>> {
        let (Some(site), Some(email), Some(token)) =
            (host.var(SITE)?, host.var(EMAIL)?, host.var(TOKEN)?)
        else {
            return Ok(None);
        };
        if site.trim().is_empty() || email.trim().is_empty() || token.trim().is_empty() {
            return Ok(None);
        }
        Ok(Some(Jira {
            host,
            site: normalize_site(&site)?,
            auth: format!(
                "Basic {}",
                base64(format!("{}:{}", email.trim(), token.trim()).as_bytes())
            ),
        }))
    }

    pub fn browse_url(&self, key: &str) -> String {
        format!("{}/browse/{}", self.site, encode(key))
    }

    fn send(&self, request: HttpRequest) -> Result<Value> {
        let response = self.host.http(
            &request
                .header("Authorization", self.auth.clone())
                .header("Accept", "application/json"),
        )?;
        if response.is_success() {
            return response.json();
        }
        let detail = response
            .json()
            .ok()
            .map(|body| error_text(&body))
            .unwrap_or_default();
        Err(Error(match response.status {
            401 => String::from(
                "Jira did not accept your email and API token. Check them in the plugin's settings.",
            ),
            403 => format!("Jira says you do not have permission for this. {}", detail)
                .trim()
                .to_string(),
            404 => format!("Jira could not find that. {}", detail)
                .trim()
                .to_string(),
            429 => {
                String::from("Jira is limiting requests right now. Wait a minute and try again.")
            }
            status if detail.is_empty() => format!("Jira answered with an error ({}).", status),
            _ => detail,
        }))
    }

    /// A file from the REST API, such as an attachment, as base64, or `None` when Jira did not send it.
    pub fn get_base64(&self, path: &str) -> Result<Option<String>> {
        let response = self.host.http(
            &HttpRequest::new("GET", format!("{}/rest/api/3/{}", self.site, path))
                .header("Authorization", self.auth.clone())
                .base64(),
        )?;
        Ok(response.is_success().then_some(response.body))
    }

    pub fn get(&self, path: &str) -> Result<Value> {
        self.send(HttpRequest::new(
            "GET",
            format!("{}/rest/api/3/{}", self.site, path),
        ))
    }

    pub fn post(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(HttpRequest::new("POST", format!("{}/rest/api/3/{}", self.site, path)).json(body))
    }

    pub fn put(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(HttpRequest::new("PUT", format!("{}/rest/api/3/{}", self.site, path)).json(body))
    }

    /// A path on the site outside the REST API, such as the gateway behind Jira's notification bell.
    pub fn site_get(&self, path: &str) -> Result<Value> {
        self.send(HttpRequest::new("GET", format!("{}/{}", self.site, path)))
    }

    pub fn site_post(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(HttpRequest::new("POST", format!("{}/{}", self.site, path)).json(body))
    }

    /// Jira Software's API, for boards and sprints.
    pub fn agile_get(&self, path: &str) -> Result<Value> {
        self.send(HttpRequest::new(
            "GET",
            format!("{}/rest/agile/1.0/{}", self.site, path),
        ))
    }

    pub fn agile_post(&self, path: &str, body: &Value) -> Result<Value> {
        self.send(
            HttpRequest::new("POST", format!("{}/rest/agile/1.0/{}", self.site, path)).json(body),
        )
    }
}

/// Jira's error messages, from `errorMessages` and `errors`.
pub fn error_text(body: &Value) -> String {
    let mut messages: Vec<String> = body
        .get("errorMessages")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(String::from)
                .collect()
        })
        .unwrap_or_default();
    if let Some(errors) = body.get("errors").and_then(Value::as_object) {
        for (field, message) in errors {
            if let Some(message) = message.as_str() {
                messages.push(format!("{}: {}", field, message));
            }
        }
    }
    messages.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::FakeHost;
    use taskproxy_plugin_sdk::json;

    #[test]
    fn sites_are_normalized_or_refused() {
        for site in [
            "team",
            "team.atlassian.net",
            "https://team.atlassian.net",
            "https://Team.atlassian.net/jira/software/projects",
            " http://team.atlassian.net/ ",
        ] {
            assert_eq!(
                normalize_site(site).unwrap(),
                "https://team.atlassian.net",
                "{}",
                site
            );
        }
        for site in [
            "",
            "https://example.com",
            "https://atlassian.net",
            "team.atlassian.net.evil.com",
            "a b",
        ] {
            assert!(normalize_site(site).is_err(), "{}", site);
        }
    }

    #[test]
    fn base64_matches_the_standard() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"me@x.com:tok"), "bWVAeC5jb206dG9r");
    }

    #[test]
    fn requests_carry_basic_auth_and_errors_are_readable() {
        let host = FakeHost::connected();
        host.answer(
            "GET",
            "/rest/api/3/myself",
            200,
            json!({ "accountId": "me" }),
        );
        host.answer(
            "GET",
            "/rest/api/3/issue/NOPE-1",
            404,
            json!({ "errorMessages": ["Issue does not exist"] }),
        );
        host.answer("GET", "/rest/api/3/issue/AUTH-1", 401, json!({}));
        let jira = Jira::connect(&host).unwrap().unwrap();
        assert_eq!(jira.get("myself").unwrap()["accountId"], "me");
        let sent = host.sent.borrow();
        assert_eq!(
            sent[0].headers["Authorization"],
            format!("Basic {}", base64(b"me@team.com:secret"))
        );
        drop(sent);
        assert_eq!(
            jira.get("issue/NOPE-1").unwrap_err().0,
            "Jira could not find that. Issue does not exist"
        );
        assert!(
            jira.get("issue/AUTH-1")
                .unwrap_err()
                .0
                .contains("email and API token")
        );
    }

    #[test]
    fn missing_settings_mean_no_connection() {
        let host = FakeHost::default();
        assert!(Jira::connect(&host).unwrap().is_none());
        assert_eq!(
            missing_settings(&host).unwrap(),
            vec!["Jira site", "Email", "API token"]
        );
    }

    #[test]
    fn values_are_encoded_for_urls() {
        assert_eq!(encode("PROJ-1"), "PROJ-1");
        assert_eq!(encode("a b&c/d"), "a%20b%26c%2Fd");
    }
}
