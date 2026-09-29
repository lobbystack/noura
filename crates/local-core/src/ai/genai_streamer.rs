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
            let client = provider_client(
                &request.provider,
                request.credential.as_deref(),
                "ai_stream",
            )?;
            let messages = request
                .messages
                .into_iter()
                .map(|message| message_from_parts(message, "ai_stream"))
                .collect::<Result<Vec<_>>>()?;
            let tools: Vec<Tool> = request
                .tools
                .into_iter()
                .map(tool_from_definition)
                .collect();
            let options = ChatOptions::default()
                .with_capture_usage(true)
                .with_capture_tool_calls(true);
            let response = client
                .exec_chat_stream(
                    request.provider.model,
                    ChatRequest::new(messages).with_tools(tools),
                    Some(&options),
                )
                .await
                .map_err(|_| provider_error("ai_stream"))?;
            let stream = response.stream.filter_map(|event| async move {
                match event {
                    Ok(ChatStreamEvent::Chunk(chunk)) => {
                        Some(Ok(AiProviderStreamEvent::TextDelta(chunk.content)))
                    }
                    Ok(ChatStreamEvent::End(end)) => {
                        let tool_calls = end
                            .captured_tool_calls()
                            .unwrap_or_default()
                            .into_iter()
                            .map(tool_call_from_genai)
                            .collect();
                        Some(Ok(AiProviderStreamEvent::Completed {
                            usage: end.captured_usage.map(|usage| AiUsage {
                                prompt_tokens: usage.prompt_tokens,
                                completion_tokens: usage.completion_tokens,
                                total_tokens: usage.total_tokens,
                            }),
                            stop_reason: end.captured_stop_reason.map(stop_reason),
                            tool_calls,
                        }))
                    }
                    Ok(_) => None,
                    Err(_) => Some(Err(provider_error("ai_stream"))),
                }
            });
            let stream: AiProviderEventStream = Box::pin(stream);
            Ok(stream)
        })
    }
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
    Ok(Client::builder()
        .with_service_target_resolver(resolver)
        .build())
}

fn message_from_parts(message: AiTransportMessage, operation: &str) -> Result<ChatMessage> {
    let parts = message
        .content
        .into_iter()
        .map(|part| match part {
            AiContentPart::Text { text } => ContentPart::Text(text),
            AiContentPart::ToolCall {
                call_id,
                name,
                arguments,
            } => ContentPart::ToolCall(ToolCall {
                call_id,
                fn_name: name,
                fn_arguments: arguments,
                thought_signatures: None,
            }),
            AiContentPart::ToolResult {
                call_id,
                name,
                content,
            } => ContentPart::ToolResponse(ToolResponse {
                call_id,
                fn_name: name,
                content,
            }),
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
    Tool {
        name: tool.name.into(),
        description: tool.description,
        schema: Some(tool.input_schema),
        strict: None,
        config: None,
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
