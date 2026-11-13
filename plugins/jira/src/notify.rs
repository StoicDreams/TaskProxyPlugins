//! Jira's notifications, for Task Proxy's Alerts panel and the Notifications page.
//!
//! Jira Cloud has no public API for its notification bell. The bell's own feed is an internal gateway API, which
//! is tried first: when it answers, these are Jira's own notifications, and closing one marks it read in Jira.
//! When it refuses, as it may for API tokens, notifications are built from the public API instead: mentions,
//! comments, assignments, and status changes from the last week on issues you are assigned, reported, or watch.
//! Those are marked read in Task Proxy only.
use crate::adf;
use crate::client::{Jira, encode};
use crate::pages::{self, open_issue};
use taskproxy_plugin_sdk::ui::{self, Action, Node};
use taskproxy_plugin_sdk::{
    ActionRequest, ActionResult, Error, Map, Notification, NotificationLink, Result, Value, json,
};

const SOURCE_KEY: &str = "notify.source";
const READ_KEY: &str = "notify.read";
const CLOUD_KEY: &str = "cloudId";
/// After the bell refuses, it is tried again an hour later.
const BELL_RETRY_MS: u64 = 60 * 60 * 1000;
/// How far back recent activity looks.
const ACTIVITY_DAYS: u64 = 7;
const DAY_MS: u64 = 24 * 60 * 60 * 1000;
/// Read activity ids kept, newest last.
const READ_KEPT: usize = 500;
const MOST: usize = 30;
const BELL_PREFIX: &str = "bell:";
const BELL_PATH: &str = "gateway/api/notification-log/api/3/notifications";

/// Where the notifications last came from, for the Notifications page.
#[derive(Debug, PartialEq)]
pub enum Source {
    Bell,
    Activity(String),
}

fn source_record(jira: &Jira) -> Result<Option<Value>> {
    Ok(jira
        .host
        .store_get(SOURCE_KEY)?
        .filter(|record| record.get("site").and_then(Value::as_str) == Some(jira.site.as_str())))
}

fn bell_refused_recently(jira: &Jira) -> Result<bool> {
    let now = jira.host.now_ms()?;
    Ok(source_record(jira)?.is_some_and(|record| {
        record.get("bell") == Some(&json!(false))
            && record
                .get("at")
                .and_then(Value::as_u64)
                .is_some_and(|at| now.saturating_sub(at) < BELL_RETRY_MS)
    }))
}

fn record_source(jira: &Jira, bell: bool, reason: &str) -> Result<()> {
    jira.host.store_set(
        SOURCE_KEY,
        Some(
            json!({ "site": jira.site, "at": jira.host.now_ms()?, "bell": bell, "reason": reason }),
        ),
    )
}

/// The current notifications, newest first: Jira's bell when it answers, otherwise recent activity.
pub fn list(jira: &Jira) -> Result<(Source, Vec<Notification>)> {
    if !bell_refused_recently(jira)? {
        match bell(jira) {
            Ok(items) => {
                record_source(jira, true, "")?;
                return Ok((Source::Bell, items));
            }
            Err(err) => record_source(jira, false, &err.0)?,
        }
    }
    let reason = source_record(jira)?
        .and_then(|record| {
            record
                .get("reason")
                .and_then(Value::as_str)
                .map(String::from)
        })
        .unwrap_or_default();
    Ok((Source::Activity(reason), activity(jira)?))
}

/// The site's cloud id, which the bell's feed asks for.
fn cloud_id(jira: &Jira) -> Result<String> {
    let key = format!("{}:{}", CLOUD_KEY, jira.site);
    if let Some(id) = jira
        .host
        .store_get(&key)?
        .and_then(|value| value.as_str().map(String::from))
    {
        return Ok(id);
    }
    let id = jira
        .site_get("_edge/tenant_info")?
        .get("cloudId")
        .and_then(Value::as_str)
        .ok_or_else(|| Error::new("Jira did not give its cloud id."))?
        .to_string();
    jira.host.store_set(&key, Some(json!(id)))?;
    Ok(id)
}

