//! The part of Jinja that anarlog's prompt templates and your own prompt
//! use, rendered the way minijinja renders it with strict undefined
//! values: text, `{{ expressions }}` with filters, `{% if %}`, `{% elif %}`,
//! `{% else %}`, `{% for %}`, comments and `-` whitespace control.
//!
//! minijinja itself builds for wasm32-wasip2, but every `Environment`
//! holds a `HashMap` whose `RandomState` asks WASI for a random seed,
//! and Noura gives plugins none, so it traps at its first template. The
//! tests render each template with minijinja as well and compare.

use std::collections::BTreeMap;

/// A value a template reads.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Val {
    Str(String),
    Bool(bool),
    List(Vec<Val>),
    Map(BTreeMap<String, Val>),
}

impl From<&str> for Val {
    fn from(text: &str) -> Self {
        Self::Str(text.to_owned())
    }
}

impl From<bool> for Val {
    fn from(value: bool) -> Self {
        Self::Bool(value)
    }
}

impl Val {
    fn truthy(&self) -> bool {
        match self {
            Self::Str(text) => !text.is_empty(),
            Self::Bool(value) => *value,
            Self::List(items) => !items.is_empty(),
            Self::Map(map) => !map.is_empty(),
        }
    }

    fn write(&self, out: &mut String) {
        match self {
            Self::Str(text) => out.push_str(text),
            Self::Bool(value) => out.push_str(if *value { "True" } else { "False" }),
            Self::List(items) => {
                out.push('[');
                for (index, item) in items.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    item.write_quoted(out);
                }
                out.push(']');
            }
            Self::Map(map) => {
                out.push('{');
                for (index, (key, value)) in map.iter().enumerate() {
                    if index > 0 {
                        out.push_str(", ");
                    }
                    out.push_str(&format!("{key:?}: "));
                    value.write_quoted(out);
                }
                out.push('}');
            }
        }
    }

    fn write_quoted(&self, out: &mut String) {
        match self {
            Self::Str(text) => out.push_str(&format!("{text:?}")),
            other => other.write(out),
        }
    }
}

/// Why a template didn't render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Error {
    /// It reads a variable or an attribute that isn't there.
    Undefined(String),
    /// It isn't a template this renderer reads.
    Syntax(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undefined(name) => write!(f, "{name} is undefined"),
            Self::Syntax(reason) => f.write_str(reason),
        }
    }
}

#[derive(Debug)]
enum Expr {
    Var(String),
    Attr(Box<Expr>, String),
    Lit(Val),
    Not(Box<Expr>),
    And(Box<Expr>, Box<Expr>),
    Or(Box<Expr>, Box<Expr>),
    Eq(Box<Expr>, Box<Expr>, bool),
    Filter(Box<Expr>, String),
}

#[derive(Debug)]
enum Node {
    Text(String),
    Out(Expr),
    If(Vec<(Expr, Vec<Node>)>, Vec<Node>),
    For(String, Expr, Vec<Node>),
}

/// A piece of the source: text, or a tag with its content and whether
/// it trims the whitespace before and after it.
#[derive(Debug)]
enum Piece {
    Text(String),
    Expr(String),
    Stmt(String),
}

