//! Jira stores rich text, such as descriptions and comments, in the Atlassian Document Format. Task Proxy shows
//! markdown, so documents are turned into markdown to read, and the markdown a user types is turned into a document.
use taskproxy_plugin_sdk::{Value, json};

/// A document as markdown. Anything this does not know is shown by its text.
pub fn to_markdown(doc: &Value) -> String {
    let mut out = String::new();
    blocks(doc.get("content"), &mut out, "");
    out.trim().to_string()
}

fn children(node: &Value) -> &[Value] {
    node.get("content")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

fn attr<'a>(node: &'a Value, name: &str) -> Option<&'a Value> {
    node.get("attrs").and_then(|attrs| attrs.get(name))
}

fn attr_str<'a>(node: &'a Value, name: &str) -> &'a str {
    attr(node, name).and_then(Value::as_str).unwrap_or("")
}

/// Block nodes, each followed by a blank line, with `prefix` before every line (for quotes and list items).
fn blocks(content: Option<&Value>, out: &mut String, prefix: &str) {
    let Some(list) = content.and_then(Value::as_array) else {
        return;
    };
    for node in list {
        let mut text = String::new();
        block(node, &mut text);
        let text = text.trim_end();
        if text.is_empty() {
            continue;
        }
        for line in text.lines() {
            out.push_str(prefix);
            out.push_str(line);
            out.push('\n');
        }
        out.push_str(prefix.trim_end());
        out.push('\n');
    }
}

fn block(node: &Value, out: &mut String) {
    match node.get("type").and_then(Value::as_str).unwrap_or("") {
        "paragraph" => out.push_str(&inline(children(node))),
        "heading" => {
            let level = attr(node, "level")
                .and_then(Value::as_u64)
                .unwrap_or(1)
                .clamp(1, 6) as usize;
            out.push_str(&format!("{} {}", "#".repeat(level), inline(children(node))));
        }
        "bulletList" | "orderedList" | "taskList" => list(node, out),
        "codeBlock" => {
            let code: String = children(node)
                .iter()
                .filter_map(|text| text.get("text").and_then(Value::as_str))
                .collect();
            out.push_str(&format!(
                "```{}\n{}\n```",
                attr_str(node, "language"),
                code.trim_end()
            ));
        }
        "blockquote" | "panel" => blocks(node.get("content"), out, "> "),
        "rule" => out.push_str("---"),
        "table" => table(node, out),
        "expand" | "nestedExpand" => {
            let title = attr_str(node, "title");
            if !title.is_empty() {
                out.push_str(&format!("**{}**\n\n", title));
            }
            blocks(node.get("content"), out, "");
        }
        "mediaSingle" | "mediaGroup" | "media" => out.push_str("*(attachment)*"),
        "blockCard" | "embedCard" => {
            let url = attr_str(node, "url");
            out.push_str(&format!("<{}>", url));
        }
        _ if node.get("content").is_some() => blocks(node.get("content"), out, ""),
        _ => out.push_str(&inline(std::slice::from_ref(node))),
    }
}

fn list(node: &Value, out: &mut String) {
    let ordered = node.get("type").and_then(Value::as_str) == Some("orderedList");
    let start = attr(node, "order").and_then(Value::as_u64).unwrap_or(1);
    for (index, item) in children(node).iter().enumerate() {
        let marker = match item.get("type").and_then(Value::as_str) {
            Some("taskItem") => {
                if attr_str(item, "state") == "DONE" {
                    "- [x] ".to_string()
                } else {
                    "- [ ] ".to_string()
                }
            }
            _ if ordered => format!("{}. ", start + index as u64),
            _ => "- ".to_string(),
        };
        let mut body = String::new();
        if item.get("type").and_then(Value::as_str) == Some("taskItem") {
            body.push_str(&inline(children(item)));
        } else {
            for child in children(item) {
                let mut text = String::new();
                block(child, &mut text);
                if !body.is_empty() {
                    body.push('\n');
                }
                body.push_str(text.trim_end());
            }
        }
        let indent = " ".repeat(marker.len());
        for (line_index, line) in body.lines().enumerate() {
            if line_index == 0 {
                out.push_str(&marker);
            } else if !line.is_empty() {
                out.push_str(&indent);
            }
            out.push_str(line);
            out.push('\n');
        }
    }
}