fn text_at<'a>(value: &'a Value, paths: &[&str]) -> &'a str {
    paths
        .iter()
        .find_map(|path| value.pointer(path).and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .unwrap_or("")
}

/// The issue a link or title names, such as ATL-142 in `https://team.atlassian.net/browse/ATL-142?focused...`.
fn issue_in(url: &str, title: &str) -> Option<String> {
    if let Some(rest) = url.split("/browse/").nth(1) {
        let key: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if let Some(found) = pages::issue_key(&key) {
            return Some(found);
        }
    }
    if let Some(query) = url.split("selectedIssue=").nth(1) {
        let key: String = query.chars().take_while(|c| *c != '&').collect();
        if let Some(found) = pages::issue_key(&key) {
            return Some(found);
        }
    }
    pages::issue_key(title)
}

/// Text on one line for an alert, which shows plain text, so markdown's marks are left out.
fn snippet(text: &str, most: usize) -> String {
    let plain = text.replace("**", "").replace('`', "");
    let line = plain.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= most {
        line
    } else {
        format!("{}...", chars[..most].iter().collect::<String>().trim_end())
    }
}

fn link_to(key: &str) -> NotificationLink {
    let mut params = Map::new();
    params.insert("key".into(), json!(key));
    NotificationLink {
        text: key.to_string(),
        page: "issue".into(),
        params,
    }
}

/// The unread notifications from Jira's bell. Any answer it cannot read is an error, so activity is used instead.
fn bell(jira: &Jira) -> Result<Vec<Notification>> {
    let cloud = cloud_id(jira)?;
    let found = jira.site_get(&format!(
        "{}?cloudId={}&direction=forward&includeContent=true&limit={}&product=jira&readState=unread",
        BELL_PATH,
        encode(&cloud),
        MOST
    ))?;
    let list = found
        .get("data")
        .or_else(|| found.get("notifications"))
        .or_else(|| found.get("values"))
        .or(Some(&found))
        .and_then(Value::as_array)
        .ok_or_else(|| {
            Error::new("Jira's notification bell answered in a way this plugin does not know.")
        })?;
    let mut items = Vec::new();
    for item in list {
        let Some(id) = item.get("id").and_then(|id| {
            id.as_str()
                .map(String::from)
                .or_else(|| id.as_u64().map(|n| n.to_string()))
        }) else {
            continue;
        };
        if text_at(item, &["/readState", "/content/readState"]).eq_ignore_ascii_case("read") {
            continue;
        }
        let message = text_at(
            item,
            &["/content/message", "/content/title", "/message", "/title"],
        );
        let actor = text_at(item, &["/content/actor/displayName", "/actor/displayName"]);
        let entity = text_at(
            item,
            &[
                "/content/entity/title",
                "/entity/title",
                "/content/entity/name",
            ],
        );
        let url = text_at(
            item,
            &[
                "/content/entity/link/url",
                "/content/url",
                "/entity/link/url",
                "/url",
            ],
        );
        if message.is_empty() && entity.is_empty() {
            continue;
        }
        let title = match (message, actor) {
            ("", "") => entity.to_string(),
            ("", actor) => format!("{} on {}", actor, entity),
            (message, actor) if !actor.is_empty() && !message.contains(actor) => {
                format!("{} {}", actor, message)
            }
            (message, _) => message.to_string(),
        };
        let body = item
            .pointer("/content/bodyItems/0/document/data")
            .map(|data| match data {
                Value::String(text) => serde_json::from_str::<Value>(text)
                    .map(|doc| adf::to_markdown(&doc))
                    .unwrap_or_else(|_| text.clone()),
                doc => adf::to_markdown(doc),
            })
            .unwrap_or_default();
        let text = match (entity, body.trim()) {
            (entity, "") => entity.to_string(),
            ("", body) => snippet(body, 160),
            (entity, body) => format!("{}: {}", entity, snippet(body, 160)),
        };
        let key = issue_in(url, &format!("{} {}", entity, title));
        items.push(Notification {
            id: format!("{}{}", BELL_PREFIX, id),
            title,
            text: if text == message { String::new() } else { text },
            theme: "info".into(),
            time: text_at(item, &["/timestamp", "/created"]).to_string(),
            links: key.as_deref().map(link_to).into_iter().collect(),
        });
    }
    Ok(items)
}

