//! anarlog's prompt templates and your own prompt, rendered with
//! minijinja in strict mode, so a variable the context lacks fails as it
//! does in anarlog. Values reach the template as [`Val`], which keeps the
//! rest of the plugin free of minijinja's types.

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
    fn to_mini(&self) -> minijinja::Value {
        match self {
            Self::Str(text) => minijinja::Value::from(text.as_str()),
            Self::Bool(value) => minijinja::Value::from(*value),
            Self::List(items) => items.iter().map(Self::to_mini).collect(),
            Self::Map(map) => minijinja::Value::from_iter(
                map.iter()
                    .map(|(key, value)| (key.clone(), value.to_mini())),
            ),
        }
    }
}

/// Why a template didn't render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Error {
    /// It reads a variable the context doesn't hold, named here.
    Undefined(String),
    /// It isn't a template, or it fails as it renders.
    Syntax(String),
}

fn error(error: &minijinja::Error) -> Error {
    match error.kind() {
        minijinja::ErrorKind::UndefinedError => Error::Undefined(
            error
                .detail()
                .map_or_else(|| error.to_string(), str::to_owned),
        ),
        _ => Error::Syntax(
            error
                .detail()
                .map_or_else(|| error.to_string(), str::to_owned),
        ),
    }
}

/// A template that parsed.
pub(crate) struct Template {
    source: String,
}

impl Template {
    pub(crate) fn parse(source: &str) -> Result<Self, Error> {
        environment()
            .template_from_str(source)
            .map_err(|failure| error(&failure))?;
        Ok(Self {
            source: source.to_owned(),
        })
    }

    /// Render with the variables in `context`. A variable that isn't
    /// there fails.
    pub(crate) fn render(&self, context: &BTreeMap<String, Val>) -> Result<String, Error> {
        let values: BTreeMap<&str, minijinja::Value> = context
            .iter()
            .map(|(key, value)| (key.as_str(), value.to_mini()))
            .collect();
        let environment = environment();
        let template = environment
            .template_from_str(&self.source)
            .map_err(|failure| error(&failure))?;
        template.render(values).map_err(|failure| error(&failure))
    }
}

fn environment() -> minijinja::Environment<'static> {
    let mut environment = minijinja::Environment::new();
    environment.set_undefined_behavior(minijinja::UndefinedBehavior::Strict);
    environment
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

    fn render(source: &str, context: &BTreeMap<String, Val>) -> Result<String, Error> {
        Template::parse(source).and_then(|template| template.render(context))
    }

    #[test]
    fn anarlogs_templates_render() {
        let user = render(
            include_str!("../templates/enhance.user.md.jinja"),
            &context(),
        )
        .unwrap();
        assert!(user.contains("Ada"), "{user}");
        assert!(user.contains("Me: Ship it."), "{user}");
        let system = render(
            include_str!("../templates/enhance.system.md.jinja"),
            &context(),
        )
        .unwrap();
        assert!(system.contains("Korean"), "{system}");
        render(
            include_str!("../templates/enhance.format.md.jinja"),
            &context(),
        )
        .unwrap();
    }

    #[test]
    fn prompts_render_and_broken_ones_fail() {
        let context = context();
        assert_eq!(
            render(
                "Summarize in {{ language|upper }} on {{current_date}}.",
                &context
            )
            .unwrap(),
            "Summarize in KOREAN on 2026-11-16."
        );
        assert_eq!(
            render(
                "{% for name in participants %}{{ name|lower }}, {% endfor %}end",
                &context
            )
            .unwrap(),
            "ada, grace, end"
        );
        for broken in [
            "{% if %}",
            "{{ language|nope }}",
            "{% if language %}open",
            "{% endif %}",
        ] {
            assert!(
                matches!(render(broken, &context), Err(Error::Syntax(_))),
                "{broken}"
            );
        }
        assert!(matches!(
            render("Summarize {{ transcript }}.", &context),
            Err(Error::Undefined(_))
        ));
    }
}
