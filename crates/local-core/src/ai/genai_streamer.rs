//! The `genai` adapter: provider clients, and conversion between our
//! transport messages and the library's chat types.

use super::*;

pub struct GenAiStreamer;

impl AiStreamer for GenAiStreamer {
    fn stream(
        &self,
        request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = Result<AiProviderEventStream>> + Send + '_>> {
        Box::pin(async move {
            if request.provider.kind == chatgpt::CHATGPT_KIND {
                return chatgpt_stream::stream(request).await;
            }
            let kind = request.provider.kind.clone();
            if request.credential.is_none() && needs_key(&request.provider) {
                return Err(failures::ProviderFailure {
                    error_type: Some("missing_key".into()),
                    ..failures::ProviderFailure::default()
                }
                .into_error(&kind, "ai_stream"));
            }
            let client = provider_client(
                &request.provider,
                request.credential.as_deref(),
                "ai_stream",
            )?;
            let messages = request
                .messages
                .into_iter()
                .map(|message| message_from_parts(message, &kind, "ai_stream"))
                .collect::<Result<Vec<_>>>()?;
            let tools: Vec<Tool> = request
                .tools
                .into_iter()
                .map(tool_from_definition)
                .collect();
            let mut options = ChatOptions::default()
                .with_capture_usage(true)
                .with_capture_tool_calls(true)
                // The signed reasoning an answer's tool calls need sent back.
                .with_capture_reasoning_content(true);
            if let Some(reasoning) = request.reasoning {
                options = options.with_reasoning_effort(reasoning_effort(reasoning));
            }
            let response = client
                .exec_chat_stream(
                    request.provider.model,
                    ChatRequest::new(messages).with_tools(tools),
                    Some(&options),
                )
                .await
                .map_err(|error| failures::of_genai(&error).into_error(&kind, "ai_stream"))?;
            let stream = response.stream.flat_map(move |event| {
                let events = match event {
                    Ok(ChatStreamEvent::Chunk(chunk)) => {
                        vec![Ok(AiProviderStreamEvent::TextDelta(chunk.content))]
                    }
                    Ok(ChatStreamEvent::End(end)) => {
                        let tool_calls: Vec<AiToolCall> = end
                            .captured_tool_calls()
                            .unwrap_or_default()
                            .into_iter()
                            .map(tool_call_from_genai)
                            .collect();
                        // Reasoning matters to the provider only when it
                        // comes back with the tool results.
                        let thoughts = if tool_calls.is_empty() {
                            Vec::new()
                        } else {
                            thoughts_of(end.captured_content.as_ref(), &kind)
                        };
                        vec![
                            Ok(AiProviderStreamEvent::Thoughts(thoughts)),
                            Ok(AiProviderStreamEvent::Completed {
                                usage: end.captured_usage.map(|usage| AiUsage {
                                    prompt_tokens: usage.prompt_tokens,
                                    completion_tokens: usage.completion_tokens,
                                    total_tokens: usage.total_tokens,
                                }),
                                stop_reason: end.captured_stop_reason.map(stop_reason),
                                tool_calls,
                            }),
                        ]
                    }
                    Ok(_) => Vec::new(),
                    Err(error) => vec![Err(
                        failures::of_genai(&error).into_error(&kind, "ai_stream")
                    )],
                };
                futures::stream::iter(events)
            });
            let stream: AiProviderEventStream = Box::pin(stream);
            Ok(stream)
        })
    }
}

/// Whether a request to `provider` goes nowhere without a key: a hosted
/// provider at its own address. Ollama and a server you named may take
/// none.
fn needs_key(provider: &AiProviderConfig) -> bool {
    provider.endpoint.is_none() && provider.kind != "ollama"
}

