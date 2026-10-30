//! The encrypted share viewer (docs/workspace-format/sync-v2.md §24): it
//! verifies a snapshot's signature with the verify key from the link's
//! fragment, decrypts it with the link key, and renders the note's Markdown
//! through a strict allowlist of elements, leaving images and raw HTML out.
//! The browser side in `browser.rs` fetches the snapshot and fills the page.

use pulldown_cmark::{Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::ShareBody;
use sync_protocol::records::{open_share, parse_share_fragment, share_token_hash};
use sync_protocol::types::{from_base64url, to_hex};

/// A snapshot that opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Opened {
    /// The note's title.
    pub title: String,
    /// The note's Markdown as allowlisted HTML.
    pub html: String,
    /// `snapshot_revision`.
    pub revision: u64,
}

/// Why the viewer shows an error page and renders nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewError {
    /// The address names no link.
    BadLink,
    /// The fragment holds no link key and verify key.
    BadKey,
    /// A bad signature, tampered ciphertext or wrong key.
    Broken,
}

impl ViewError {
    /// What the error page says.
    pub fn text(self) -> &'static str {
        match self {
            ViewError::BadLink => {
                "This address isn't a share link. Check that you copied all of it."
            }
            ViewError::BadKey => {
                "This link is missing part of its key. Check that you copied all of it."
            }
            ViewError::Broken => {
                "This note can't be opened. Its signature or key doesn't match, so nothing is shown."
            }
        }
    }
}

/// The token of a viewer page's path, `/s/{token}`.
pub fn token_of(path: &str) -> Result<[u8; 16], ViewError> {
    path.strip_prefix("/s/")
        .and_then(|token| from_base64url::<16>(token).ok())
        .ok_or(ViewError::BadLink)
}

/// The local storage key under which the viewer remembers the highest
/// `snapshot_revision` it showed for a token: the token's hash, so storage
/// never holds the token itself.
pub fn revision_key(token: &[u8; 16]) -> String {
    format!(
        "noura.share.revision.{}",
        to_hex(&share_token_hash(token).0)
    )
}

/// What the viewer warns about when a visit shows a lower
/// `snapshot_revision` than one it showed before.
pub fn revision_warning(highest_shown: Option<u64>, shown: u64) -> Option<String> {
    highest_shown.filter(|&highest| shown < highest).map(|highest| {
        format!(
            "This is an older version of the note than you saw before (version {shown}, after version {highest}). The server may be showing an old copy."
        )
    })
}

/// Opens the body of `GET /v2/shares/{token}` with the keys in the
/// fragment, the part after `#`.
pub fn open_snapshot(token: &[u8; 16], body: &[u8], fragment: &str) -> Result<Opened, ViewError> {
    let (link_key, verify_key) = parse_share_fragment(fragment).map_err(|_| ViewError::BadKey)?;
    let body = ShareBody::from_bytes(body).map_err(|_| ViewError::Broken)?;
    let (share, payload) = open_share(&body.share.0.to_bytes(), &link_key, &verify_key)
        .map_err(|_| ViewError::Broken)?;
    if share.body.token_hash != share_token_hash(token) {
        return Err(ViewError::Broken);
    }
    let markdown = String::from_utf8_lossy(&payload.markdown.0);
    Ok(Opened {
        title: payload.title.0,
        html: render(&markdown),
        revision: share.body.snapshot_revision,
    })
}

fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Whether a link may point at `url`: web and mail addresses, and anchors
/// on the page. Every other scheme, `javascript:` among them, renders as
/// plain text.
fn allowed_link(url: &str) -> bool {
    let lower = url.trim().to_ascii_lowercase();
    lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("mailto:")
        || lower.starts_with('#')
}

fn heading(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "h1",
        HeadingLevel::H2 => "h2",
        HeadingLevel::H3 => "h3",
        HeadingLevel::H4 => "h4",
        HeadingLevel::H5 => "h5",
        HeadingLevel::H6 => "h6",
    }
}

