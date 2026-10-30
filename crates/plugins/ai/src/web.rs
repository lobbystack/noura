//! Web search and page reads, as tools the model may call. The plugin
//! offers them only while Noura says web search works in the workspace,
//! answers the model's calls through the host AI interface, which sends
//! each request for it, and returns each answer to the model as JSON,
//! which Noura writes to the chat as the call's result. A failure goes
//! back to the model as an error with Noura's code and words, so the chat
//! goes on without the web.

use plugin_sdk::{
    Error,
    ai::{self, Run, ToolCall, ToolDefinition, ToolResult, WebError},
};
use serde_json::{Value, json};

/// The tool that searches the web.
pub(crate) const SEARCH: &str = "web_search";

/// The tool that reads a page.
pub(crate) const FETCH: &str = "web_fetch";

/// The most calls the plugin answers in one turn of the model's. Each
/// later one gets an error. Each call may wait up to Noura's 15-second
/// web timeout, and the plugin's call may take two minutes in all.
const MAX_CALLS: usize = 5;

/// The most results a search asks for.
const MAX_RESULTS: u64 = 10;

/// The most bytes of a page's text the model gets, about 10,000 tokens,
/// so one page leaves room in the model's context for the chat.
const MAX_PAGE_TEXT: usize = 40 * 1024;

/// The tools the model may call: web search and page reads, while Noura
/// says they work in this workspace, and none otherwise.
pub(crate) fn tools() -> Vec<ToolDefinition> {
    if !ai::web_ready().unwrap_or(false) {
        return Vec::new();
    }
    vec![
        ToolDefinition {
            name: SEARCH.into(),
            description: "Search the web with Brave Search. Returns each result's title, \
                          address and snippet. Cite the addresses you use."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "What to search for." },
                    "count": {
                        "type": "integer",
                        "minimum": 1,
                        "maximum": MAX_RESULTS,
                        "description": "How many results to return, 10 at most."
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            })
            .to_string(),
        },
        ToolDefinition {
            name: FETCH.into(),
            description: "Read a web page at an https address and return its text with the \
                          address that answered."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The page's https address." }
                },
                "required": ["url"],
                "additionalProperties": false
            })
            .to_string(),
        },
    ]
}

/// Answer each tool call of `run`, which completed asking for them, and
/// let the model go on in the same run, offered `tools`.
pub(crate) fn answer(run: &Run, tools: &[ToolDefinition]) -> Result<Run, Error> {
    let results: Vec<ToolResult> = run
        .tool_calls
        .iter()
        .enumerate()
        .map(|(index, call)| ToolResult {
            call_id: call.id.clone(),
            name: call.name.clone(),
            content: if index < MAX_CALLS {
                call_tool(call)
            } else {
                error(
                    "tool_call_limit",
                    "The model asked for too many web calls at once.",
                )
            },
        })
        .collect();
    ai::answer_tools(&run.id, &results, tools)
}

/// What one call answers, as JSON text.
fn call_tool(call: &ToolCall) -> String {
    let arguments: Value = serde_json::from_str(&call.arguments).unwrap_or(Value::Null);
    match call.name.as_str() {
        SEARCH => {
            let Some(query) = arguments.get("query").and_then(Value::as_str) else {
                return error("tool_arguments_invalid", "A search needs a query.");
            };
            let count = arguments
                .get("count")
                .and_then(Value::as_u64)
                .map(|count| count.clamp(1, MAX_RESULTS) as u32);
            match ai::web_search(query, count) {
                Ok(results) => json!({
                    "query": query,
                    "results": results
                        .iter()
                        .map(|found| json!({
                            "title": found.title,
                            "url": found.url,
                            "snippet": found.snippet,
                        }))
                        .collect::<Vec<_>>(),
                })
                .to_string(),
                Err(failure) => web_error(&failure),
            }
        }
        FETCH => {
            let Some(url) = arguments.get("url").and_then(Value::as_str) else {
                return error("tool_arguments_invalid", "A page read needs an address.");
            };
            match ai::web_fetch(url) {
                Ok(page) => {
                    let (text, cut) = cut(&page.text, MAX_PAGE_TEXT);
                    json!({
                        "url": page.url,
                        "contentType": page.content_type,
                        "truncated": page.truncated || cut,
                        "text": text,
                    })
                    .to_string()
                }
                Err(failure) => web_error(&failure),
            }
        }
        other => error(
            "tool_unknown",
            &format!("The AI plugin has no tool named {other}."),
        ),
    }
}

