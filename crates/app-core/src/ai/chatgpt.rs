//! Continue with ChatGPT: the browser sign-in that lets requests use your
//! ChatGPT plan, how it stands for Settings, and sign-out. `local-core`
//! runs the OAuth flow and keeps the tokens in the credential store; this
//! opens the browser, waits for it, picks the plan's first model and
//! says what went wrong in words for you.

use std::{sync::Arc, time::Duration};

use local_core::chatgpt::{
    CHATGPT_DEFAULT_MODEL, CHATGPT_PROVIDER, ChatGptEndpoints, ChatGptSignIn, MANAGE_USAGE_URL,
};

use super::{Ai, lock};
use crate::AppError;

/// How long a sign-in waits for the browser to come back.
const PATIENCE: Duration = Duration::from_secs(10 * 60);

/// Where you review and limit what Noura uses of your ChatGPT plan.
pub const CHATGPT_USAGE_URL: &str = MANAGE_USAGE_URL;

/// How Continue with ChatGPT stands, for Settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChatGptStatus {
    /// Requests can use your ChatGPT plan.
    pub signed_in: bool,
    /// The account's email, once you signed in.
    pub email: Option<String>,
    /// The browser is open on the sign-in, and Noura waits for it.
    pub waiting: bool,
    /// Why the last sign-in or sign-out didn't go as asked.
    pub problem: Option<String>,
    /// You just signed in for the first time, so Settings says once that
    /// Noura now uses your plan.
    pub first_sign_in: bool,
}

/// The sign-in under way, and what the last one left to say.
#[derive(Default)]
pub(super) struct Flow {
    /// Ends the sign-in that waits for the browser.
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    problem: Option<String>,
    first_sign_in: bool,
    /// The signed-in account's email, read once from the registration.
    email: Option<Option<String>>,
}

impl Ai {
    pub(crate) fn chatgpt_status(&self) -> ChatGptStatus {
        let mut flow = lock(&self.chatgpt);
        let store = self.store().ok();
        let signed_in = store.is_some_and(|store| {
            store
                .list_providers()
                .unwrap_or_default()
                .iter()
                .any(|provider| {
                    provider.id == CHATGPT_PROVIDER && provider.credential_ref.is_some()
                })
        });
        let email = if signed_in {
            flow.email
                .get_or_insert_with(|| {
                    store
                        .and_then(|store| store.chatgpt_registration().ok())
                        .and_then(|registration| registration.email)
                })
                .clone()
        } else {
            None
        };
        ChatGptStatus {
            signed_in,
            email,
            waiting: flow.cancel.is_some(),
            problem: flow.problem.clone(),
            first_sign_in: flow.first_sign_in,
        }
    }

    /// Open the ChatGPT sign-in in the browser and wait for it on the AI
    /// runtime. A sign-in already waiting stays.
    pub(crate) fn sign_in_with_chatgpt(self: &Arc<Self>) {
        let (cancel, cancelled) = tokio::sync::oneshot::channel();
        {
            let mut flow = lock(&self.chatgpt);
            if flow.cancel.is_some() {
                return;
            }
            flow.cancel = Some(cancel);
            flow.problem = None;
        }
        self.changed();
        // The flow runs on a thread of its own, as answers do, so the
        // service never ends inside its own runtime.
        let ai = self.clone();
        let started = std::thread::Builder::new()
            .name("chatgpt-sign-in".to_owned())
            .spawn(move || {
                let outcome = ai.runtime.block_on(async {
                    tokio::select! {
                        outcome = ai.chatgpt_sign_in() => outcome,
                        _ = cancelled => Ok(()),
                    }
                });
                {
                    let mut flow = lock(&ai.chatgpt);
                    flow.cancel = None;
                    if let Err(error) = outcome {
                        flow.problem = Some(error.message);
                    }
                }
                ai.changed();
            });
        if let Err(error) = started {
            tracing::warn!(%error, "the ChatGPT sign-in's thread didn't start");
            lock(&self.chatgpt).cancel = None;
        }
    }