fn lex(source: &str) -> Result<Vec<Piece>, Error> {
    // As minijinja does by default, drop one line break at the end.
    let source = source
        .strip_suffix("\r\n")
        .or_else(|| source.strip_suffix('\n'))
        .unwrap_or(source);
    let mut pieces: Vec<Piece> = Vec::new();
    let mut rest = source;
    let mut trim_next = false;
    loop {
        let next = ["{{", "{%", "{#"]
            .iter()
            .filter_map(|open| rest.find(open).map(|at| (at, *open)))
            .min_by_key(|(at, _)| *at);
        let Some((at, open)) = next else {
            push_text(&mut pieces, rest, trim_next);
            return Ok(pieces);
        };
        let mut text = &rest[..at];
        let after = &rest[at + 2..];
        let trim_before = after.starts_with('-');
        if trim_before {
            text = text.trim_end();
        }
        push_text(&mut pieces, text, trim_next);
        let close = match open {
            "{{" => "}}",
            "{%" => "%}",
            _ => "#}",
        };
        let inner_start = usize::from(trim_before);
        let end = after
            .find(close)
            .ok_or_else(|| Error::Syntax(format!("{open} isn't closed")))?;
        let mut inner = &after[inner_start..end];
        trim_next = inner.ends_with('-');
        if trim_next {
            inner = &inner[..inner.len() - 1];
        }
        match open {
            "{{" => pieces.push(Piece::Expr(inner.trim().to_owned())),
            "{%" => pieces.push(Piece::Stmt(inner.trim().to_owned())),
            _ => {}
        }
        rest = &after[end + 2..];
    }
}

fn push_text(pieces: &mut Vec<Piece>, text: &str, trim_start: bool) {
    let text = if trim_start { text.trim_start() } else { text };
    if !text.is_empty() {
        pieces.push(Piece::Text(text.to_owned()));
    }
}