/// Renders Markdown as HTML made only of an allowlist of elements: no
/// attribute but a checked link's `href` and `rel` and an ordered list's
/// `start`, no raw HTML, and no images. Text is always escaped.
pub fn render(markdown: &str) -> String {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_YAML_STYLE_METADATA_BLOCKS;
    let mut out = String::new();
    // Links that render as text close with no `</a>`.
    let mut links: Vec<bool> = Vec::new();
    let mut image_depth = 0usize;
    let mut metadata = false;
    let mut in_head = false;
    for event in Parser::new_ext(markdown, options) {
        if image_depth > 0 {
            match event {
                Event::Start(Tag::Image { .. }) => image_depth += 1,
                Event::End(TagEnd::Image) => image_depth -= 1,
                _ => {}
            }
            continue;
        }
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => out.push_str("<p>"),
                Tag::Heading { level, .. } => {
                    out.push('<');
                    out.push_str(heading(level));
                    out.push('>');
                }
                Tag::BlockQuote(_) => out.push_str("<blockquote>"),
                Tag::CodeBlock(_) => out.push_str("<pre><code>"),
                Tag::List(Some(start)) if start != 1 => {
                    out.push_str(&format!("<ol start=\"{start}\">"));
                }
                Tag::List(Some(_)) => out.push_str("<ol>"),
                Tag::List(None) => out.push_str("<ul>"),
                Tag::Item => out.push_str("<li>"),
                Tag::Emphasis => out.push_str("<em>"),
                Tag::Strong => out.push_str("<strong>"),
                Tag::Strikethrough => out.push_str("<del>"),
                Tag::Link { dest_url, .. } => {
                    let allowed = allowed_link(&dest_url);
                    if allowed {
                        out.push_str("<a href=\"");
                        escape(&dest_url, &mut out);
                        out.push_str("\" rel=\"noopener noreferrer nofollow\">");
                    }
                    links.push(allowed);
                }
                Tag::Image { .. } => image_depth = 1,
                Tag::Table(_) => out.push_str("<table>"),
                Tag::TableHead => {
                    in_head = true;
                    out.push_str("<thead><tr>");
                }
                Tag::TableRow => out.push_str("<tr>"),
                Tag::TableCell => out.push_str(if in_head { "<th>" } else { "<td>" }),
                Tag::MetadataBlock(_) => metadata = true,
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => out.push_str("</p>"),
                TagEnd::Heading(level) => {
                    out.push_str("</");
                    out.push_str(heading(level));
                    out.push('>');
                }
                TagEnd::BlockQuote(_) => out.push_str("</blockquote>"),
                TagEnd::CodeBlock => out.push_str("</code></pre>"),
                TagEnd::List(true) => out.push_str("</ol>"),
                TagEnd::List(false) => out.push_str("</ul>"),
                TagEnd::Item => out.push_str("</li>"),
                TagEnd::Emphasis => out.push_str("</em>"),
                TagEnd::Strong => out.push_str("</strong>"),
                TagEnd::Strikethrough => out.push_str("</del>"),
                TagEnd::Link => {
                    if links.pop() == Some(true) {
                        out.push_str("</a>");
                    }
                }
                TagEnd::Table => out.push_str("</tbody></table>"),
                TagEnd::TableHead => {
                    in_head = false;
                    out.push_str("</tr></thead><tbody>");
                }
                TagEnd::TableRow => out.push_str("</tr>"),
                TagEnd::TableCell => out.push_str(if in_head { "</th>" } else { "</td>" }),
                TagEnd::MetadataBlock(_) => metadata = false,
                _ => {}
            },
            Event::Text(text) if !metadata => escape(&text, &mut out),
            Event::Code(code) => {
                out.push_str("<code>");
                escape(&code, &mut out);
                out.push_str("</code>");
            }
            Event::SoftBreak => out.push('\n'),
            Event::HardBreak => out.push_str("<br>"),
            Event::Rule => out.push_str("<hr>"),
            Event::TaskListMarker(done) => out.push_str(if done { "☑ " } else { "☐ " }),
            Event::FootnoteReference(name) => {
                out.push('[');
                escape(&name, &mut out);
                out.push(']');
            }
            Event::InlineMath(text) | Event::DisplayMath(text) => escape(&text, &mut out),
            // Raw HTML stays out.
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use sync_protocol::codec::{Bytes, Nested, Text};
    use sync_protocol::crypto::{SigningKey, random_nonce};
    use sync_protocol::records::{Share, SharePayload, share_fragment};
    use sync_protocol::types::SecretKey;

    struct Link {
        token: [u8; 16],
        key: SecretKey,
        signing: SigningKey,
    }

    fn link() -> Link {
        Link {
            token: [7; 16],
            key: SecretKey::random().unwrap(),
            signing: SigningKey::random().unwrap(),
        }
    }

    fn body(link: &Link, revision: u64, markdown: &str) -> Vec<u8> {
        let share = Share::seal(
            share_token_hash(&link.token),
            revision,
            &SharePayload {
                title: Text("Plan".into()),
                markdown: Bytes(markdown.as_bytes().to_vec()),
            },
            &link.key,
            &link.signing,
            random_nonce().unwrap(),
        )
        .unwrap();
        ShareBody {
            share: Nested(share),
        }
        .to_bytes()
    }

    fn fragment(link: &Link) -> String {
        share_fragment(&link.key, &link.signing.public())
    }

    #[test]
    fn a_snapshot_opens_with_its_fragment() {
        let link = link();
        let opened = open_snapshot(
            &link.token,
            &body(&link, 3, "# Plan\n\nMay."),
            &fragment(&link),
        )
        .unwrap();
        assert_eq!(opened.title, "Plan");
        assert_eq!(opened.revision, 3);
        assert_eq!(opened.html, "<h1>Plan</h1><p>May.</p>");
    }

    #[test]
    fn a_wrong_key_bad_signature_or_tampered_ciphertext_opens_nothing() {
        let link = link();
        let good = body(&link, 1, "Secret");
        let other = self::link();
        assert_eq!(
            open_snapshot(&link.token, &good, &fragment(&other)),
            Err(ViewError::Broken)
        );
        let wrong_key = share_fragment(&other.key, &link.signing.public());
        assert_eq!(
            open_snapshot(&link.token, &good, &wrong_key),
            Err(ViewError::Broken)
        );
        let mut tampered = good.clone();
        let at = tampered.len() - 80;
        tampered[at] ^= 1;
        assert_eq!(
            open_snapshot(&link.token, &tampered, &fragment(&link)),
            Err(ViewError::Broken)
        );
        assert_eq!(
            open_snapshot(&[8; 16], &good, &fragment(&link)),
            Err(ViewError::Broken),
            "a snapshot of another link"
        );
        assert_eq!(
            open_snapshot(&link.token, &good, "short"),
            Err(ViewError::BadKey)
        );
    }

    #[test]
    fn markdown_renders_through_the_allowlist() {
        let html = render(
            "---\nid: note_1\n---\n\n## Steps\n\n- [x] *one* **two** ~~three~~ `four`\n\n\
             <script>alert(1)</script>\n\n<b onclick=x>bold</b>\n\n\
             ![tracker](https://example.com/p.png) [site](https://example.com) \
             [bad](javascript:alert(1)) [\"q](https://e.com/\"onmouseover=\"x)\n\n\
             | a | b |\n|---|---|\n| 1 | <i>2</i> |\n",
        );
        assert!(!html.contains("note_1"), "{html}");
        assert!(!html.contains("<script"), "{html}");
        assert!(!html.contains("onclick"), "{html}");
        assert!(
            !html.contains("<img") && !html.contains("tracker"),
            "{html}"
        );
        assert!(!html.contains("javascript:"), "{html}");
        assert!(!html.contains("\"onmouseover"), "{html}");
        assert!(html.contains("<h2>Steps</h2>"), "{html}");
        assert!(
            html.contains("☑ <em>one</em> <strong>two</strong> <del>three</del> <code>four</code>")
        );
        assert!(html.contains(
            "<a href=\"https://example.com\" rel=\"noopener noreferrer nofollow\">site</a>"
        ));
        assert!(html.contains("bad"), "the link's text stays: {html}");
        assert!(html.contains("<thead><tr><th>a</th><th>b</th></tr></thead><tbody><tr><td>1</td>"));
        let allowed = [
            "p",
            "h1",
            "h2",
            "h3",
            "h4",
            "h5",
            "h6",
            "blockquote",
            "pre",
            "code",
            "ol",
            "ul",
            "li",
            "em",
            "strong",
            "del",
            "a",
            "table",
            "thead",
            "tbody",
            "tr",
            "th",
            "td",
            "br",
            "hr",
        ];
        for tag in html.split('<').skip(1) {
            let name: String = tag
                .trim_start_matches('/')
                .chars()
                .take_while(char::is_ascii_alphanumeric)
                .collect();
            assert!(allowed.contains(&name.as_str()), "<{name}> in {html}");
        }
    }

    #[test]
    fn paths_and_revisions() {
        let link = link();
        let token = sync_protocol::types::to_base64url(&link.token);
        assert_eq!(token_of(&format!("/s/{token}")), Ok(link.token));
        assert_eq!(token_of("/s/short"), Err(ViewError::BadLink));
        assert_eq!(token_of("/account"), Err(ViewError::BadLink));
        assert!(!revision_key(&link.token).contains(&token));
        assert_eq!(revision_warning(None, 1), None);
        assert_eq!(revision_warning(Some(2), 2), None);
        assert_eq!(revision_warning(Some(2), 3), None);
        assert!(revision_warning(Some(3), 2).is_some());
    }
}
