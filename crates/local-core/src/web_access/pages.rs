//! Reads a web page for the AI: one GET through the host request boundary
//! to any public HTTPS host, then the page as plain text. The boundary
//! checks every hop and every address it resolves to, stops at the
//! redirect limit and refuses a body past the size limit; this module
//! refuses a status that isn't a success and a type that isn't text,
//! decodes the page in the character set it names, turns HTML into text
//! with its links as footnotes, and cuts the text at the content limit.

use std::sync::Arc;

use encoding_rs::{Encoding, UTF_8};

use super::{
    HostRequest, HostRequests, WebFetchRequest, WebFetchResponse, WebMethod, WebTransport,
};
use crate::{CoreError, ErrorCategory, Result};

const OPERATION: &str = "web_fetch";

/// What a page read asks for: HTML or plain text.
const ACCEPT: &str =
    "text/html, application/xhtml+xml;q=0.9, text/plain;q=0.8, text/markdown;q=0.8";

/// How many bytes at the start of an HTML page Noura looks through for
/// the character set a `<meta>` names, as browsers do.
const META_PRESCAN_BYTES: usize = 1024;

/// The width the text of an HTML page wraps at.
const TEXT_WIDTH: usize = 100;

/// The width a one-line fragment renders at before its lines join.
const LINE_WIDTH: usize = 1000;

/// How deeply a page's elements may nest. Pages people read nest a few
/// dozen deep; converting one nested this deep takes milliseconds.
const MAX_NESTING: usize = 1024;

/// The kinds of page Noura reads.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Html,
    Text,
}

/// Read the page `request` names over `transport`, as text.
pub(crate) async fn fetch(
    transport: Arc<dyn WebTransport>,
    request: WebFetchRequest,
) -> Result<WebFetchResponse> {
    let requests = HostRequests::new(transport, request.limits)?;
    let response = requests
        .send_public(HostRequest {
            method: WebMethod::Get,
            url: request.url.into(),
            headers: vec![
                ("accept".to_owned(), ACCEPT.to_owned()),
                ("user-agent".to_owned(), "Noura".to_owned()),
            ],
            body: Vec::new(),
        })
        .await
        .map_err(|mut error| {
            error.operation = OPERATION.to_owned();
            error
        })?;
    if !(200..300).contains(&response.status) {
        tracing::warn!(status = response.status, "a page the AI read didn't answer");
        return Err(CoreError::new(
            "web_page_unavailable",
            ErrorCategory::Provider,
            format!("The site answered with HTTP status {}", response.status),
            OPERATION,
        ));
    }
    let content_type = header(&response.headers, "content-type").unwrap_or_default();
    let (media_type, charset) = media_type(content_type);
    let kind = match media_type.as_str() {
        "text/html" | "application/xhtml+xml" => Kind::Html,
        "text/plain" | "text/markdown" => Kind::Text,
        _ => {
            return Err(CoreError::new(
                "web_content_type_unsupported",
                ErrorCategory::Provider,
                "Noura reads only HTML and plain-text pages",
                OPERATION,
            ));
        }
    };
    let decoded = decode(&response.body, charset.as_deref(), kind);
    let text = match kind {
        Kind::Html => html_text(&decoded)?,
        Kind::Text => decoded,
    };
    let (content, truncated) = cut(text, request.limits.max_content_bytes);
    Ok(WebFetchResponse {
        url: response.url,
        status: response.status,
        content_type: Some(media_type),
        content,
        response_bytes: response.body.len(),
        truncated,
    })
}