fn table(node: &Value, out: &mut String) {
    let rows: Vec<Vec<String>> = children(node)
        .iter()
        .map(|row| {
            children(row)
                .iter()
                .map(|cell| {
                    let mut text = String::new();
                    blocks(cell.get("content"), &mut text, "");
                    text.split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                        .replace('|', "\\|")
                })
                .collect()
        })
        .collect();
    let width = rows.iter().map(Vec::len).max().unwrap_or(0);
    if width == 0 {
        return;
    }
    for (index, row) in rows.iter().enumerate() {
        let mut cells = row.clone();
        cells.resize(width, String::new());
        out.push_str(&format!("| {} |\n", cells.join(" | ")));
        if index == 0 {
            out.push_str(&format!("|{}\n", " --- |".repeat(width)));
        }
    }
}

fn inline(nodes: &[Value]) -> String {
    let mut out = String::new();
    for node in nodes {
        match node.get("type").and_then(Value::as_str).unwrap_or("") {
            "text" => {
                let mut text = node
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let marks = node
                    .get("marks")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let mut link = None;
                for mark in &marks {
                    match mark.get("type").and_then(Value::as_str).unwrap_or("") {
                        "code" => text = format!("`{}`", text),
                        "strong" => text = format!("**{}**", text),
                        "em" => text = format!("*{}*", text),
                        "strike" => text = format!("~~{}~~", text),
                        "link" => link = Some(attr_str(mark, "href").to_string()),
                        _ => {}
                    }
                }
                match link {
                    Some(href) => out.push_str(&format!("[{}]({})", text, href)),
                    None => out.push_str(&text),
                }
            }
            "hardBreak" => out.push_str("  \n"),
            "mention" => out.push_str(&format!(
                "**{}**",
                attr_str(node, "text").trim_start_matches('@')
            )),
            "emoji" => {
                let text = attr_str(node, "text");
                out.push_str(if text.is_empty() {
                    attr_str(node, "shortName")
                } else {
                    text
                });
            }
            "inlineCard" => out.push_str(&format!("<{}>", attr_str(node, "url"))),
            "status" => out.push_str(&format!("`{}`", attr_str(node, "text"))),
            "date" => out.push_str(&format_timestamp(attr_str(node, "timestamp"))),
            _ => out.push_str(&inline(children(node))),
        }
    }
    out
}

/// A date node's timestamp, in milliseconds, as YYYY-MM-DD.
fn format_timestamp(millis: &str) -> String {
    let Ok(millis) = millis.parse::<i64>() else {
        return millis.to_string();
    };
    let days = millis.div_euclid(86_400_000);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{:04}-{:02}-{:02}", year, month, day)
}

/// Markdown as typed in a comment or description, as a document. Covers paragraphs, headings, lists, quotes,
/// code blocks, rules, and inline bold, italics, code, and links.
pub fn from_markdown(markdown: &str) -> Value {
    let lines: Vec<&str> = markdown.lines().collect();
    let mut content = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let line = lines[index];
        let trimmed = line.trim();
        if trimmed.is_empty() {
            index += 1;
            continue;
        }
        if let Some(language) = trimmed.strip_prefix("```") {
            let mut code = Vec::new();
            index += 1;
            while index < lines.len() && !lines[index].trim().starts_with("```") {
                code.push(lines[index]);
                index += 1;
            }
            index += 1;
            let mut node = json!({ "type": "codeBlock", "content": [{ "type": "text", "text": code.join("\n") }] });
            if !language.trim().is_empty() {
                node["attrs"] = json!({ "language": language.trim() });
            }
            if code.is_empty() {
                node["content"] = json!([]);
            }
            content.push(node);
            continue;
        }
        if let Some(level) = heading_level(trimmed) {
            let text = trimmed[level..].trim();
            content.push(json!({ "type": "heading", "attrs": { "level": level }, "content": inline_nodes(text) }));
            index += 1;
            continue;
        }
        if trimmed == "---" || trimmed == "***" {
            content.push(json!({ "type": "rule" }));
            index += 1;
            continue;
        }
        if trimmed.starts_with("> ") || trimmed == ">" {
            let mut quoted = Vec::new();
            while index < lines.len() && lines[index].trim().starts_with('>') {
                quoted.push(lines[index].trim().trim_start_matches('>').trim_start());
                index += 1;
            }
            let inner = from_markdown(&quoted.join("\n"));
            content.push(json!({ "type": "blockquote", "content": inner["content"] }));
            continue;
        }
        if let Some((ordered, _)) = list_item(trimmed) {
            let mut items = Vec::new();
            while index < lines.len() {
                match list_item(lines[index].trim()) {
                    Some((item_ordered, text)) if item_ordered == ordered => {
                        items.push(json!({ "type": "listItem", "content": [
                            { "type": "paragraph", "content": inline_nodes(text) }
                        ] }));
                        index += 1;
                    }
                    _ => break,
                }
            }
            let kind = if ordered { "orderedList" } else { "bulletList" };
            content.push(json!({ "type": kind, "content": items }));
            continue;
        }
        let mut paragraph = Vec::new();
        while index < lines.len() {
            let current = lines[index].trim();
            if current.is_empty()
                || current.starts_with("```")
                || heading_level(current).is_some()
                || list_item(current).is_some()
                || current.starts_with('>')
            {
                break;
            }
            paragraph.push(current);
            index += 1;
        }
        let mut nodes = Vec::new();
        for (line_index, text) in paragraph.iter().enumerate() {
            if line_index > 0 {
                nodes.push(json!({ "type": "hardBreak" }));
            }
            nodes.extend(inline_nodes(text));
        }
        content.push(json!({ "type": "paragraph", "content": nodes }));
    }
    json!({ "type": "doc", "version": 1, "content": content })
}