/// The words and symbols of an expression.
fn tokens(source: &str) -> Result<Vec<String>, Error> {
    let mut out = Vec::new();
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '"' || c == '\'' {
            let mut text = String::from('"');
            i += 1;
            while i < chars.len() && chars[i] != c {
                if chars[i] == '\\' && i + 1 < chars.len() {
                    i += 1;
                    text.push(match chars[i] {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                } else {
                    text.push(chars[i]);
                }
                i += 1;
            }
            if i == chars.len() {
                return Err(Error::Syntax("a string isn't closed".into()));
            }
            i += 1;
            out.push(text);
        } else if c.is_alphanumeric() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            out.push(chars[start..i].iter().collect());
        } else if (c == '=' || c == '!') && chars.get(i + 1) == Some(&'=') {
            out.push(format!("{c}="));
            i += 2;
        } else if matches!(c, '|' | '.' | '(' | ')') {
            out.push(c.to_string());
            i += 1;
        } else {
            return Err(Error::Syntax(format!("{c} isn't supported here")));
        }
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<String>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&str> {
        self.tokens.get(self.at).map(String::as_str)
    }

    fn take(&mut self) -> Option<String> {
        let token = self.tokens.get(self.at).cloned();
        self.at += 1;
        token
    }

    fn or(&mut self) -> Result<Expr, Error> {
        let mut left = self.and()?;
        while self.peek() == Some("or") {
            self.at += 1;
            left = Expr::Or(Box::new(left), Box::new(self.and()?));
        }
        Ok(left)
    }

    fn and(&mut self) -> Result<Expr, Error> {
        let mut left = self.not()?;
        while self.peek() == Some("and") {
            self.at += 1;
            left = Expr::And(Box::new(left), Box::new(self.not()?));
        }
        Ok(left)
    }

    fn not(&mut self) -> Result<Expr, Error> {
        if self.peek() == Some("not") {
            self.at += 1;
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.compare()
    }

    fn compare(&mut self) -> Result<Expr, Error> {
        let left = self.filtered()?;
        match self.peek() {
            Some(op @ ("==" | "!=")) => {
                let equal = op == "==";
                self.at += 1;
                Ok(Expr::Eq(Box::new(left), Box::new(self.filtered()?), equal))
            }
            _ => Ok(left),
        }
    }

    fn filtered(&mut self) -> Result<Expr, Error> {
        let mut value = self.primary()?;
        while self.peek() == Some("|") {
            self.at += 1;
            let name = self
                .take()
                .filter(|name| name.chars().all(|c| c.is_alphanumeric() || c == '_'))
                .ok_or_else(|| Error::Syntax("a filter needs a name".into()))?;
            value = Expr::Filter(Box::new(value), name);
        }
        Ok(value)
    }

    fn primary(&mut self) -> Result<Expr, Error> {
        let token = self
            .take()
            .ok_or_else(|| Error::Syntax("an expression is missing".into()))?;
        let mut value = if let Some(text) = token.strip_prefix('"') {
            Expr::Lit(Val::Str(text.to_owned()))
        } else if token == "(" {
            let inner = self.or()?;
            if self.take().as_deref() != Some(")") {
                return Err(Error::Syntax("a ( isn't closed".into()));
            }
            inner
        } else if token == "true" || token == "True" {
            Expr::Lit(Val::Bool(true))
        } else if token == "false" || token == "False" {
            Expr::Lit(Val::Bool(false))
        } else if token
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            Expr::Var(token)
        } else {
            return Err(Error::Syntax(format!("{token} isn't supported here")));
        };
        while self.peek() == Some(".") {
            self.at += 1;
            let name = self
                .take()
                .ok_or_else(|| Error::Syntax("an attribute needs a name".into()))?;
            value = Expr::Attr(Box::new(value), name);
        }
        Ok(value)
    }
}

fn expression(source: &str) -> Result<Expr, Error> {
    let mut parser = Parser {
        tokens: tokens(source)?,
        at: 0,
    };
    let expr = parser.or()?;
    if parser.at < parser.tokens.len() {
        return Err(Error::Syntax(format!("{source} isn't supported here")));
    }
    Ok(expr)
}

/// Parse `pieces` up to one of the statements `ends`, returning the nodes
/// and the statement that ended them.
fn block(
    pieces: &mut std::iter::Peekable<std::vec::IntoIter<Piece>>,
    ends: &[&str],
) -> Result<(Vec<Node>, Option<String>), Error> {
    let mut nodes = Vec::new();
    while let Some(piece) = pieces.next() {
        match piece {
            Piece::Text(text) => nodes.push(Node::Text(text)),
            Piece::Expr(source) => nodes.push(Node::Out(expression(&source)?)),
            Piece::Stmt(source) => {
                let keyword = source.split_whitespace().next().unwrap_or_default();
                if ends.contains(&keyword) {
                    return Ok((nodes, Some(source)));
                }
                match keyword {
                    "if" => nodes.push(if_block(pieces, &source[2..])?),
                    "for" => {
                        let rest = source[3..].trim();
                        let (name, list) = rest
                            .split_once(" in ")
                            .ok_or_else(|| Error::Syntax("for needs in".into()))?;
                        let (body, end) = block(pieces, &["endfor"])?;
                        if end.is_none() {
                            return Err(Error::Syntax("for isn't closed".into()));
                        }
                        nodes.push(Node::For(name.trim().to_owned(), expression(list)?, body));
                    }
                    other => {
                        return Err(Error::Syntax(format!("{other} isn't supported here")));
                    }
                }
            }
        }
    }
    Ok((nodes, None))
}

fn if_block(
    pieces: &mut std::iter::Peekable<std::vec::IntoIter<Piece>>,
    condition: &str,
) -> Result<Node, Error> {
    let mut branches = Vec::new();
    let mut condition = expression(condition)?;
    loop {
        let (body, end) = block(pieces, &["elif", "else", "endif"])?;
        let end = end.ok_or_else(|| Error::Syntax("if isn't closed".into()))?;
        branches.push((condition, body));
        if let Some(next) = end.strip_prefix("elif") {
            condition = expression(next)?;
        } else if end == "else" {
            let (otherwise, end) = block(pieces, &["endif"])?;
            if end.is_none() {
                return Err(Error::Syntax("if isn't closed".into()));
            }
            return Ok(Node::If(branches, otherwise));
        } else {
            return Ok(Node::If(branches, Vec::new()));
        }
    }
}

/// A parsed template.
pub(crate) struct Template {
    nodes: Vec<Node>,
}

impl Template {
    pub(crate) fn parse(source: &str) -> Result<Self, Error> {
        let mut pieces = lex(source)?.into_iter().peekable();
        let (nodes, end) = block(&mut pieces, &[])?;
        if let Some(end) = end {
            return Err(Error::Syntax(format!("{end} has no start")));
        }
        Ok(Self { nodes })
    }

    /// Render with the variables in `context`. A variable that isn't
    /// there fails, as minijinja's strict mode does.
    pub(crate) fn render(&self, context: &BTreeMap<String, Val>) -> Result<String, Error> {
        let mut out = String::new();
        let mut scope = context.clone();
        render(&self.nodes, &mut scope, &mut out)?;
        Ok(out)
    }
}

fn render(
    nodes: &[Node],
    scope: &mut BTreeMap<String, Val>,
    out: &mut String,
) -> Result<(), Error> {
    for node in nodes {
        match node {
            Node::Text(text) => out.push_str(text),
            Node::Out(expr) => eval(expr, scope)?.write(out),
            Node::If(branches, otherwise) => {
                let mut chosen = None;
                for (condition, body) in branches {
                    if eval(condition, scope)?.truthy() {
                        chosen = Some(body);
                        break;
                    }
                }
                render(chosen.unwrap_or(otherwise), scope, out)?;
            }
            Node::For(name, list, body) => {
                let items = match eval(list, scope)? {
                    Val::List(items) => items,
                    Val::Str(text) => text.chars().map(|c| Val::Str(c.to_string())).collect(),
                    _ => return Err(Error::Syntax(format!("{name} can't loop over that"))),
                };
                let saved = scope.get(name).cloned();
                for item in items {
                    scope.insert(name.clone(), item);
                    render(body, scope, out)?;
                }
                match saved {
                    Some(saved) => scope.insert(name.clone(), saved),
                    None => scope.remove(name),
                };
            }
        }
    }
    Ok(())
}

fn eval(expr: &Expr, scope: &BTreeMap<String, Val>) -> Result<Val, Error> {
    Ok(match expr {
        Expr::Var(name) => scope
            .get(name)
            .cloned()
            .ok_or_else(|| Error::Undefined(name.clone()))?,
        Expr::Attr(value, name) => match eval(value, scope)? {
            Val::Map(map) => map
                .get(name)
                .cloned()
                .ok_or_else(|| Error::Undefined(name.clone()))?,
            _ => return Err(Error::Undefined(name.clone())),
        },
        Expr::Lit(value) => value.clone(),
        Expr::Not(value) => Val::Bool(!eval(value, scope)?.truthy()),
        Expr::And(left, right) => {
            let left = eval(left, scope)?;
            if left.truthy() {
                eval(right, scope)?
            } else {
                left
            }
        }
        Expr::Or(left, right) => {
            let left = eval(left, scope)?;
            if left.truthy() {
                left
            } else {
                eval(right, scope)?
            }
        }
        Expr::Eq(left, right, equal) => {
            Val::Bool((eval(left, scope)? == eval(right, scope)?) == *equal)
        }
        Expr::Filter(value, name) => {
            let value = eval(value, scope)?;
            let Val::Str(text) = value else {
                return Err(Error::Syntax(format!("{name} needs text")));
            };
            Val::Str(match name.as_str() {
                "upper" => text.to_uppercase(),
                "lower" => text.to_lowercase(),
                "trim" => text.trim().to_owned(),
                "capitalize" => {
                    let mut chars = text.chars();
                    chars.next().map_or_else(String::new, |first| {
                        first
                            .to_uppercase()
                            .chain(chars.flat_map(char::to_lowercase))
                            .collect()
                    })
                }
                other => return Err(Error::Syntax(format!("the filter {other} isn't supported"))),
            })
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn context() -> BTreeMap<String, Val> {
        let segment = |speaker: &str, text: &str| {
            Val::Map(BTreeMap::from([
                ("speaker".to_owned(), Val::from(speaker)),
                ("text".to_owned(), Val::from(text)),
            ]))
        };
        BTreeMap::from([
            ("language".to_owned(), Val::from("Korean")),
            ("current_date".to_owned(), Val::from("2026-11-16")),
            ("format_requirements".to_owned(), Val::from("- Bullets.")),
            ("default_format".to_owned(), Val::from(true)),
            ("title".to_owned(), Val::from("Standup")),
            (
                "participants".to_owned(),
                Val::List(vec![Val::from("Ada"), Val::from("Grace")]),
            ),
            ("pre_meeting_memo".to_owned(), Val::from("")),
            ("post_meeting_memo".to_owned(), Val::from("- check CI")),
            (
                "segments".to_owned(),
                Val::List(vec![segment("Me", "Ship it."), segment("Others", "Sure.")]),
            ),
        ])
    }

    fn to_mini(value: &Val) -> minijinja::Value {
        match value {
            Val::Str(text) => minijinja::Value::from(text.as_str()),
            Val::Bool(value) => minijinja::Value::from(*value),
            Val::List(items) => items.iter().map(to_mini).collect(),
            Val::Map(map) => minijinja::Value::from_iter(
                map.iter().map(|(key, value)| (key.clone(), to_mini(value))),
            ),
        }
    }

    /// What minijinja renders, in strict mode.
    fn minijinja(source: &str, context: &BTreeMap<String, Val>) -> Result<String, ()> {
        let mut env = minijinja::Environment::new();
        env.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
        let mini: BTreeMap<String, minijinja::Value> = context
            .iter()
            .map(|(key, value)| (key.clone(), to_mini(value)))
            .collect();
        env.template_from_str(source)
            .and_then(|template| template.render(&mini))
            .map_err(drop)
    }

    fn ours(source: &str, context: &BTreeMap<String, Val>) -> Result<String, ()> {
        Template::parse(source)
            .and_then(|template| template.render(context))
            .map_err(drop)
    }

    #[test]
    fn anarlogs_templates_render_as_minijinja_renders_them() {
        let templates = [
            include_str!("../templates/enhance.system.md.jinja"),
            include_str!("../templates/enhance.format.md.jinja"),
            include_str!("../templates/enhance.user.md.jinja"),
        ];
        let mut variations = vec![context()];
        for (key, value) in [
            ("language", Val::from("English")),
            ("default_format", Val::from(false)),
            ("title", Val::from("")),
            ("participants", Val::List(Vec::new())),
            ("post_meeting_memo", Val::from("")),
            ("pre_meeting_memo", Val::from("Agenda")),
            ("segments", Val::List(Vec::new())),
        ] {
            let mut changed = context();
            changed.insert(key.to_owned(), value);
            variations.push(changed);
        }
        for template in templates {
            for context in &variations {
                let expected = minijinja(template, context).unwrap();
                assert_eq!(ours(template, context).unwrap(), expected);
            }
        }
    }

    #[test]
    fn prompts_render_as_minijinja_renders_them() {
        let context = context();
        for prompt in [
            "Write a concise narrative in {{ language }}.",
            "Summarize in {{ language|upper }} on {{current_date}}.",
            "Group by {{ \"{{\" }} customer }}.",
            "{# a comment #}Plain\n\n  {%- if language == 'Korean' -%}  ko {% elif language %}other{% else %}none{% endif %}\n",
            "{% if not default_format or language != \"English\" %}yes{% endif %}",
            "{% for name in participants %}{{ name|lower }}, {% endfor %}end",
            "{{ language | capitalize | trim }}",
            "Lines\r\nwith\r\n",
            "{{ participants }} {{ default_format }}",
        ] {
            assert_eq!(
                ours(prompt, &context),
                minijinja(prompt, &context),
                "{prompt}"
            );
        }
        for broken in [
            "Summarize {{ transcript }}.",
            "{% if %}",
            "{{ language|nope }}",
            "{% if language %}open",
            "{% endif %}",
            "{{ segments.first }}",
        ] {
            assert!(ours(broken, &context).is_err(), "{broken}");
            assert!(minijinja(broken, &context).is_err(), "{broken}");
        }
        assert_eq!(
            Template::parse("{{ transcript }}")
                .unwrap()
                .render(&context)
                .unwrap_err(),
            Error::Undefined("transcript".into())
        );
    }
}
