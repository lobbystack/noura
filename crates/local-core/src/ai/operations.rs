//! Active streams: the operation registry, cancellation, and the frames
//! each stream sends to its subscriber.

use super::*;

pub(super) const STREAM_BUFFER_CAPACITY: usize = 32;

const STREAM_BACKPRESSURE_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) struct AiOperationRegistry {
    pub(super) operations: Mutex<HashMap<String, ActiveAiOperation>>,
}

pub(super) struct ActiveAiOperation {
    cancellation: watch::Sender<()>,
    consent_key: AiConsentKey,
}

impl AiOperationRegistry {
    pub(super) fn start(
        self: &Arc<Self>,
        operation_id: String,
        consent_key: AiConsentKey,
    ) -> Result<AiOperation> {
        let (cancellation, receiver) = watch::channel(());
        let mut operations = self
            .operations
            .lock()
            .map_err(|_| operation_lock_error("ai_stream"))?;
        match operations.entry(operation_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(ActiveAiOperation {
                    cancellation,
                    consent_key,
                });
            }
            Entry::Occupied(_) => {
                return Err(CoreError::validation(
                    "ai_operation_in_progress",
                    "An AI operation with this identifier is already running",
                    "ai_stream",
                ));
            }
        }
        Ok(AiOperation {
            registry: self.clone(),
            operation_id,
            cancellation: receiver,
        })
    }

    pub(super) fn cancel(&self, operation_id: &str) -> Result<bool> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| operation_lock_error("ai_stream_cancel"))?;
        Ok(operations
            .get(operation_id)
            .is_some_and(|operation| operation.cancellation.send(()).is_ok()))
    }

    pub(super) fn cancel_matching(&self, consent_key: &AiConsentKey) -> Result<u32> {
        let operations = self
            .operations
            .lock()
            .map_err(|_| operation_lock_error("ai_consent_revoke"))?;
        Ok(operations
            .values()
            .filter(|operation| &operation.consent_key == consent_key)
            .filter(|operation| operation.cancellation.send(()).is_ok())
            .count() as u32)
    }
}

pub(super) struct AiOperation {
    registry: Arc<AiOperationRegistry>,
    operation_id: String,
    pub(super) cancellation: watch::Receiver<()>,
}

impl Drop for AiOperation {
    fn drop(&mut self) {
        if let Ok(mut operations) = self.registry.operations.lock() {
            operations.remove(&self.operation_id);
        }
    }
}

pub(super) async fn run_stream<S: AiStreamer>(
    mut operation: AiOperation,
    sender: mpsc::Sender<AiStreamFrame>,
    model: AiModelRef,
    request: ResolvedAiStreamRequest,
    streamer: Arc<S>,
) {
    let mut sequence = 1;
    if !send_stream_frame(
        &sender,
        &mut operation.cancellation,
        &operation.operation_id,
        &mut sequence,
        AiStreamEvent::Started {
            model: model.clone(),
        },
    )
    .await
    {
        let _ = send_stream_frame(
            &sender,
            &mut operation.cancellation,
            &operation.operation_id,
            &mut sequence,
            AiStreamEvent::Cancelled,
        )
        .await;
        return;
    }
    let stream_result = tokio::select! {
        changed = operation.cancellation.changed() => {
            if changed.is_ok() {
                let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Cancelled).await;
            }
            return;
        }
        result = streamer.stream(request) => result,
    };
    let mut stream = match stream_result {
        Ok(stream) => stream,
        Err(error) => {
            let _ = send_stream_frame(
                &sender,
                &mut operation.cancellation,
                &operation.operation_id,
                &mut sequence,
                AiStreamEvent::Error { error },
            )
            .await;
            return;
        }
    };
    loop {
        tokio::select! {
            changed = operation.cancellation.changed() => {
                if changed.is_ok() {
                    let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Cancelled).await;
                }
                return;
            }
            event = stream.next() => match event {
                Some(Ok(AiProviderStreamEvent::TextDelta(text))) => {
                    if !send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::TextDelta { text }).await {
                        return;
                    }
                }
                Some(Ok(AiProviderStreamEvent::Completed { usage, stop_reason, tool_calls })) => {
                    for call in tool_calls {
                        if !send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::ToolCall { call }).await {
                            return;
                        }
                    }
                    let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Completed {
                        summary: AiStreamSummary { model, usage, stop_reason },
                    }).await;
                    return;
                }
                Some(Err(error)) => {
                    let _ = send_stream_frame(&sender, &mut operation.cancellation, &operation.operation_id, &mut sequence, AiStreamEvent::Error { error }).await;
                    return;
                }
                None => {
                    let _ = send_stream_frame(
                        &sender,
                        &mut operation.cancellation,
                        &operation.operation_id,
                        &mut sequence,
                        AiStreamEvent::Error {
                            error: incomplete_stream_error(),
                        },
                    )
                    .await;
                    return;
                }
            }
        }
    }
}

async fn send_stream_frame(
    sender: &mpsc::Sender<AiStreamFrame>,
    cancellation: &mut watch::Receiver<()>,
    operation_id: &str,
    sequence: &mut u64,
    event: AiStreamEvent,
) -> bool {
    let frame = AiStreamFrame {
        operation_id: operation_id.into(),
        sequence: *sequence,
        event,
    };
    let sent = if matches!(&frame.event, AiStreamEvent::Cancelled) {
        matches!(
            tokio::time::timeout(STREAM_BACKPRESSURE_TIMEOUT, sender.send(frame)).await,
            Ok(Ok(()))
        )
    } else {
        tokio::select! {
            changed = cancellation.changed() => changed.is_err(),
            result = tokio::time::timeout(STREAM_BACKPRESSURE_TIMEOUT, sender.send(frame)) => matches!(result, Ok(Ok(()))),
        }
    };
    if sent {
        *sequence += 1;
    }
    sent
}

pub(super) fn validate_operation_id(operation_id: &str, operation: &str) -> Result<()> {
    uuid::Uuid::parse_str(operation_id).map_err(|_| {
        CoreError::validation(
            "ai_operation_invalid",
            "The AI operation identifier is invalid",
            operation,
        )
    })?;
    Ok(())
}

pub(super) fn operation_lock_error(operation: &str) -> CoreError {
    CoreError::new(
        "ai_operation_lock_unavailable",
        ErrorCategory::Transient,
        "The AI operation state is unavailable",
        operation,
    )
}

fn incomplete_stream_error() -> CoreError {
    let mut error = CoreError::new(
        "provider_stream_incomplete",
        ErrorCategory::Transient,
        "The AI provider stream ended without a terminal response",
        "ai_stream",
    );
    error.retryable = true;
    error
}