fn heading_level(line: &str) -> Option<usize> {
    let level = line.chars().take_while(|c| *c == '#').count();
    ((1..=6).contains(&level) && line[level..].starts_with(' ')).then_some(level)
}

/// `(ordered, text)` for `- item`, `* item`, or `1. item`.
fn list_item(line: &str) -> Option<(bool, &str)> {
    if let Some(text) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
        return Some((false, text));
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 {
        return line[digits..].strip_prefix(". ").map(|text| (true, text));
    }
    None
}

/// Inline text with `**bold**`, `*italic*` or `_italic_`, `` `code` ``, and `[text](https://...)`.
fn inline_nodes(text: &str) -> Vec<Value> {
    let mut nodes = Vec::new();
    let mut plain = String::new();
    let chars: Vec<char> = text.chars().collect();
    let mut index = 0;
    let flush = |plain: &mut String, nodes: &mut Vec<Value>| {
        if !plain.is_empty() {
            nodes.push(json!({ "type": "text", "text": std::mem::take(plain) }));
        }
    };
    let find = |from: usize, pattern: &str| -> Option<usize> {
        let rest: String = chars[from..].iter().collect();
        rest.find(pattern)
            .map(|byte| rest[..byte].chars().count() + from)
    };
    while index < chars.len() {
        let rest: String = chars[index..].iter().collect();
        let marked = if rest.starts_with("**") {
            find(index + 2, "**").map(|end| (index + 2, end, end + 2, "strong"))
        } else if rest.starts_with('`') {
            find(index + 1, "`").map(|end| (index + 1, end, end + 1, "code"))
        } else if rest.starts_with('*') || rest.starts_with('_') {
            let mark = &rest[..1];
            find(index + 1, mark).map(|end| (index + 1, end, end + 1, "em"))
        } else {
            None
        };
        if let Some((start, end, next, mark)) = marked.filter(|(start, end, ..)| end > start) {
            flush(&mut plain, &mut nodes);
            let inner: String = chars[start..end].iter().collect();
            nodes.push(json!({ "type": "text", "text": inner, "marks": [{ "type": mark }] }));
            index = next;
            continue;
        }
        if rest.starts_with('[')
            && let Some(close) = find(index, "](")
            && let Some(end) = find(close + 2, ")")
        {
            let label: String = chars[index + 1..close].iter().collect();
            let href: String = chars[close + 2..end].iter().collect();
            if href.starts_with("https://") || href.starts_with("http://") {
                flush(&mut plain, &mut nodes);
                nodes.push(json!({ "type": "text", "text": label,
                    "marks": [{ "type": "link", "attrs": { "href": href } }] }));
                index = end + 1;
                continue;
            }
        }
        plain.push(chars[index]);
        index += 1;
    }
    flush(&mut plain, &mut nodes);
    nodes
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(content: Value) -> Value {
        json!({ "type": "doc", "version": 1, "content": content })
    }

    fn text(text: &str) -> Value {
        json!({ "type": "text", "text": text })
    }

    #[test]
    fn documents_become_markdown() {
        let document = doc(json!([
            { "type": "heading", "attrs": { "level": 2 }, "content": [text("Goal")] },
            { "type": "paragraph", "content": [
                text("Fix "),
                { "type": "text", "text": "login", "marks": [{ "type": "strong" }] },
                text(" for "),
                { "type": "mention", "attrs": { "text": "@Sam Lee" } },
                { "type": "hardBreak" },
                { "type": "text", "text": "docs", "marks": [{ "type": "link", "attrs": { "href": "https://x.io" } }] }
            ] },
            { "type": "bulletList", "content": [
                { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("one")] }] },
                { "type": "listItem", "content": [
                    { "type": "paragraph", "content": [text("two")] },
                    { "type": "bulletList", "content": [
                        { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("nested")] }] }
                    ] }
                ] }
            ] },
            { "type": "orderedList", "attrs": { "order": 3 }, "content": [
                { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("third")] }] }
            ] },
            { "type": "codeBlock", "attrs": { "language": "rust" }, "content": [text("fn main() {}")] },
            { "type": "panel", "content": [{ "type": "paragraph", "content": [text("Careful")] }] },
            { "type": "table", "content": [
                { "type": "tableRow", "content": [
                    { "type": "tableHeader", "content": [{ "type": "paragraph", "content": [text("A")] }] },
                    { "type": "tableHeader", "content": [{ "type": "paragraph", "content": [text("B|C")] }] }
                ] },
                { "type": "tableRow", "content": [
                    { "type": "tableCell", "content": [{ "type": "paragraph", "content": [text("1")] }] }
                ] }
            ] },
            { "type": "paragraph", "content": [
                { "type": "date", "attrs": { "timestamp": "1762992000000" } },
                text(" "),
                { "type": "status", "attrs": { "text": "BLOCKED" } }
            ] },
            { "type": "mediaSingle", "content": [{ "type": "media", "attrs": {} }] }
        ]));
        assert_eq!(
            to_markdown(&document),
            "## Goal\n\n\
             Fix **login** for **Sam Lee**  \n[docs](https://x.io)\n\n\
             - one\n- two\n  - nested\n\n\
             3. third\n\n\
             ```rust\nfn main() {}\n```\n\n\
             > Careful\n>\n\n\
             | A | B\\|C |\n| --- | --- |\n| 1 |  |\n\n\
             2025-11-13 `BLOCKED`\n\n\
             *(attachment)*"
        );
    }

    #[test]
    fn empty_and_odd_documents_do_not_fail() {
        assert_eq!(to_markdown(&Value::Null), "");
        assert_eq!(to_markdown(&doc(json!([]))), "");
        assert_eq!(
            to_markdown(&doc(json!([{ "type": "somethingNew", "content": [
            { "type": "paragraph", "content": [text("still shown")] }
        ] }]))),
            "still shown"
        );
    }

    #[test]
    fn markdown_becomes_a_document() {
        let document = from_markdown(
            "# Title\n\nSome **bold** and *em* and `code` and [a link](https://x.io).\nNext line\n\n\
             - one\n- two\n\n1. first\n2. second\n\n> quoted\n\n```js\nlet a = 1;\n```\n\n---",
        );
        assert_eq!(
            document,
            doc(json!([
                { "type": "heading", "attrs": { "level": 1 }, "content": [text("Title")] },
                { "type": "paragraph", "content": [
                    text("Some "),
                    { "type": "text", "text": "bold", "marks": [{ "type": "strong" }] },
                    text(" and "),
                    { "type": "text", "text": "em", "marks": [{ "type": "em" }] },
                    text(" and "),
                    { "type": "text", "text": "code", "marks": [{ "type": "code" }] },
                    text(" and "),
                    { "type": "text", "text": "a link", "marks": [{ "type": "link", "attrs": { "href": "https://x.io" } }] },
                    text("."),
                    { "type": "hardBreak" },
                    text("Next line")
                ] },
                { "type": "bulletList", "content": [
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("one")] }] },
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("two")] }] }
                ] },
                { "type": "orderedList", "content": [
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("first")] }] },
                    { "type": "listItem", "content": [{ "type": "paragraph", "content": [text("second")] }] }
                ] },
                { "type": "blockquote", "content": [{ "type": "paragraph", "content": [text("quoted")] }] },
                { "type": "codeBlock", "attrs": { "language": "js" }, "content": [text("let a = 1;")] },
                { "type": "rule" }
            ]))
        );
    }

    #[test]
    fn unfinished_marks_and_other_links_stay_plain() {
        assert_eq!(
            from_markdown("2 * 3 = 6"),
            doc(json!([{ "type": "paragraph", "content": [text("2 * 3 = 6")] }]))
        );
        assert_eq!(
            from_markdown("[x](javascript:alert)"),
            doc(json!([{ "type": "paragraph", "content": [text("[x](javascript:alert)")] }]))
        );
        assert_eq!(from_markdown("  \n "), doc(json!([])));
    }

    #[test]
    fn markdown_survives_a_round_trip() {
        let markdown = "## Steps\n\n- open the page\n- click **Save**\n\nSee `config.json`.";
        assert_eq!(to_markdown(&from_markdown(markdown)), markdown);
    }
}