/// `text` cut to at most `limit` bytes at a character boundary, and
/// whether it was cut.
fn cut(text: &str, limit: usize) -> (&str, bool) {
    if text.len() <= limit {
        return (text, false);
    }
    let mut end = limit;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (&text[..end], true)
}

fn web_error(failure: &WebError) -> String {
    match failure {
        WebError::Failed(failure) => error(&failure.code, &failure.message),
        WebError::Refused(refusal) => error("web_call_refused", &crate::reason(refusal)),
    }
}

fn error(code: &str, message: &str) -> String {
    json!({ "error": { "code": code, "message": message } }).to_string()
}

/// What a web tool call's arguments or result show on the page: the
/// query or address it asked for, the sources a search found, or why it
/// failed.
pub(crate) enum Shown {
    /// A search for this query.
    Searched(String),
    /// A read of this address.
    Read(String),
    /// The sources a search found, as titles and addresses.
    Sources(Vec<(String, String)>),
    /// A page that came back, which its call already shows.
    Page,
    /// A call that failed, with Noura's words.
    Failed(String),
}

/// What a tool call message's arguments show, if they are a web call's.
pub(crate) fn shown_call(content: &str) -> Option<Shown> {
    let arguments: Value = serde_json::from_str(content).ok()?;
    if let Some(query) = arguments.get("query").and_then(Value::as_str) {
        return Some(Shown::Searched(query.to_owned()));
    }
    let url = arguments.get("url").and_then(Value::as_str)?;
    Some(Shown::Read(url.to_owned()))
}

/// What a tool result message shows, if it is a web call's.
pub(crate) fn shown_result(content: &str) -> Option<Shown> {
    let result: Value = serde_json::from_str(content).ok()?;
    if let Some(message) = result.pointer("/error/message").and_then(Value::as_str) {
        return Some(Shown::Failed(message.to_owned()));
    }
    if let Some(results) = result.get("results").and_then(Value::as_array) {
        let sources = results
            .iter()
            .filter_map(|found| {
                let url = found.get("url").and_then(Value::as_str)?;
                let title = found
                    .get("title")
                    .and_then(Value::as_str)
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or(url);
                Some((title.to_owned(), url.to_owned()))
            })
            .collect();
        return Some(Shown::Sources(sources));
    }
    (result.get("text").is_some() && result.get("url").is_some()).then_some(Shown::Page)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_text_is_cut_at_a_character_boundary() {
        assert_eq!(cut("tomato", 10), ("tomato", false));
        assert_eq!(cut("ééé", 5), ("éé", true));
        assert_eq!(cut("ééé", 4), ("éé", true));
    }

    #[test]
    fn a_web_call_shows_its_query_or_address_and_a_search_its_sources() {
        assert!(matches!(
            shown_call(r#"{"query":"tomato care"}"#),
            Some(Shown::Searched(query)) if query == "tomato care"
        ));
        assert!(matches!(
            shown_call(r#"{"url":"https://garden.example.com/"}"#),
            Some(Shown::Read(url)) if url == "https://garden.example.com/"
        ));
        assert!(shown_call("not json").is_none());

        let Some(Shown::Sources(sources)) = shown_result(
            r#"{"query":"q","results":[
                {"title":"Tomatoes","url":"https://garden.example.com/"},
                {"title":"  ","url":"https://notes.example.com/"},
                {"title":"No address"}
            ]}"#,
        ) else {
            panic!("a search shows its sources");
        };
        assert_eq!(
            sources,
            [
                (
                    "Tomatoes".to_owned(),
                    "https://garden.example.com/".to_owned()
                ),
                (
                    "https://notes.example.com/".to_owned(),
                    "https://notes.example.com/".to_owned()
                ),
            ]
        );
        assert!(matches!(
            shown_result(r#"{"error":{"code":"web_consent_required","message":"Allow it."}}"#),
            Some(Shown::Failed(message)) if message == "Allow it."
        ));
        assert!(matches!(
            shown_result(r#"{"url":"https://garden.example.com/","text":"Grow."}"#),
            Some(Shown::Page)
        ));
        assert!(shown_result(r#"{"echoed":"hi"}"#).is_none());
    }
}