/// The text of an HTML fragment on one line, as for a search result's
/// title or snippet: tags gone, entities decoded, runs of white space as
/// one space.
pub(crate) fn plain_line(fragment: &str) -> String {
    let text = html2text::config::plain_no_decorate()
        .allow_width_overflow()
        .string_from_read(fragment.as_bytes(), LINE_WIDTH)
        .unwrap_or_else(|_| fragment.to_owned());
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The text of an HTML page, with emphasis marked as in Markdown and its
/// links as numbered footnotes. The conversion's time grows with the
/// square of how deeply elements nest, so a page nested past
/// [`MAX_NESTING`] is refused first.
fn html_text(html: &str) -> Result<String> {
    if nesting(html) > MAX_NESTING {
        tracing::warn!("a page the AI read nests its markup too deeply");
        return Err(CoreError::new(
            "web_page_unreadable",
            ErrorCategory::Provider,
            "The page's markup nests too deeply for Noura to read",
            OPERATION,
        ));
    }
    html2text::config::plain()
        .allow_width_overflow()
        .no_table_borders()
        .no_link_wrapping()
        .string_from_read(html.as_bytes(), TEXT_WIDTH)
        .map_err(|error| {
            tracing::warn!(%error, "a page the AI read didn't turn into text");
            CoreError::new(
                "web_page_unreadable",
                ErrorCategory::Provider,
                "Noura couldn't read the page's text",
                OPERATION,
            )
        })
}

/// How deeply the elements of `html` nest, counting only elements that
/// hold others until their end tag: void elements, self-closing tags and
/// the elements HTML closes by itself, such as `p` and `li`, don't count,
/// and comments and the text of `script`, `style`, `textarea` and
/// `title` are skipped. It reads tags the simple way, so it can only
/// overcount what a parser would nest.
fn nesting(html: &str) -> usize {
    /// Elements that hold no content, or that HTML closes when the next
    /// one of their kind or their container's end comes.
    const UNCOUNTED: &[&str] = &[
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr", "p", "li", "dt", "dd", "tr", "td", "th", "option", "optgroup",
        "thead", "tbody", "tfoot", "colgroup", "caption", "rb", "rt", "rtc", "rp", "html", "head",
        "body",
    ];
    /// Elements whose text holds no tags.
    const RAW_TEXT: &[&str] = &["script", "style", "textarea", "title", "xmp"];
    let bytes = html.as_bytes();
    // ASCII lowercase keeps every byte where it was.
    let lower = html.to_ascii_lowercase();
    let (mut at, mut depth, mut deepest) = (0, 0_usize, 0);
    while let Some(offset) = bytes[at..].iter().position(|byte| *byte == b'<') {
        at += offset + 1;
        if lower[at..].starts_with("!--") {
            at = lower[at..]
                .find("-->")
                .map_or(bytes.len(), |end| at + end + 3);
            continue;
        }
        let closing = bytes.get(at) == Some(&b'/');
        let name_start = at + usize::from(closing);
        let name_end = bytes[name_start..]
            .iter()
            .position(|byte| !byte.is_ascii_alphanumeric() && *byte != b'-')
            .map_or(bytes.len(), |end| name_start + end);
        if name_end == name_start {
            continue;
        }
        let name = &lower[name_start..name_end];
        let (tag_end, self_closing) = tag_end(bytes, name_end);
        at = tag_end;
        if UNCOUNTED.contains(&name) {
            continue;
        }
        if RAW_TEXT.contains(&name) {
            if !closing {
                let close = format!("</{name}");
                at = lower[at..].find(&close).map_or(bytes.len(), |end| at + end);
            }
        } else if closing {
            depth = depth.saturating_sub(1);
        } else if !self_closing {
            depth += 1;
            deepest = deepest.max(depth);
        }
    }
    deepest
}

/// Where the tag whose attributes start at `from` ends, past its `>`, and
/// whether it ends with `/>`. A `>` inside a quoted attribute value
/// doesn't end it.
fn tag_end(bytes: &[u8], from: usize) -> (usize, bool) {
    let mut quote = None;
    let mut previous = 0;
    for (index, byte) in bytes.iter().enumerate().skip(from) {
        match (quote, *byte) {
            (Some(open), byte) if byte == open => quote = None,
            (Some(_), _) => {}
            (None, b'"' | b'\'') => quote = Some(*byte),
            (None, b'>') => return (index + 1, previous == b'/'),
            _ => {}
        }
        previous = *byte;
    }
    (bytes.len(), false)
}

/// A `Content-Type` value's media type, in lowercase, and the character
/// set it names.
fn media_type(value: &str) -> (String, Option<String>) {
    let mut parts = value.split(';');
    let media = parts.next().unwrap_or_default().trim().to_ascii_lowercase();
    let charset = parts.find_map(|parameter| {
        let (name, value) = parameter.split_once('=')?;
        name.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| value.trim().trim_matches('"').to_owned())
    });
    (media, charset)
}

/// `body` as text: in the character set a byte-order mark, the header or,
/// for HTML, a `<meta>` near the start names, and UTF-8 otherwise. Bytes
/// that don't fit become U+FFFD.
fn decode(body: &[u8], charset: Option<&str>, kind: Kind) -> String {
    let named = charset
        .map(str::to_owned)
        .or_else(|| (kind == Kind::Html).then(|| meta_charset(body)).flatten())
        .and_then(|label| Encoding::for_label_no_replacement(label.as_bytes()))
        .unwrap_or(UTF_8);
    let (text, _, _) = named.decode(body);
    text.into_owned()
}

