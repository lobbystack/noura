//! Workspace tools: the tools that plugins on in the workspace register,
//! as the model sees them, the question Noura asks before each call runs,
//! and what a call's result says.

use plugin_host::AiToolDefinition;

use crate::{AppError, PluginTool};

/// Between a plugin's ID and its tool's name in the name the model sees.
/// Plugin IDs hold no underscore, so the first one ends the plugin's ID.
const SEPARATOR: &str = "__";

/// The longest tool name providers take.
const MAX_NAME: usize = 64;

/// The plugins running in the open workspace and the tools they register.
/// The session gives the AI service one, so a run reaches tools without
/// holding the plugins service.
pub(crate) trait ToolBox: Send + Sync {
    /// The tools of the plugins running in the open workspace.
    fn tools(&self) -> Vec<PluginTool>;

    /// The name of the plugin `id`, as its manifest gives it.
    fn plugin_name(&self, id: &str) -> Option<String>;

    /// Run the tool `name` of the plugin `plugin` with `arguments`, which
    /// must match its schema, and wait for its answer.
    fn call(&self, plugin: &str, name: &str, arguments: &str) -> Result<String, AppError>;
}

/// The question Noura asks before a tool call that the model asked for
/// runs: may the tool of this plugin run with this input?
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiToolPrompt {
    /// The run that asked, and the model's ID for the call.
    pub run: String,
    pub call: String,
    /// The tool, as its plugin names it.
    pub tool: String,
    /// What the tool does, as its plugin says.
    pub description: String,
    pub plugin_id: String,
    pub plugin_name: String,
    /// The call's input, as indented JSON.
    pub input: String,
}

/// The name the model sees for the tool `tool` of the plugin `plugin`,
/// or none when it would be longer than providers take.
fn qualified(plugin: &str, tool: &str) -> Option<String> {
    let name = format!("{plugin}{SEPARATOR}{tool}");
    (name.len() <= MAX_NAME).then_some(name)
}

/// The plugin and the tool a name the model sees stands for.
pub(super) fn split(name: &str) -> Option<(&str, &str)> {
    let (plugin, tool) = name.split_once(SEPARATOR)?;
    (plugin_host::valid_plugin_id(plugin) && !tool.is_empty()).then_some((plugin, tool))
}

/// The tools of `tools`, as the model sees them.
pub(super) fn definitions(tools: &[PluginTool]) -> Vec<AiToolDefinition> {
    tools
        .iter()
        .filter_map(|tool| {
            let name = qualified(&tool.plugin_id, &tool.name);
            if name.is_none() {
                tracing::warn!("a plugin tool whose name is too long for providers was left out");
            }
            Some(AiToolDefinition {
                name: name?,
                description: tool.description.clone(),
                input_schema: tool.input_schema.clone(),
            })
        })
        .collect()
}

/// The registered tool that the name `name` the model sees stands for.
pub(super) fn find<'a>(name: &str, tools: &'a [PluginTool]) -> Option<&'a PluginTool> {
    let (plugin, tool) = split(name)?;
    tools
        .iter()
        .find(|known| known.plugin_id == plugin && known.name == tool)
}

/// The plugins whose tools `tools` names.
pub(super) fn plugins_of(tools: &[AiToolDefinition]) -> Vec<String> {
    let mut plugins: Vec<String> = tools
        .iter()
        .filter_map(|tool| split(&tool.name))
        .map(|(plugin, _)| plugin.to_owned())
        .collect();
    plugins.sort();
    plugins.dedup();
    plugins
}

/// `arguments`, a JSON object as the model sent it, indented for the
/// prompt.
pub(super) fn indented(arguments: &str) -> String {
    serde_json::from_str::<serde_json::Value>(arguments)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| arguments.to_owned())
}

/// How a tool call ended, as its result and its tool-call message say.
pub(super) enum Settled {
    /// The tool ran and answered with this.
    Answered(String),
    /// You denied the call, closed the prompt, stopped the chat or opened
    /// another workspace.
    Denied,
    /// The tool, or Noura, failed before or while it ran.
    Failed { code: &'static str, message: String },
    /// The plugin the run relied on was turned off.
    Disabled,
}

/// The error code a tool call records when it couldn't run.
pub(super) const PLUGIN_DISABLED: &str = "plugin-disabled";
pub(super) const TOOL_FAILED: &str = "tool-failed";
pub(super) const TOOL_NOT_FOUND: &str = "tool-not-found";

impl Settled {
    /// The result's content, as JSON text for the model.
    pub(super) fn content(&self) -> String {
        let value = match self {
            Self::Answered(answer) => serde_json::from_str::<serde_json::Value>(answer)
                .unwrap_or_else(|_| serde_json::Value::String(answer.clone())),
            Self::Denied => serde_json::json!({ "denied": true }),
            Self::Failed { code, message } => {
                serde_json::json!({ "error": { "code": code, "message": message } })
            }
            Self::Disabled => serde_json::json!({
                "error": {
                    "code": PLUGIN_DISABLED,
                    "message": "The plugin that offers this tool was turned off.",
                }
            }),
        };
        value.to_string()
    }

    /// The status and error code of the call's tool-call message.
    pub(super) fn status(&self) -> (local_core::ChatMessageStatus, Option<String>) {
        use local_core::ChatMessageStatus;
        match self {
            Self::Answered(_) | Self::Denied => (ChatMessageStatus::Completed, None),
            Self::Failed { code, .. } => (ChatMessageStatus::Failed, Some((*code).to_owned())),
            Self::Disabled => (
                ChatMessageStatus::Interrupted,
                Some(PLUGIN_DISABLED.to_owned()),
            ),
        }
    }

    /// How a call whose tool ran, or failed to, ended.
    pub(super) fn of(outcome: Result<String, AppError>) -> Self {
        match outcome {
            Ok(answer) => Self::Answered(answer),
            Err(error) => Self::Failed {
                code: if error.code == "plugin_tool_unknown" {
                    TOOL_NOT_FOUND
                } else {
                    TOOL_FAILED
                },
                message: error.message,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(plugin: &str, name: &str) -> PluginTool {
        PluginTool {
            plugin_id: plugin.to_owned(),
            name: name.to_owned(),
            description: "Does it.".to_owned(),
            input_schema: "{}".to_owned(),
        }
    }

    #[test]
    fn the_model_sees_each_tool_under_its_plugin() {
        let tools = [tool("tasks", "create"), tool("tasks", "do_it")];
        let names: Vec<String> = definitions(&tools)
            .into_iter()
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["tasks__create", "tasks__do_it"]);
        assert_eq!(find("tasks__do_it", &tools), Some(&tools[1]));
        assert_eq!(find("tasks__gone", &tools), None);
        assert_eq!(find("echo", &tools), None);
        assert_eq!(split("Tasks__create"), None);
    }

    #[test]
    fn a_name_too_long_for_providers_is_left_out() {
        let tools = [tool("tasks", &"a".repeat(60))];
        assert!(definitions(&tools).is_empty());
    }

    #[test]
    fn each_ending_says_what_happened_to_the_model() {
        assert_eq!(
            Settled::Answered("{\"id\":\"task_1\"}".into()).content(),
            "{\"id\":\"task_1\"}"
        );
        assert_eq!(Settled::Answered("done".into()).content(), "\"done\"");
        assert_eq!(Settled::Denied.content(), "{\"denied\":true}");
        assert_eq!(
            Settled::Failed {
                code: TOOL_FAILED,
                message: "No.".into()
            }
            .content(),
            "{\"error\":{\"code\":\"tool-failed\",\"message\":\"No.\"}}"
        );
    }
}