    async fn chatgpt_sign_in(&self) -> Result<(), AppError> {
        let store = self.store()?;
        let mut registration = store.chatgpt_registration().map_err(sign_in_error)?;
        let sign_in = ChatGptSignIn::start(store.chatgpt_endpoints(), &registration)
            .await
            .map_err(sign_in_error)?;
        self.system.open_url(sign_in.url()).map_err(|error| {
            tracing::warn!(%error, "the browser didn't open for the ChatGPT sign-in");
            AppError::new(
                "chatgpt_browser_unavailable",
                "Noura couldn't open your browser to sign in.",
            )
        })?;
        let credential = sign_in.finish(PATIENCE).await.map_err(sign_in_error)?;
        // The plan's own list of models picks the first one, unless it
        // offers Noura's default.
        let model = match store.chatgpt_models(&credential.access_token).await {
            Ok(models) if !models.iter().any(|(slug, _)| slug == CHATGPT_DEFAULT_MODEL) => models
                .into_iter()
                .next()
                .map_or_else(|| CHATGPT_DEFAULT_MODEL.to_owned(), |(slug, _)| slug),
            _ => CHATGPT_DEFAULT_MODEL.to_owned(),
        };
        store
            .save_chatgpt(&credential, &model)
            .map_err(sign_in_error)?;
        registration = store.chatgpt_registration().map_err(sign_in_error)?;
        let mut flow = lock(&self.chatgpt);
        flow.email = Some(registration.email.clone());
        if !registration.notice_seen {
            flow.first_sign_in = true;
        }
        Ok(())
    }

    /// Stop waiting for the browser.
    pub(crate) fn cancel_chatgpt_sign_in(&self) {
        let cancel = lock(&self.chatgpt).cancel.take();
        if let Some(cancel) = cancel {
            let _ = cancel.send(());
        }
        self.changed();
    }

    /// Sign out of ChatGPT on the AI runtime: OpenAI ends the session,
    /// then the tokens leave the credential store.
    pub(crate) fn sign_out_chatgpt(self: &Arc<Self>) {
        let ai = self.clone();
        let started = std::thread::Builder::new()
            .name("chatgpt-sign-out".to_owned())
            .spawn(move || {
                let outcome = match ai.store() {
                    Ok(store) => ai
                        .runtime
                        .block_on(store.sign_out_chatgpt())
                        .map_err(sign_in_error),
                    Err(error) => Err(error),
                };
                {
                    let mut flow = lock(&ai.chatgpt);
                    flow.email = None;
                    flow.problem = match outcome {
                        Ok(true) => None,
                        Ok(false) => Some(
                            "Signed out here, but ChatGPT didn't confirm it, so disconnect \
                             Noura in ChatGPT settings."
                                .to_owned(),
                        ),
                        Err(error) => Some(error.message),
                    };
                }
                ai.changed();
            });
        if let Err(error) = started {
            tracing::warn!(%error, "the ChatGPT sign-out's thread didn't start");
        }
    }

    /// You saw the notice that Noura uses your plan.
    pub(crate) fn dismiss_chatgpt_notice(&self) {
        lock(&self.chatgpt).first_sign_in = false;
        if let Ok(store) = self.store()
            && let Ok(mut registration) = store.chatgpt_registration()
        {
            registration.notice_seen = true;
            if let Err(error) = store.save_chatgpt_registration(&registration) {
                tracing::warn!(code = %error.code, "the ChatGPT notice wasn't marked as seen");
            }
        }
        self.changed();
    }

    /// Sign in at `endpoints` in place of OpenAI's, as tests do.
    pub(crate) fn set_chatgpt_endpoints(&self, endpoints: ChatGptEndpoints) {
        if let Ok(store) = self.store() {
            store.set_chatgpt_endpoints(endpoints);
        }
    }
}

/// What a failed sign-in says, in one sentence with what to do next.
fn sign_in_error(error: local_core::CoreError) -> AppError {
    tracing::warn!(code = %error.code, "ChatGPT sign-in didn't finish");
    let message = match error.code.as_str() {
        "chatgpt_sign_in_declined" => "You declined in ChatGPT, so Noura isn't using your plan.",
        "chatgpt_sign_in_timed_out" => {
            "The sign-in timed out, so choose Continue with ChatGPT again."
        }
        "chatgpt_plan_not_granted" => {
            "This ChatGPT account can't share its plan with Noura, so use an API key instead."
        }
        "chatgpt_account_mismatch" => {
            "That's a different ChatGPT account, so sign in with the one you used before."
        }
        "chatgpt_unreachable" => "Noura couldn't reach ChatGPT, so check your connection.",
        "credential_store_error" => {
            "Noura couldn't save the sign-in to the keychain, so try again."
        }
        _ => "The ChatGPT sign-in didn't finish, so try again.",
    };
    AppError::new(&error.code, message)
}