fn provider_client(
    provider: &AiProviderConfig,
    credential: Option<&str>,
    operation: &str,
) -> Result<Client> {
    let adapter = AdapterKind::from_lower_str(&provider.kind).ok_or_else(|| {
        CoreError::validation(
            "provider_kind_invalid",
            "The AI provider kind is not supported",
            operation,
        )
    })?;
    let endpoint = provider.endpoint.clone();
    let credential = credential.map(str::to_owned);
    let resolver = ServiceTargetResolver::from_resolver_fn(move |target: ServiceTarget| {
        let model = ModelIden::new(adapter, target.model.model_name);
        let endpoint = endpoint
            .as_ref()
            .map_or(target.endpoint, |value| Endpoint::from_owned(value.clone()));
        let auth = credential
            .as_ref()
            .map_or(AuthData::None, |value| AuthData::from_single(value.clone()));
        Ok(ServiceTarget {
            endpoint,
            auth,
            model,
        })
    });
    Client::builder()
        .with_service_target_resolver(resolver)
        .build()
        .map_err(|_| provider_error(operation))
}

/// The thought parts in an answer's captured content, in the order they
/// came: each signed block with its text and each block kept whole. A
/// signature that belongs to a tool call, as Gemini's do, stays bound to
/// that call. `genai` also copies Anthropic's signatures onto the first
/// call, but Anthropic's blocks belong to the whole answer, in order.
pub(super) fn thoughts_of(content: Option<&MessageContent>, kind: &str) -> Vec<AiContentPart> {
    let Some(content) = content else {
        return Vec::new();
    };
    let thought = |signature, text, block, call_id| AiContentPart::Thought {
        kind: kind.to_owned(),
        signature,
        text,
        block,
        call_id,
    };
    let mut thoughts: Vec<AiContentPart> = Vec::new();
    for part in content.parts() {
        match part {
            ContentPart::ThoughtSignature(signature) => {
                thoughts.push(thought(Some(signature.clone()), None, None, None));
            }
            ContentPart::ReasoningContent(reasoning) => match thoughts.last_mut() {
                Some(AiContentPart::Thought {
                    signature: Some(_),
                    text: text @ None,
                    block: None,
                    ..
                }) => *text = Some(reasoning.clone()),
                _ => thoughts.push(thought(None, Some(reasoning.clone()), None, None)),
            },
            ContentPart::Custom(custom) => {
                thoughts.push(thought(None, None, Some(custom.data.clone()), None));
            }
            _ => {}
        }
    }
    if kind == "anthropic" {
        return thoughts;
    }
    for call in content.tool_calls() {
        for signature in call.thought_signatures.iter().flatten() {
            let loose = thoughts.iter_mut().find_map(|part| match part {
                AiContentPart::Thought {
                    signature: Some(found),
                    call_id: call_id @ None,
                    ..
                } if found == signature => Some(call_id),
                _ => None,
            });
            match loose {
                Some(call_id) => *call_id = Some(call.call_id.clone()),
                None => thoughts.push(thought(
                    Some(signature.clone()),
                    None,
                    None,
                    Some(call.call_id.clone()),
                )),
            }
        }
    }
    thoughts
}

/// The `genai` parts for a thought sent back to a provider of `kind`.
/// Anthropic takes a signed block only with its text beside it, even when
/// the text is empty.
fn thought_parts(
    kind: &str,
    signature: Option<String>,
    text: Option<String>,
    block: Option<serde_json::Value>,
) -> Vec<ContentPart> {
    let mut parts = Vec::new();
    if let Some(block) = block {
        parts.push(ContentPart::Custom(genai::chat::CustomPart {
            model_iden: None,
            data: block,
        }));
    }
    let signed = signature.is_some();
    if let Some(signature) = signature {
        parts.push(ContentPart::ThoughtSignature(signature));
    }
    match text {
        Some(text) => parts.push(ContentPart::ReasoningContent(text)),
        None if signed && kind == "anthropic" => {
            parts.push(ContentPart::ReasoningContent(String::new()))
        }
        None => {}
    }
    parts
}

