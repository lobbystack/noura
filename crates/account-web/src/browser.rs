//! The browser side: the module's start function wires each passkey form
//! to its ceremony.

use serde_json::{Value, json};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{
    Event, HtmlFormElement, HtmlInputElement, Request, RequestCredentials, RequestInit, Response,
};
use webauthn_rs_proto::{
    CreationChallengeResponse, PublicKeyCredential, RegisterPublicKeyCredential,
    RequestChallengeResponse,
};

use crate::{Ceremony, Outcome, outcome};

fn window() -> Option<web_sys::Window> {
    web_sys::window()
}

fn explain(error: JsValue) -> String {
    // A refused or cancelled prompt rejects with a DOMException whose name
    // says which.
    let name = js_sys::Reflect::get(&error, &"name".into())
        .ok()
        .and_then(|v| v.as_string())
        .unwrap_or_default();
    match name.as_str() {
        "NotAllowedError" => "You cancelled, or the passkey prompt timed out. Try again.".into(),
        "InvalidStateError" => "This passkey is already on your account.".into(),
        _ => "Your browser couldn't use a passkey here. Try an email link.".into(),
    }
}

fn email_input(form: &HtmlFormElement) -> Option<HtmlInputElement> {
    form.query_selector("input[name=email]")
        .ok()
        .flatten()
        .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
}

/// Shows each form's passkey button and runs the form's ceremony when you
/// press it; the form's own submit button still posts it as a plain form.
/// A browser without WebAuthn keeps the buttons hidden.
#[wasm_bindgen(start)]
pub fn start() {
    let Some(window) = window() else {
        return;
    };
    let webauthn = js_sys::Reflect::has(&window, &"PublicKeyCredential".into()).unwrap_or(false);
    let (true, Some(document)) = (webauthn, window.document()) else {
        return;
    };
    let Ok(forms) = document.query_selector_all("form[data-passkey]") else {
        return;
    };
    for index in 0..forms.length() {
        let Some(form) = forms
            .item(index)
            .and_then(|node| node.dyn_into::<HtmlFormElement>().ok())
        else {
            continue;
        };
        let Some(ceremony) = form
            .dataset()
            .get("passkey")
            .and_then(|value| Ceremony::from_attribute(&value))
        else {
            continue;
        };
        let Some(button) = form.query_selector("[data-passkey-start]").ok().flatten() else {
            continue;
        };
        let _ = button.remove_attribute("hidden");
        let _ = form.class_list().add_1("passkey-ready");
        let target = form.clone();
        let handler = Closure::<dyn FnMut(Event)>::new(move |event: Event| {
            event.prevent_default();
            let form = target.clone();
            if ceremony.takes_email()
                && !email_input(&form).is_some_and(|input| input.report_validity())
            {
                return;
            }
            spawn_local(async move { run(form, ceremony).await });
        });
        let _ = button.add_event_listener_with_callback("click", handler.as_ref().unchecked_ref());
        // The page keeps its forms for its whole life.
        handler.forget();
    }
}

fn say(form: &HtmlFormElement, text: &str) {
    if let Ok(Some(status)) = form.query_selector("[data-status]") {
        status.set_text_content(Some(text));
    }
}

async fn run(form: HtmlFormElement, ceremony: Ceremony) {
    say(&form, "Waiting for your passkey…");
    match steps(&form, ceremony).await {
        Ok(Outcome::Open(path)) => {
            if let Some(window) = window() {
                let _ = window.location().set_href(path);
            }
        }
        Ok(Outcome::Say(text)) => say(&form, text),
        Err(text) => say(&form, &text),
    }
}

async fn steps(form: &HtmlFormElement, ceremony: Ceremony) -> Result<Outcome, String> {
    let window = window().ok_or("No browser window.")?;
    let (start, finish) = ceremony.routes();
    let body = if ceremony.takes_email() {
        let email = email_input(form)
            .map(|input| input.value())
            .unwrap_or_default();
        json!({ "email": email })
    } else {
        json!({})
    };
    let started = post(&window, start, &body).await?;
    let credentials = window.navigator().credentials();
    let credential = if ceremony.creates() {
        let options: CreationChallengeResponse = serde_json::from_value(started["options"].clone())
            .map_err(|_| "The server's passkey options don't parse.")?;
        let promise = credentials
            .create_with_options(&options.into())
            .map_err(explain)?;
        let created: web_sys::PublicKeyCredential = JsFuture::from(promise)
            .await
            .map_err(explain)?
            .dyn_into()
            .map_err(|_| "The browser returned no passkey.")?;
        serde_json::to_value(RegisterPublicKeyCredential::from(created))
    } else {
        let options: RequestChallengeResponse = serde_json::from_value(started["options"].clone())
            .map_err(|_| "The server's passkey options don't parse.")?;
        let promise = credentials
            .get_with_options(&options.into())
            .map_err(explain)?;
        let signed: web_sys::PublicKeyCredential = JsFuture::from(promise)
            .await
            .map_err(explain)?
            .dyn_into()
            .map_err(|_| "The browser returned no passkey.")?;
        serde_json::to_value(PublicKeyCredential::from(signed))
    }
    .map_err(|_| "The passkey doesn't encode.")?;
    let finished = post(
        &window,
        finish,
        &json!({ "ceremony": started["ceremony"], "credential": credential }),
    )
    .await?;
    Ok(outcome(finished["status"].as_str().unwrap_or_default()))
}

/// Posts JSON to the server's own origin and returns the JSON answer, or
/// the message of a refusal.
async fn post(window: &web_sys::Window, path: &str, body: &Value) -> Result<Value, String> {
    let init = RequestInit::new();
    init.set_method("POST");
    init.set_credentials(RequestCredentials::SameOrigin);
    init.set_body(&JsValue::from_str(&body.to_string()));
    let request = Request::new_with_str_and_init(path, &init).map_err(explain)?;
    request
        .headers()
        .set("content-type", "application/json")
        .map_err(explain)?;
    let unreachable = || "The server can't be reached. Try again.".to_owned();
    let response: Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|_| unreachable())?
        .dyn_into()
        .map_err(|_| unreachable())?;
    let text = JsFuture::from(response.text().map_err(|_| unreachable())?)
        .await
        .map_err(|_| unreachable())?
        .as_string()
        .unwrap_or_default();
    let value: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if !response.ok() {
        return Err(value["message"]
            .as_str()
            .unwrap_or("The server refused the passkey.")
            .to_owned());
    }
    Ok(value)
}