/// The character set a `<meta charset>` or `<meta http-equiv>` names near
/// the start of an HTML page.
fn meta_charset(body: &[u8]) -> Option<String> {
    let start = &body[..body.len().min(META_PRESCAN_BYTES)];
    let lower = String::from_utf8_lossy(start).to_ascii_lowercase();
    let at = lower.find("charset=")? + "charset=".len();
    let label: String = lower[at..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.' | ':')
        })
        .collect();
    (!label.is_empty()).then_some(label)
}

/// `text` cut to at most `limit` bytes at a character boundary, and
/// whether it was cut.
fn cut(mut text: String, limit: usize) -> (String, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    (text, true)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::web_access::{
        BraveSearch, WebAccessLimits, WebAccessProviderConfig, WebAccessService, WebFetchInput,
        test_server::{FakeServer, page, redirect},
    };

    fn reader(server: &Arc<FakeServer>, limits: WebAccessLimits) -> WebAccessService {
        let mut service = WebAccessService::new(limits).unwrap();
        service
            .configure(
                WebAccessProviderConfig {
                    id: "brave-search".into(),
                    user_consent_granted: true,
                },
                Arc::new(BraveSearch::with_transport("key", server.clone())),
            )
            .unwrap();
        service
    }

    async fn read(server: &Arc<FakeServer>, url: &str) -> Result<WebFetchResponse> {
        reader(server, WebAccessLimits::default())
            .fetch(WebFetchInput { url: url.into() })
            .await
    }

    const ARTICLE: &str = "<!doctype html><html><head><title>Tomatoes</title>\
        <script>steal()</script><style>p{}</style></head><body>\
        <h1>Growing tomatoes</h1><p>Plant them in <b>full sun</b>.</p>\
        <p>Read <a href=\"https://garden.example.org/guide\">the guide</a>.</p></body></html>";

    #[tokio::test]
    async fn a_page_comes_back_as_text_with_the_address_that_answered() {
        let server = FakeServer::new(vec![
            (
                "https://garden.example.com/start",
                redirect(301, "/tomatoes"),
            ),
            (
                "https://garden.example.com/tomatoes",
                page("text/html; charset=utf-8", ARTICLE),
            ),
        ]);

        let response = read(&server, "https://garden.example.com/start")
            .await
            .unwrap();

        assert_eq!(response.url, "https://garden.example.com/tomatoes");
        assert_eq!(response.content_type.as_deref(), Some("text/html"));
        assert!(!response.truncated);
        let text = &response.content;
        assert!(text.contains("Growing tomatoes"), "{text}");
        assert!(text.contains("Plant them in **full sun**."), "{text}");
        assert!(text.contains("https://garden.example.org/guide"), "{text}");
        assert!(!text.contains("steal()") && !text.contains("<p>"), "{text}");
        let hop = &server.hops()[1];
        assert!(
            hop.headers
                .iter()
                .any(|(name, value)| name == "accept" && value.starts_with("text/html"))
        );
    }

    #[tokio::test]
    async fn plain_text_pages_come_back_as_they_are_in_the_character_set_they_name() {
        let server = FakeServer::new(vec![
            (
                "https://notes.example.com/plain.txt",
                page("text/plain", "line one\n<b>not a tag</b>\n"),
            ),
            (
                "https://notes.example.com/latin.html",
                page_bytes("text/html; charset=windows-1252", b"<p>caf\xe9</p>"),
            ),
            (
                "https://notes.example.com/meta.html",
                page_bytes(
                    "text/html",
                    b"<html><head><meta charset=\"iso-8859-1\"></head><p>na\xefve</p>",
                ),
            ),
        ]);

        let plain = read(&server, "https://notes.example.com/plain.txt")
            .await
            .unwrap();
        assert_eq!(plain.content, "line one\n<b>not a tag</b>\n");
        let latin = read(&server, "https://notes.example.com/latin.html")
            .await
            .unwrap();
        assert!(latin.content.contains("café"), "{}", latin.content);
        let meta = read(&server, "https://notes.example.com/meta.html")
            .await
            .unwrap();
        assert!(meta.content.contains("naïve"), "{}", meta.content);
    }

    fn page_bytes(content_type: &str, body: &[u8]) -> super::super::TransportResponse {
        super::super::TransportResponse {
            status: 200,
            headers: vec![("content-type".into(), content_type.into())],
            body: body.to_vec(),
        }
    }

    #[tokio::test]
    async fn only_https_pages_on_public_hosts_are_read() {
        let server = FakeServer::new(Vec::new());

        for (url, code) in [
            ("http://garden.example.com/", "web_url_not_https"),
            ("ftp://garden.example.com/", "web_url_not_https"),
            ("file:///etc/passwd", "web_url_not_https"),
            ("https://localhost/", "web_url_unsafe_host"),
            ("https://127.0.0.1/", "web_url_unsafe_host"),
            (
                "https://169.254.169.254/latest/meta-data",
                "web_url_unsafe_host",
            ),
            ("https://[::1]/", "web_url_unsafe_host"),
            ("https://[fe80::1]/", "web_url_unsafe_host"),
            ("https://192.168.1.1/", "web_url_unsafe_host"),
            ("https://user:pass@garden.example.com/", "web_url_invalid"),
        ] {
            let error = read(&server, url).await.unwrap_err();
            assert_eq!(error.code, code, "{url}");
        }
        assert!(server.lookups().is_empty());
        assert!(server.hops().is_empty());
    }

    #[tokio::test]
    async fn a_host_that_resolves_to_a_private_address_is_never_reached() {
        let server = FakeServer::new(Vec::new());

        for host in [
            "intranet.example.com",
            "loopback.example.com",
            "link-local.example.com",
            "mixed.example.com",
        ] {
            let error = read(&server, &format!("https://{host}/"))
                .await
                .unwrap_err();
            assert_eq!(error.code, "web_url_unsafe_host", "{host}");
        }
        assert!(server.hops().is_empty());
    }

    #[tokio::test]
    async fn every_redirect_hop_is_checked_before_noura_connects_to_it() {
        let server = FakeServer::new(vec![
            (
                "https://garden.example.com/to-plain",
                redirect(302, "http://garden.example.com/plain"),
            ),
            (
                "https://garden.example.com/to-loopback",
                redirect(302, "https://loopback.example.com/"),
            ),
            (
                "https://garden.example.com/to-metadata",
                redirect(307, "https://169.254.169.254/latest/meta-data"),
            ),
            (
                "https://garden.example.com/to-private",
                redirect(308, "https://10.0.0.8/admin"),
            ),
        ]);

        for (path, code) in [
            ("to-plain", "web_url_not_https"),
            ("to-loopback", "web_url_unsafe_host"),
            ("to-metadata", "web_url_unsafe_host"),
            ("to-private", "web_url_unsafe_host"),
        ] {
            let error = read(&server, &format!("https://garden.example.com/{path}"))
                .await
                .unwrap_err();
            assert_eq!(error.code, code, "{path}");
        }
        assert!(
            server
                .hops()
                .iter()
                .all(|hop| hop.url.host_str() == Some("garden.example.com")),
            "{:?}",
            server.hops()
        );
    }

    #[tokio::test]
    async fn redirects_stop_at_the_boundarys_limit() {
        let server = FakeServer::new(vec![
            ("https://garden.example.com/1", redirect(302, "/2")),
            ("https://garden.example.com/2", redirect(302, "/3")),
            ("https://garden.example.com/3", redirect(302, "/4")),
            ("https://garden.example.com/4", redirect(302, "/5")),
            ("https://garden.example.com/5", page("text/plain", "far")),
        ]);

        let error = read(&server, "https://garden.example.com/1")
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_redirect_limit_exceeded");
        assert_eq!(
            server.hops().len(),
            WebAccessLimits::default().max_redirects + 1
        );
    }

    #[tokio::test]
    async fn a_page_past_the_size_limit_or_of_another_type_is_refused() {
        let limits = WebAccessLimits::default();
        let server = FakeServer::new(vec![
            (
                "https://garden.example.com/huge",
                page("text/plain", &"x".repeat(limits.max_response_bytes + 1)),
            ),
            (
                "https://garden.example.com/guide.pdf",
                page("application/pdf", "%PDF-1.7"),
            ),
            ("https://garden.example.com/photo", page("image/png", "png")),
            (
                "https://garden.example.com/untyped",
                super::super::TransportResponse {
                    status: 200,
                    headers: Vec::new(),
                    body: b"what am I".to_vec(),
                },
            ),
            (
                "https://garden.example.com/gone",
                super::super::TransportResponse {
                    status: 404,
                    headers: vec![("content-type".into(), "text/html".into())],
                    body: b"<p>secret server detail</p>".to_vec(),
                },
            ),
        ]);

        for (path, code) in [
            ("huge", "web_response_limit_exceeded"),
            ("guide.pdf", "web_content_type_unsupported"),
            ("photo", "web_content_type_unsupported"),
            ("untyped", "web_content_type_unsupported"),
            ("gone", "web_page_unavailable"),
        ] {
            let error = read(&server, &format!("https://garden.example.com/{path}"))
                .await
                .unwrap_err();
            assert_eq!(error.code, code, "{path}");
            assert!(!error.message.contains("secret"), "{}", error.message);
        }
    }

    #[tokio::test]
    async fn text_past_the_content_limit_is_cut_at_a_character_boundary() {
        let limits = WebAccessLimits {
            max_content_bytes: 10,
            ..WebAccessLimits::default()
        };
        let server = FakeServer::new(vec![(
            "https://garden.example.com/long",
            page("text/plain", "ééééééééééé"),
        )]);

        let response = reader(&server, limits)
            .fetch(WebFetchInput {
                url: "https://garden.example.com/long".into(),
            })
            .await
            .unwrap();

        assert!(response.truncated);
        assert_eq!(response.content, "ééééé");
    }

    #[tokio::test]
    async fn a_page_that_answers_too_slowly_times_out() {
        let limits = WebAccessLimits {
            request_timeout: Duration::from_millis(50),
            ..WebAccessLimits::default()
        };
        let server = FakeServer::slow(
            vec![(
                "https://garden.example.com/slow",
                page("text/plain", "late"),
            )],
            Duration::from_secs(5),
        );

        let error = reader(&server, limits)
            .fetch(WebFetchInput {
                url: "https://garden.example.com/slow".into(),
            })
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_request_timeout");
        assert!(error.retryable);
    }

    /// Turning markup into text takes time that grows with the square of
    /// how deeply its elements nest, so a page nested past the limit is
    /// refused before the conversion starts.
    #[tokio::test]
    async fn markup_nested_past_the_limit_is_refused_and_ordinary_nesting_reads() {
        let nested = |depth: usize, open: &str, close: &str| {
            format!("{}deep{}", open.repeat(depth), close.repeat(depth))
        };
        let server = FakeServer::new(vec![
            (
                "https://garden.example.com/deep",
                page("text/html", &nested(200_000, "<div>", "</div>")),
            ),
            (
                "https://garden.example.com/lists",
                page("text/html", &nested(MAX_NESTING + 1, "<ul><li>", "</ul>")),
            ),
            (
                "https://garden.example.com/unclosed",
                page("text/html", &"<blockquote>x".repeat(MAX_NESTING + 1)),
            ),
            (
                "https://garden.example.com/fine",
                page(
                    "text/html",
                    &format!(
                        "<!-- <div><div> --><script>if (a<b) {{ '<div>' }}</script>\
                         <p>one<p>two<br><img src=x><input>{}",
                        nested(100, "<div class=\"a>b\">", "</div>")
                    ),
                ),
            ),
        ]);

        for path in ["deep", "lists", "unclosed"] {
            let started = std::time::Instant::now();
            let error = read(&server, &format!("https://garden.example.com/{path}"))
                .await
                .unwrap_err();
            assert_eq!(error.code, "web_page_unreadable", "{path}");
            assert!(started.elapsed() < Duration::from_secs(2), "{path}");
        }
        let fine = read(&server, "https://garden.example.com/fine")
            .await
            .unwrap();
        assert!(fine.content.contains("deep"), "{}", fine.content);
    }

    #[test]
    fn nesting_counts_elements_that_contain_others() {
        assert_eq!(nesting("<div><p>a<p>b</div>"), 1);
        assert_eq!(
            nesting("<div><div></div><div><span>x</span></div></div>"),
            3
        );
        assert_eq!(nesting("<br><img src=x><hr/><svg><path/></svg>"), 1);
        assert_eq!(
            nesting("<style>div > a {}</style><textarea><div></textarea>"),
            0
        );
        assert_eq!(nesting("<!-- <div> --><div title='>'><b>x</b></div>"), 2);
        assert_eq!(nesting("</div></div><div>"), 1);
    }

    #[test]
    fn a_snippet_loses_its_markup_and_extra_white_space() {
        assert_eq!(
            plain_line("Grow <strong>tomatoes</strong> &amp; basil\n  in   sun"),
            "Grow tomatoes & basil in sun"
        );
        assert_eq!(plain_line(""), "");
    }
}