/// Days since 1970 for a civil date, from Howard Hinnant's algorithm.
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Milliseconds since 1970 for Jira's `2026-11-11T15:30:00.000+0000`.
pub fn timestamp_ms(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 19 || bytes[10] != b'T' {
        return None;
    }
    let num = |from: usize, to: usize| text.get(from..to)?.parse::<i64>().ok();
    let days = days_from_civil(num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let mut seconds = days * 86_400 + num(11, 13)? * 3600 + num(14, 16)? * 60 + num(17, 19)?;
    // The zone, as +0000 or -05:00, after the seconds and any fraction.
    let rest = &text[19..];
    if let Some(at) = rest.find(['+', '-']) {
        let zone: String = rest[at + 1..]
            .chars()
            .filter(char::is_ascii_digit)
            .collect();
        if zone.len() == 4 {
            let offset =
                zone[..2].parse::<i64>().ok()? * 3600 + zone[2..].parse::<i64>().ok()? * 60;
            seconds -= if &rest[at..at + 1] == "+" {
                offset
            } else {
                -offset
            };
        }
    }
    u64::try_from(seconds * 1000).ok()
}

fn mentions(node: &Value, account: &str) -> bool {
    if node.get("type").and_then(Value::as_str) == Some("mention")
        && node.pointer("/attrs/id").and_then(Value::as_str) == Some(account)
    {
        return true;
    }
    node.get("content")
        .and_then(Value::as_array)
        .is_some_and(|children| children.iter().any(|child| mentions(child, account)))
}

fn read_ids(jira: &Jira) -> Result<Vec<String>> {
    Ok(jira
        .host
        .store_get(READ_KEY)?
        .and_then(|value| serde_json::from_value(value).ok())
        .unwrap_or_default())
}

fn mark_activity_read(jira: &Jira, ids: &[String]) -> Result<()> {
    let mut read = read_ids(jira)?;
    for id in ids {
        if !read.contains(id) {
            read.push(id.clone());
        }
    }
    if read.len() > READ_KEPT {
        read.drain(..read.len() - READ_KEPT);
    }
    jira.host
        .store_set(READ_KEY, Some(serde_json::to_value(read)?))
}

/// Notifications built from the public API: what others did in the last week on issues you are assigned,
/// reported, or watch.
fn activity(jira: &Jira) -> Result<Vec<Notification>> {
    let me = pages::me(jira)?;
    let since = jira.host.now_ms()?.saturating_sub(ACTIVITY_DAYS * DAY_MS);
    let read = read_ids(jira)?;
    let found = jira.post(
        "search/jql",
        &json!({
            "jql": format!(
                "(assignee = currentUser() OR reporter = currentUser() OR watcher = currentUser()) AND updated >= -{}d ORDER BY updated DESC",
                ACTIVITY_DAYS
            ),
            "maxResults": MOST,
            "fields": ["summary", "comment"],
            "expand": "changelog",
        }),
    )?;
    let mut items: Vec<(u64, Notification)> = Vec::new();
    for issue in found
        .get("issues")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        let key = issue.get("key").and_then(Value::as_str).unwrap_or("");
        if key.is_empty() {
            continue;
        }
        let summary = issue
            .pointer("/fields/summary")
            .and_then(Value::as_str)
            .unwrap_or("");
        let about = format!("{} {}", key, summary);
        let mut add = |id: String, at: u64, title: String, text: String, theme: &str| {
            if at < since || read.contains(&id) {
                return;
            }
            items.push((
                at,
                Notification {
                    id,
                    title,
                    text,
                    theme: theme.into(),
                    time: crate::fields::short_date(&format_ms(at)),
                    links: vec![link_to(key)],
                },
            ));
        };
        for comment in issue
            .pointer("/fields/comment/comments")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let author_id = comment
                .pointer("/author/accountId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if author_id == me {
                continue;
            }
            let Some(at) = comment
                .get("created")
                .and_then(Value::as_str)
                .and_then(timestamp_ms)
            else {
                continue;
            };
            let author = comment
                .pointer("/author/displayName")
                .and_then(Value::as_str)
                .unwrap_or("Someone");
            let body = comment.get("body").unwrap_or(&Value::Null);
            let id = comment.get("id").and_then(Value::as_str).unwrap_or("");
            let verb = if mentions(body, &me) {
                "mentioned you on"
            } else {
                "commented on"
            };
            add(
                format!("comment:{}", id),
                at,
                format!("{} {} {}", author, verb, key),
                format!("{}: {}", summary, snippet(&adf::to_markdown(body), 160)),
                if verb.starts_with("mentioned") {
                    "warning"
                } else {
                    "info"
                },
            );
        }
        for history in issue
            .pointer("/changelog/histories")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let author_id = history
                .pointer("/author/accountId")
                .and_then(Value::as_str)
                .unwrap_or("");
            if author_id == me {
                continue;
            }
            let Some(at) = history
                .get("created")
                .and_then(Value::as_str)
                .and_then(timestamp_ms)
            else {
                continue;
            };
            let author = history
                .pointer("/author/displayName")
                .and_then(Value::as_str)
                .unwrap_or("Someone");
            let history_id = history.get("id").and_then(Value::as_str).unwrap_or("");
            for change in history
                .get("items")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or(&[])
            {
                let field = change.get("field").and_then(Value::as_str).unwrap_or("");
                let to = change.get("toString").and_then(Value::as_str).unwrap_or("");
                let to_id = change.get("to").and_then(Value::as_str).unwrap_or("");
                let title = match field {
                    "assignee" if to_id == me => format!("{} assigned {} to you", author, key),
                    "status" if !to.is_empty() => format!("{} moved {} to {}", author, key, to),
                    _ => continue,
                };
                add(
                    format!("change:{}:{}", history_id, field),
                    at,
                    title,
                    about.clone(),
                    "info",
                );
            }
        }
    }
    items.sort_by_key(|item| std::cmp::Reverse(item.0));
    Ok(items.into_iter().take(MOST).map(|(_, item)| item).collect())
}