/// The `genai` message for `message`, to a provider of `kind`. Thoughts
/// another kind of provider wrote are dropped. A thought bound to a tool
/// call goes just before that call, where Gemini looks for it.
fn message_from_parts(
    message: AiTransportMessage,
    kind: &str,
    operation: &str,
) -> Result<ChatMessage> {
    let mut bound: Vec<(String, Vec<ContentPart>)> = Vec::new();
    let mut loose: Vec<AiContentPart> = Vec::new();
    for part in message.content {
        match part {
            AiContentPart::Thought { kind: wrote, .. } if wrote != kind => {}
            AiContentPart::Thought {
                signature,
                text,
                block,
                call_id: Some(call_id),
                ..
            } => bound.push((call_id, thought_parts(kind, signature, text, block))),
            other => loose.push(other),
        }
    }
    let parts = loose
        .into_iter()
        .flat_map(|part| match part {
            AiContentPart::Thought {
                signature,
                text,
                block,
                ..
            } => thought_parts(kind, signature, text, block),
            AiContentPart::ToolCall {
                call_id,
                name,
                arguments,
            } => {
                let mut parts: Vec<ContentPart> = bound
                    .iter()
                    .filter(|(bound_to, _)| *bound_to == call_id)
                    .flat_map(|(_, parts)| parts.clone())
                    .collect();
                parts.push(ContentPart::ToolCall(ToolCall {
                    call_id,
                    fn_name: name,
                    fn_arguments: arguments,
                    thought_signatures: None,
                }));
                parts
            }
            AiContentPart::Text { text } => vec![ContentPart::Text(text)],
            AiContentPart::ToolResult {
                call_id,
                name,
                content,
            } => vec![ContentPart::ToolResponse(ToolResponse {
                call_id,
                fn_name: name,
                content,
            })],
        })
        .collect::<Vec<_>>();
    let content = MessageContent::from_parts(parts);
    match message.role.as_str() {
        "system" => Ok(ChatMessage::system(content)),
        "assistant" => Ok(ChatMessage::assistant(content)),
        "user" => Ok(ChatMessage::user(content)),
        "tool" => Ok(ChatMessage::tool(content)),
        _ => Err(CoreError::validation(
            "message_role_invalid",
            "The AI message role is not supported",
            operation,
        )),
    }
}

fn tool_from_definition(tool: AiToolDefinition) -> Tool {
    let mut defined = Tool::new(tool.name).with_schema(tool.input_schema);
    defined.description = tool.description;
    defined
}

/// The `genai` effort for `reasoning`, which each adapter writes as its
/// provider's parameter.
pub(super) fn reasoning_effort(reasoning: AiReasoning) -> ReasoningEffort {
    match reasoning {
        AiReasoning::Zero => ReasoningEffort::Zero,
        AiReasoning::Minimal => ReasoningEffort::Minimal,
        AiReasoning::Low => ReasoningEffort::Low,
        AiReasoning::Medium => ReasoningEffort::Medium,
        AiReasoning::High => ReasoningEffort::High,
    }
}

fn tool_call_from_genai(call: &ToolCall) -> AiToolCall {
    AiToolCall {
        call_id: call.call_id.clone(),
        name: call.fn_name.clone(),
        arguments: call.fn_arguments.clone(),
    }
}

fn stop_reason(reason: StopReason) -> AiStopReason {
    match reason {
        StopReason::Completed(raw) => AiStopReason::Completed { raw },
        StopReason::MaxTokens(raw) => AiStopReason::MaxTokens { raw },
        StopReason::ToolCall(raw) => AiStopReason::ToolCall { raw },
        StopReason::ContentFilter(raw) => AiStopReason::ContentFilter { raw },
        StopReason::StopSequence(raw) => AiStopReason::StopSequence { raw },
        StopReason::Other(raw) => AiStopReason::Other { raw },
    }
}
