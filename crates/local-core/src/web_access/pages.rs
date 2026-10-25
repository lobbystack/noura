//! Reads a web page for the AI: one GET through the host request boundary
//! to any public HTTPS host, then the page as plain text. The boundary
//! checks every hop and every address it resolves to, stops at the
//! redirect limit and refuses a body past the size limit; this module
//! refuses a status that isn't a success and a type that isn't text,
//! decodes the page in the character set it names, turns HTML into text
//! with its links as footnotes, and cuts the text at the content limit.
//! The request's timeout covers the whole read, turning the page into
//! text included, and a page whose elements nest too deeply is refused
//! while it parses.

use std::{
    borrow::Cow,
    cell::{Cell, RefCell},
    collections::HashMap,
    rc::Rc,
    sync::Arc,
    time::Instant,
};

use encoding_rs::{Encoding, UTF_8};
use html2text::{Handle, RcDom, config::Config, render::TextDecorator};
use html5ever::{
    Attribute, ExpandedName, ParseOpts, QualName,
    interface::{ElementFlags, NodeOrText, QuirksMode, TreeSink},
    tendril::{StrTendril, TendrilSink},
    tree_builder::TreeBuilderOpts,
};

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

/// How much of a page the parser takes at a time before Noura checks how
/// deep its elements went and how long it has taken.
const PARSE_PIECE_BYTES: usize = 4 * 1024;

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
    let deadline = Instant::now() + request.limits.request_timeout;
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
        Kind::Html => html_text(decoded, deadline).await?,
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
/// one space. A fragment Noura can't read by `deadline`, or that nests
/// too deeply, comes back empty.
pub(crate) fn plain_line(fragment: &str, deadline: Instant) -> String {
    let config = html2text::config::plain_no_decorate().allow_width_overflow();
    match convert(&config, fragment, LINE_WIDTH, deadline) {
        Ok(text) => text.split_whitespace().collect::<Vec<_>>().join(" "),
        Err(_) => String::new(),
    }
}

/// The text of an HTML page, with emphasis marked as in Markdown and its
/// links as numbered footnotes, or an error once `deadline` passes. The
/// work runs on a thread for blocking work, and the call gives up on it at
/// `deadline`: the parse checks the time as it goes and stops by itself.
async fn html_text(html: String, deadline: Instant) -> Result<String> {
    let converting = tokio::task::spawn_blocking(move || {
        let config = html2text::config::plain()
            .allow_width_overflow()
            .no_table_borders()
            .no_link_wrapping();
        convert(&config, &html, TEXT_WIDTH, deadline)
    });
    match tokio::time::timeout_at(deadline.into(), converting).await {
        Ok(Ok(converted)) => converted,
        Ok(Err(_)) => Err(unreadable("Noura couldn't read the page's text")),
        Err(_) => {
            tracing::warn!("a page the AI read took too long to turn into text");
            Err(super::timeout_error(OPERATION))
        }
    }
}

/// `html` as text through `config`, wrapped at `width`. The parse stops
/// as soon as an element nests deeper than [`MAX_NESTING`] or `deadline`
/// passes, since parsing and converting take time that grows with the
/// square of how deeply elements nest.
fn convert<D: TextDecorator>(
    config: &Config<D>,
    html: &str,
    width: usize,
    deadline: Instant,
) -> Result<String> {
    let dom = parse(html, deadline)?;
    if deepest(&dom.document) > MAX_NESTING {
        return Err(too_deep());
    }
    config
        .dom_to_render_tree(&dom)
        .and_then(|tree| config.render_to_string(tree, width))
        .map_err(|error| {
            tracing::warn!(%error, "a page the AI read didn't turn into text");
            unreadable("Noura couldn't read the page's text")
        })
}

