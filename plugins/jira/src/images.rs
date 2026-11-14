//! Images in rich text, such as a description's screenshots. Jira keeps them as attachments that only a signed-in
//! request can read, so the plugin downloads them and sends them as data URIs, which are all Task Proxy will show.
use crate::adf;
use crate::client::{Jira, encode};
use std::collections::BTreeMap;
use taskproxy_plugin_sdk::ui::Node;
use taskproxy_plugin_sdk::{Value, host_supports};

/// At most this many images are downloaded for one field.
const MAX_IMAGES: usize = 8;
/// Larger attachments are shown by their thumbnail.
const FULL_SIZE_MAX: u64 = 2 * 1024 * 1024;
/// Types Task Proxy draws from a data URI.
const DRAWN: [&str; 7] = [
    "image/png",
    "image/jpeg",
    "image/jpg",
    "image/gif",
    "image/webp",
    "image/bmp",
    "image/avif",
];

/// The attachment ids a rendered field's images show, in order, from their `attachment/content/<id>` or
/// `secure/attachment/<id>` addresses.
fn rendered_ids(html: &str) -> Vec<String> {
    let mut ids = Vec::new();
    for tag in html.split("<img").skip(1) {
        let tag = tag.split('>').next().unwrap_or("");
        let found = [
            "attachment/content/",
            "secure/attachment/",
            "attachment/thumbnail/",
        ]
        .iter()
        .find_map(|marker| {
            let (_, rest) = tag.split_once(marker)?;
            let id: String = rest.chars().take_while(char::is_ascii_digit).collect();
            (!id.is_empty()).then_some(id)
        });
        if let Some(id) = found {
            ids.push(id);
        }
    }
    ids
}

/// Which attachment each image of `doc` is: by the rendered field's images in order when they match up, or else
/// by file name.
fn attachment_ids(doc: &Value, rendered: &str, attachments: &[Value]) -> Vec<Option<String>> {
    let media = adf::media_nodes(doc);
    let ids = rendered_ids(rendered);
    if ids.len() == media.len() {
        return ids.into_iter().map(Some).collect();
    }
    media
        .iter()
        .map(|node| {
            let alt = node
                .pointer("/attrs/alt")
                .and_then(Value::as_str)
                .unwrap_or("");
            attachments
                .iter()
                .find(|attachment| {
                    !alt.is_empty()
                        && attachment.get("filename").and_then(Value::as_str) == Some(alt)
                })
                .and_then(|attachment| attachment.get("id")?.as_str().map(String::from))
        })
        .collect()
}

/// The data URIs for a field's images, by the names `adf::to_markdown_with_media` gives them. Images that cannot be
/// read are left out, so the rest of the field still shows.
pub fn for_field(jira: &Jira, key: &str, real: &str, doc: &Value) -> BTreeMap<String, String> {
    let mut images = BTreeMap::new();
    // Task Proxy 0.1.122 and earlier send files as text, which would break them.
    if adf::media_nodes(doc).is_empty() || !host_supports(jira.host, "0.1.3") {
        return images;
    }
    let Ok(issue) = jira.get(&format!(
        "issue/{}?fields=attachment,{}&expand=renderedFields",
        encode(key),
        encode(real)
    )) else {
        return images;
    };
    let attachments: Vec<Value> = issue
        .pointer("/fields/attachment")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rendered = issue
        .pointer(&format!("/renderedFields/{}", real))
        .and_then(Value::as_str)
        .unwrap_or("");
    for (index, id) in attachment_ids(doc, rendered, &attachments)
        .into_iter()
        .enumerate()
        .take(MAX_IMAGES)
    {
        let Some(id) = id else { continue };
        let attachment = attachments
            .iter()
            .find(|attachment| attachment.get("id").and_then(Value::as_str) == Some(id.as_str()));
        let mime = attachment
            .and_then(|attachment| attachment.get("mimeType")?.as_str())
            .unwrap_or("image/png")
            .to_ascii_lowercase();
        let size = attachment
            .and_then(|attachment| attachment.get("size")?.as_u64())
            .unwrap_or(0);
        let (path, mime) = if DRAWN.contains(&mime.as_str()) && size <= FULL_SIZE_MAX {
            (
                format!("attachment/content/{}?redirect=false", encode(&id)),
                mime,
            )
        } else {
            // Jira draws a thumbnail for large images and for types Task Proxy does not draw.
            (
                format!("attachment/thumbnail/{}?redirect=false", encode(&id)),
                "image/png".to_string(),
            )
        };
        if let Ok(Some(body)) = jira.get_base64(&path) {
            let body: String = body.split_whitespace().collect();
            if !body.is_empty() {
                images.insert(
                    adf::media_name(index),
                    format!(
                        "data:{};base64,{}",
                        mime.replace("image/jpg", "image/jpeg"),
                        body
                    ),
                );
            }
        }
    }
    images
}