/// `2026-11-11T15:30:00.000+0000` for milliseconds since 1970, in UTC.
fn format_ms(ms: u64) -> String {
    let seconds = (ms / 1000) as i64;
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    // Howard Hinnant's civil_from_days.
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.000+0000",
        year,
        month,
        day,
        rest / 3600,
        rest % 3600 / 60,
        rest % 60
    )
}

/// Marks a notification read: in Jira for one from its bell, otherwise in Task Proxy.
pub fn mark_read(jira: &Jira, ids: &[String]) -> Result<()> {
    let (bell, activity): (Vec<&String>, Vec<&String>) =
        ids.iter().partition(|id| id.starts_with(BELL_PREFIX));
    if !bell.is_empty() {
        let ids: Vec<&str> = bell.iter().map(|id| &id[BELL_PREFIX.len()..]).collect();
        jira.site_post(&format!("{}/mark/read", BELL_PATH), &json!({ "ids": ids }))
            .map_err(|err| {
                Error(format!(
                    "Jira did not mark the notification read. {}",
                    err.0
                ))
            })?;
    }
    if !activity.is_empty() {
        let ids: Vec<String> = activity.into_iter().cloned().collect();
        mark_activity_read(jira, &ids)?;
    }
    Ok(())
}

/// The Notifications page: where they come from, and each one with a way to open its issue or mark it read.
pub fn page(jira: &Jira) -> Result<Node> {
    let (source, items) = list(jira)?;
    let mut children = vec![match &source {
        Source::Bell => ui::notice(
            "These are Jira's own notifications. Marking one read here, or closing it in Task Proxy's alerts, marks it read in Jira too.",
            "info",
        ),
        Source::Activity(reason) => ui::notice(
            format!(
                "Jira's notification bell did not answer this plugin{}, so these are built from the last {} days of \
                 activity on issues you are assigned, reported, or watch. Marking them read only changes Task Proxy.",
                if reason.is_empty() {
                    String::new()
                } else {
                    format!(" ({})", reason)
                },
                ACTIVITY_DAYS
            ),
            "warning",
        ),
    }];
    let mut tools = vec![ui::button(
        Action::new("notify.refresh", "Refresh").icon("refresh"),
        "",
    )];
    if !items.is_empty() {
        tools.push(ui::button(
            Action::new("notify.readAll", "Mark all read").param(
                "ids",
                json!(items.iter().map(|item| item.id.clone()).collect::<Vec<_>>()),
            ),
            "",
        ));
    }
    if matches!(source, Source::Activity(_)) {
        tools.push(ui::button(
            Action::new("notify.retryBell", "Try Jira's bell again"),
            "",
        ));
    }
    children.push(ui::toolbar(tools));
    if items.is_empty() {
        children.push(ui::empty("No unread notifications."));
    }
    for item in &items {
        let mut line = vec![ui::strong(item.title.clone())];
        if !item.time.is_empty() {
            line.push(ui::muted(crate::fields::short_date(&item.time)));
        }
        let mut actions = Vec::new();
        for link in &item.links {
            if let Some(key) = link.params.get("key").and_then(Value::as_str) {
                actions.push(ui::link(format!("Open {}", key), open_issue(key)));
            }
        }
        actions.push(ui::link(
            "Mark read",
            Action::new("notify.read", "Mark read").param("ids", json!([item.id])),
        ));
        let mut stack = vec![ui::row(line)];
        if !item.text.is_empty() {
            stack.push(ui::text(item.text.clone()));
        }
        stack.push(ui::row(actions));
        children.push(ui::stack(stack));
    }
    Ok(ui::stack(children))
}