/// `html` parsed as browsers parse it, a few kilobytes at a time. Between
/// pieces, the parse stops once an element went deeper than
/// [`MAX_NESTING`] or `deadline` passed.
fn parse(html: &str, deadline: Instant) -> Result<RcDom> {
    let opts = ParseOpts {
        tree_builder: TreeBuilderOpts {
            scripting_enabled: false,
            ..TreeBuilderOpts::default()
        },
        ..ParseOpts::default()
    };
    let mut parser = html5ever::parse_document(Capped::default(), opts);
    let mut rest = html;
    while !rest.is_empty() {
        let mut end = rest.len().min(PARSE_PIECE_BYTES);
        while !rest.is_char_boundary(end) {
            end += 1;
        }
        let (piece, after) = rest.split_at(end);
        parser.process(StrTendril::from_slice(piece));
        if parser.tokenizer.sink.sink.too_deep.get() {
            return Err(too_deep());
        }
        if Instant::now() >= deadline {
            tracing::warn!("a page the AI read took too long to parse");
            return Err(super::timeout_error(OPERATION));
        }
        rest = after;
    }
    let capped = parser.finish();
    if capped.too_deep.get() {
        return Err(too_deep());
    }
    Ok(capped.dom)
}

/// How deeply the elements under `root` nest, template contents included,
/// counted up to just past [`MAX_NESTING`].
fn deepest(root: &Handle) -> usize {
    let mut deepest = 0;
    let mut pending = vec![(root.clone(), 0_usize)];
    while let Some((node, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        if deepest > MAX_NESTING {
            break;
        }
        for child in node.children.borrow().iter() {
            if matches!(child.data, html2text::Element { .. }) {
                pending.push((child.clone(), depth + 1));
            }
        }
        if let html2text::Element {
            template_contents, ..
        } = &node.data
            && let Some(contents) = template_contents.borrow().as_ref()
        {
            pending.push((contents.clone(), depth));
        }
    }
    deepest
}

/// html2text's tree, built by html5ever, that notes when an element goes
/// in deeper than [`MAX_NESTING`]. It counts each element's depth in the
/// tree as it goes in, so a stray end tag or an element HTML nests where
/// it shouldn't can't hide how deep the tree grows, and it counts a
/// template's contents as inside the template.
#[derive(Default)]
struct Capped {
    dom: RcDom,
    /// Each template's contents, by address, and the template that holds
    /// them. Holding the template keeps the address from being reused.
    templates: RefCell<HashMap<usize, Handle>>,
    too_deep: Cell<bool>,
}

impl Capped {
    /// How deep `node` is in the tree, counted up to just past
    /// [`MAX_NESTING`].
    fn depth(&self, node: &Handle) -> usize {
        let templates = self.templates.borrow();
        let mut depth = 0;
        let mut current = node.clone();
        while depth <= MAX_NESTING {
            let Some(above) = current
                .get_parent()
                .or_else(|| templates.get(&address(&current)).cloned())
            else {
                break;
            };
            depth += 1;
            current = above;
        }
        depth
    }

    /// Note an element going in at `depth`.
    fn entering(&self, child: &NodeOrText<Handle>, depth: impl FnOnce() -> usize) {
        if !self.too_deep.get()
            && let NodeOrText::AppendNode(node) = child
            && matches!(node.data, html2text::Element { .. })
            && depth() > MAX_NESTING
        {
            self.too_deep.set(true);
        }
    }
}

fn address(node: &Handle) -> usize {
    Rc::as_ptr(node).cast::<()>() as usize
}

impl TreeSink for Capped {
    type Handle = Handle;
    type Output = Self;
    type ElemName<'a> = ExpandedName<'a>;

    fn finish(self) -> Self {
        self
    }

    // html2text reads no parse errors, so none are kept.
    fn parse_error(&self, _message: Cow<'static, str>) {}

    fn get_document(&self) -> Handle {
        self.dom.get_document()
    }

    fn elem_name<'a>(&'a self, target: &'a Handle) -> ExpandedName<'a> {
        self.dom.elem_name(target)
    }

    fn create_element(&self, name: QualName, attrs: Vec<Attribute>, flags: ElementFlags) -> Handle {
        self.dom.create_element(name, attrs, flags)
    }

    fn create_comment(&self, text: StrTendril) -> Handle {
        self.dom.create_comment(text)
    }

    fn create_pi(&self, target: StrTendril, data: StrTendril) -> Handle {
        self.dom.create_pi(target, data)
    }

    fn append(&self, parent: &Handle, child: NodeOrText<Handle>) {
        self.entering(&child, || self.depth(parent) + 1);
        self.dom.append(parent, child);
    }

    fn append_based_on_parent_node(
        &self,
        element: &Handle,
        prev_element: &Handle,
        child: NodeOrText<Handle>,
    ) {
        if element.get_parent().is_some() {
            self.append_before_sibling(element, child);
        } else {
            self.append(prev_element, child);
        }
    }

    fn append_doctype_to_document(
        &self,
        name: StrTendril,
        public_id: StrTendril,
        system_id: StrTendril,
    ) {
        self.dom
            .append_doctype_to_document(name, public_id, system_id);
    }

    fn get_template_contents(&self, target: &Handle) -> Handle {
        let contents = self.dom.get_template_contents(target);
        self.templates
            .borrow_mut()
            .entry(address(&contents))
            .or_insert_with(|| target.clone());
        contents
    }

    fn same_node(&self, x: &Handle, y: &Handle) -> bool {
        self.dom.same_node(x, y)
    }

    fn set_quirks_mode(&self, mode: QuirksMode) {
        self.dom.set_quirks_mode(mode);
    }

    fn append_before_sibling(&self, sibling: &Handle, new_node: NodeOrText<Handle>) {
        self.entering(&new_node, || self.depth(sibling));
        self.dom.append_before_sibling(sibling, new_node);
    }

    fn add_attrs_if_missing(&self, target: &Handle, attrs: Vec<Attribute>) {
        self.dom.add_attrs_if_missing(target, attrs);
    }

    fn remove_from_parent(&self, target: &Handle) {
        self.dom.remove_from_parent(target);
    }

    // A move under another element can make the tree deeper; the check of
    // the whole tree after the parse finds that.
    fn reparent_children(&self, node: &Handle, new_parent: &Handle) {
        self.dom.reparent_children(node, new_parent);
    }

    fn is_mathml_annotation_xml_integration_point(&self, handle: &Handle) -> bool {
        self.dom.is_mathml_annotation_xml_integration_point(handle)
    }
}

fn too_deep() -> CoreError {
    tracing::warn!("a page the AI read nests its markup too deeply");
    unreadable("The page's markup nests too deeply for Noura to read")
}

fn unreadable(message: &str) -> CoreError {
    CoreError::new(
        "web_page_unreadable",
        ErrorCategory::Provider,
        message,
        OPERATION,
    )
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

    /// Parsing markup and turning it into text take time that grows with
    /// the square of how deeply its elements nest, so a page that nests
    /// past the limit is refused while it parses, whatever markup builds
    /// the nesting: end tags the parser ignores, or elements it nests
    /// wherever they appear.
    #[tokio::test]
    async fn markup_nested_past_the_limit_is_refused_and_ordinary_nesting_reads() {
        let nested = |depth: usize, open: &str, close: &str| {
            format!("{}deep{}", open.repeat(depth), close.repeat(depth))
        };
        let limit = WebAccessLimits::default().max_response_bytes;
        let filled = |unit: &str| unit.repeat(limit / unit.len());
        let deep = [
            ("divs", nested(200_000, "<div>", "</div>")),
            ("lists", nested(MAX_NESTING + 1, "<ul><li>", "</ul>")),
            ("unclosed", "<blockquote>x".repeat(MAX_NESTING + 1)),
            ("stray-ends", filled("<div></i>")),
            ("ruby-bases", filled("<rb>")),
            ("ruby-texts", filled("<rt>")),
            ("ruby-containers", filled("<rtc>")),
            ("ruby-parentheses", filled("<rp>")),
            ("option-groups", filled("<optgroup>")),
            ("templates", filled("<template>")),
            ("tables", filled("<table><td>")),
        ];
        let mut pages: Vec<(String, super::super::TransportResponse)> = deep
            .iter()
            .map(|(path, body)| {
                (
                    format!("https://garden.example.com/{path}"),
                    page("text/html", body),
                )
            })
            .collect();
        pages.push((
            "https://garden.example.com/fine".into(),
            page(
                "text/html",
                &format!(
                    "<!-- <div><div> --><script>if (a<b) {{ '<div>' }}</script>\
                     <p>one<p>two<br><img src=x><input>{}",
                    nested(100, "<div class=\"a>b\">", "</div>")
                ),
            ),
        ));
        let server = FakeServer::new(
            pages
                .iter()
                .map(|(url, response)| (url.as_str(), response.clone()))
                .collect(),
        );

        for (path, body) in &deep {
            assert!(body.len() <= limit, "{path}");
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

    /// A page shallow enough to read but slow to parse, here a thousand
    /// open elements and a million end tags that each make the parser
    /// look through all of them, runs into the request's timeout, and the
    /// parse stops by itself.
    #[tokio::test]
    async fn a_page_that_takes_too_long_to_turn_into_text_times_out() {
        let limits = WebAccessLimits {
            request_timeout: Duration::from_millis(300),
            ..WebAccessLimits::default()
        };
        let slow = format!(
            "{}{}",
            "<div>".repeat(1000),
            "</p>".repeat((limits.max_response_bytes - 5000) / 4)
        );
        let server = FakeServer::new(vec![(
            "https://garden.example.com/slow",
            page("text/html", &slow),
        )]);

        let started = std::time::Instant::now();
        let error = reader(&server, limits)
            .fetch(WebFetchInput {
                url: "https://garden.example.com/slow".into(),
            })
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_request_timeout");
        assert!(started.elapsed() < Duration::from_secs(2));
        let started = std::time::Instant::now();
        let Err(error) = parse(&slow, Instant::now()) else {
            panic!("the parse should stop at its deadline");
        };
        assert_eq!(error.code, "web_request_timeout");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn depth(html: &str) -> Result<usize> {
        parse(html, Instant::now() + Duration::from_secs(10)).map(|dom| deepest(&dom.document))
    }

    #[test]
    fn nesting_counts_elements_in_the_tree_the_parser_builds() {
        // The parser adds `html` and `body`, and closes `p` by itself.
        assert_eq!(depth("<div><p>a<p>b</div>").unwrap(), 4);
        assert_eq!(
            depth("<div><div></div><div><span>x</span></div></div>").unwrap(),
            5
        );
        assert_eq!(depth("<br><img src=x><hr/><svg><path/></svg>").unwrap(), 4);
        assert_eq!(depth("</div></div><div>").unwrap(), 3);
        // A stray end tag doesn't close the element it is in.
        assert_eq!(depth(&"<div></i>".repeat(10)).unwrap(), 12);
        // A template's contents count as inside it. These go in `head`.
        assert_eq!(depth(&"<template>".repeat(10)).unwrap(), 12);
        let shallow = format!("{}x", "<div>".repeat(MAX_NESTING - 2));
        assert_eq!(depth(&shallow).unwrap(), MAX_NESTING);
        let deep = format!("{}x", "<div>".repeat(MAX_NESTING - 1));
        assert_eq!(depth(&deep).unwrap_err().code, "web_page_unreadable");
    }

    #[test]
    fn a_snippet_loses_its_markup_and_extra_white_space() {
        let later = Instant::now() + Duration::from_secs(10);
        assert_eq!(
            plain_line(
                "Grow <strong>tomatoes</strong> &amp; basil\n  in   sun",
                later
            ),
            "Grow tomatoes & basil in sun"
        );
        assert_eq!(plain_line("", later), "");
        assert_eq!(plain_line(&"<rb>".repeat(5000), later), "");
    }
}