/// Rich text as markdown with its images, and a switch to the text as written.
pub fn rich_text(jira: &Jira, key: &str, real: &str, doc: &Value) -> Node {
    if !host_supports(jira.host, "0.1.3") {
        // Older hosts would show each image as a broken one, so they keep the note in its place.
        return taskproxy_plugin_sdk::ui::markdown(adf::to_markdown(doc));
    }
    let (text, _) = adf::to_markdown_with_media(doc);
    Node::Markdown {
        images: for_field(jira, key, real, doc),
        text: if text.is_empty() {
            "*(empty)*".into()
        } else {
            text
        },
        toggle: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::FakeHost;
    use taskproxy_plugin_sdk::json;

    fn doc() -> Value {
        json!({ "type": "doc", "content": [
            { "type": "mediaSingle", "content": [{ "type": "media", "attrs": { "id": "u1", "alt": "one.png" } }] },
            { "type": "paragraph", "content": [{ "type": "text", "text": "Between" }] },
            { "type": "mediaSingle", "content": [{ "type": "media", "attrs": { "id": "u2", "alt": "big.png" } }] }
        ] })
    }

    #[test]
    fn rendered_images_name_their_attachments_in_order() {
        let html = r#"<p><img src="/rest/api/3/attachment/content/10001" alt="one.png"></p><p>x</p><span><img alt="b" src="https://t.atlassian.net/secure/attachment/10002/big.png"></span><img src="https://elsewhere/pic.png">"#;
        assert_eq!(rendered_ids(html), vec!["10001", "10002"]);
        let attachments = vec![json!({ "id": "20001", "filename": "big.png" })];
        // When the rendered images do not match up, file names are used.
        assert_eq!(
            attachment_ids(&doc(), "", &attachments),
            vec![None, Some("20001".to_string())]
        );
    }

    #[test]
    fn images_are_downloaded_as_data_uris_once_task_proxy_can_take_them() {
        let host = FakeHost::connected();
        host.answer(
            "GET",
            "/rest/api/3/issue/P-1",
            200,
            json!({
                "fields": { "attachment": [
                    { "id": "10001", "filename": "one.png", "mimeType": "image/png", "size": 100 },
                    { "id": "10002", "filename": "big.png", "mimeType": "image/png", "size": 9_000_000 }
                ] },
                "renderedFields": { "description": "<img src=\"/rest/api/3/attachment/content/10001\"><img src=\"/rest/api/3/attachment/content/10002\">" }
            }),
        );
        host.answer(
            "GET",
            "/rest/api/3/attachment/content/10001?redirect=false",
            200,
            json!("iVBORw0K"),
        );
        host.answer(
            "GET",
            "/rest/api/3/attachment/thumbnail/10002?redirect=false",
            200,
            json!("VGh1bWI="),
        );
        let jira = Jira::connect(&host).unwrap().unwrap();
        // Task Proxy 0.1.122 and earlier would corrupt them, so none are asked for.
        assert!(for_field(&jira, "P-1", "description", &doc()).is_empty());
        assert!(host.sent.borrow().is_empty());

        *host.spec.borrow_mut() = Some("0.1.3".into());
        let images = for_field(&jira, "P-1", "description", &doc());
        assert_eq!(images["media-0"], "data:image/png;base64,iVBORw0K");
        assert_eq!(
            images["media-1"], "data:image/png;base64,VGh1bWI=",
            "a large one is its thumbnail"
        );
        let sent = host.sent.borrow();
        assert!(
            sent.iter()
                .filter(|request| request.url.contains("attachment/"))
                .all(|request| request.encoding.as_deref() == Some("base64"))
        );
    }
}