/// Answers the Notifications page's actions, or `None` when the action is not one of them.
pub fn action(jira: &Jira, request: &ActionRequest) -> Result<Option<ActionResult>> {
    let ids = || -> Vec<String> {
        request
            .params
            .get("ids")
            .and_then(Value::as_array)
            .map(|list| {
                list.iter()
                    .filter_map(|id| id.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    };
    Ok(Some(match request.action.as_str() {
        "notify.refresh" => ActionResult::refresh(),
        "notify.read" | "notify.readAll" => {
            mark_read(jira, &ids())?;
            ActionResult::refresh()
        }
        "notify.retryBell" => {
            jira.host.store_set(SOURCE_KEY, None)?;
            ActionResult::refresh()
        }
        _ => return Ok(None),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::FakeHost;

    fn connected() -> FakeHost {
        let host = FakeHost::connected();
        host.answer(
            "GET",
            "/rest/api/3/myself",
            200,
            json!({ "accountId": "me-1" }),
        );
        host.answer(
            "GET",
            "/_edge/tenant_info",
            200,
            json!({ "cloudId": "cloud-9" }),
        );
        host
    }

    #[test]
    fn jira_times_become_milliseconds_and_back() {
        assert_eq!(timestamp_ms("1970-01-01T00:00:00.000+0000"), Some(0));
        assert_eq!(
            timestamp_ms("2026-11-11T15:30:00.000+0000"),
            Some(1_794_411_000_000)
        );
        assert_eq!(
            timestamp_ms("2026-11-11T10:30:00.000-0500"),
            Some(1_794_411_000_000)
        );
        assert_eq!(
            timestamp_ms("2026-11-11T16:30:00.000+01:00"),
            Some(1_794_411_000_000)
        );
        assert_eq!(timestamp_ms("not a date"), None);
        assert_eq!(format_ms(1_794_411_000_000), "2026-11-11T15:30:00.000+0000");
    }

    #[test]
    fn the_bell_is_used_when_jira_answers() {
        let host = connected();
        host.answer("GET", "/gateway/api/notification-log/api/3/notifications", 200, json!({ "data": [
            { "id": "n-1", "timestamp": "2026-11-11T15:30:00.000Z", "readState": "unread", "content": {
                "message": "mentioned you in a comment", "actor": { "displayName": "Sam Lee" },
                "entity": { "title": "ATL-142: Show CI status", "link": { "url": "https://team.atlassian.net/browse/ATL-142?focusedCommentId=1" } },
                "bodyItems": [{ "document": { "format": "ADF", "data": "{\"type\":\"doc\",\"content\":[{\"type\":\"paragraph\",\"content\":[{\"type\":\"text\",\"text\":\"Can you look?\"}]}]}" } }]
            } },
            { "id": "n-2", "readState": "read", "content": { "message": "old" } }
        ] }));
        host.answer(
            "POST",
            "/gateway/api/notification-log/api/3/notifications/mark/read",
            204,
            Value::Null,
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        let (source, items) = list(&jira).unwrap();
        assert_eq!(source, Source::Bell);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "bell:n-1");
        assert_eq!(items[0].title, "Sam Lee mentioned you in a comment");
        assert_eq!(items[0].text, "ATL-142: Show CI status: Can you look?");
        assert_eq!(items[0].links, vec![link_to("ATL-142")]);
        assert!(
            host.sent
                .borrow()
                .iter()
                .any(|sent| sent.url.contains("cloudId=cloud-9"))
        );
        mark_read(&jira, &["bell:n-1".to_string()]).unwrap();
        let sent = host.sent.borrow();
        let post = sent.iter().find(|sent| sent.method == "POST").unwrap();
        assert_eq!(post.body.as_deref(), Some("{\"ids\":[\"n-1\"]}"));
    }

    #[test]
    fn activity_is_used_when_the_bell_refuses_and_is_marked_read_here() {
        let host = connected();
        host.now
            .set(timestamp_ms("2026-11-12T12:00:00.000+0000").unwrap());
        host.answer(
            "GET",
            "/gateway/api/notification-log/api/3/notifications",
            401,
            json!({}),
        );
        let doc = |content: Value| json!({ "type": "doc", "content": [{ "type": "paragraph", "content": content }] });
        host.answer("POST", "/rest/api/3/search/jql", 200, json!({ "issues": [{
            "key": "ATL-142", "fields": { "summary": "Show CI status", "comment": { "comments": [
                { "id": "11", "author": { "accountId": "sam", "displayName": "Sam Lee" }, "created": "2026-11-11T15:30:00.000+0000",
                  "body": doc(json!([{ "type": "text", "text": "Over to " }, { "type": "mention", "attrs": { "id": "me-1", "text": "@Me" } }])) },
                { "id": "12", "author": { "accountId": "me-1", "displayName": "Me" }, "created": "2026-11-11T16:00:00.000+0000", "body": doc(json!([])) },
                { "id": "13", "author": { "accountId": "pri", "displayName": "Priya" }, "created": "2026-10-01T10:00:00.000+0000", "body": doc(json!([])) }
            ] } },
            "changelog": { "histories": [
                { "id": "h1", "author": { "accountId": "pri", "displayName": "Priya" }, "created": "2026-11-12T09:00:00.000+0000",
                  "items": [{ "field": "assignee", "to": "me-1", "toString": "Me" }, { "field": "status", "toString": "In Review" }, { "field": "labels", "toString": "x" }] }
            ] }
        }] }));
        let jira = Jira::connect(&host).unwrap().unwrap();
        let (source, items) = list(&jira).unwrap();
        assert!(
            matches!(source, Source::Activity(ref reason) if reason.contains("email and API token")),
            "{:?}",
            source
        );
        let titles: Vec<&str> = items.iter().map(|item| item.title.as_str()).collect();
        assert_eq!(
            titles,
            vec![
                "Priya assigned ATL-142 to you",
                "Priya moved ATL-142 to In Review",
                "Sam Lee mentioned you on ATL-142"
            ]
        );
        assert_eq!(items[2].text, "Show CI status: Over to Me");
        assert_eq!(items[2].theme, "warning");
        mark_read(&jira, &[items[2].id.clone()]).unwrap();
        let (_, left) = list(&jira).unwrap();
        assert_eq!(left.len(), 2);
        // The bell is not asked again for an hour after it refused.
        let bell_asks = host
            .sent
            .borrow()
            .iter()
            .filter(|sent| sent.url.contains("notification-log"))
            .count();
        assert_eq!(bell_asks, 1);
        let page = serde_json::to_string(&page(&jira).unwrap()).unwrap();
        assert!(
            page.contains("did not answer this plugin") && page.contains("Try Jira's bell again"),
            "{}",
            page
        );
    }
}
