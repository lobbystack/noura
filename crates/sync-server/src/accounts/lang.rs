//! The language of the account pages and of the mail about plans: French
//! or English, from a cookie the language switch sets, or else the
//! browser's `Accept-Language`.

use axum::extract::RawQuery;
use axum::http::header::{ACCEPT_LANGUAGE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};
use axum::response::Response;

use super::{cookie_value, field, redirect};

/// The cookie that holds the language you chose.
pub const LANG_COOKIE: &str = "noura_lang";

/// A language of the account pages.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    /// English.
    #[default]
    En,
    /// French, as written in Québec.
    Fr,
}

impl Lang {
    /// The language's code, as `lang` attributes and the cookie hold it.
    pub fn code(self) -> &'static str {
        match self {
            Self::En => "en",
            Self::Fr => "fr",
        }
    }

    /// The language a code names.
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "en" => Some(Self::En),
            "fr" => Some(Self::Fr),
            _ => None,
        }
    }

    /// The language a BCP 47 tag starts with, such as `fr-CA`.
    pub fn from_tag(tag: &str) -> Self {
        let tag = tag.trim().to_ascii_lowercase();
        if tag == "fr" || tag.starts_with("fr-") || tag.starts_with("fr_") {
            Self::Fr
        } else {
            Self::En
        }
    }

    /// The other language, which the switch offers.
    pub fn other(self) -> Self {
        match self {
            Self::En => Self::Fr,
            Self::Fr => Self::En,
        }
    }

    /// The other language's name, in itself.
    pub fn switch_label(self) -> &'static str {
        match self.other() {
            Self::En => "English",
            Self::Fr => "Français",
        }
    }

    /// Picks `en` or `fr` for this language.
    pub fn pick<'a>(self, en: &'a str, fr: &'a str) -> &'a str {
        match self {
            Self::En => en,
            Self::Fr => fr,
        }
    }
}

/// The language of a request: the cookie when it names one, else the
/// browser's first preference.
pub fn negotiate(headers: &HeaderMap) -> Lang {
    if let Some(lang) = cookie_value(headers, LANG_COOKIE).and_then(|c| Lang::from_code(&c)) {
        return lang;
    }
    headers
        .get(ACCEPT_LANGUAGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(',').next())
        .and_then(|first| first.split(';').next())
        .map_or(Lang::En, Lang::from_tag)
}

/// `GET /account/language?to={en|fr}&back={path}`: remembers the language
/// and goes back to the page, which must be an account page.
pub async fn switch(RawQuery(query): RawQuery) -> Response {
    let query = query.unwrap_or_default();
    let lang = field(query.as_bytes(), "to")
        .and_then(|code| Lang::from_code(&code))
        .unwrap_or_default();
    let back = field(query.as_bytes(), "back")
        .filter(|path| path.starts_with("/account") && !path.contains("//") && !path.contains('\\'))
        .unwrap_or_else(|| "/account".to_owned());
    let mut response = redirect(&back);
    let cookie = format!(
        "{LANG_COOKIE}={}; Path=/; SameSite=Lax; Max-Age=31536000",
        lang.code()
    );
    if let Ok(value) = HeaderValue::from_str(&cookie) {
        response.headers_mut().append(SET_COOKIE, value);
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_wins_over_the_browser() {
        let mut headers = HeaderMap::new();
        headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("fr-CA,en;q=0.8"));
        assert_eq!(negotiate(&headers), Lang::Fr);
        headers.insert("cookie", HeaderValue::from_static("noura_lang=en"));
        assert_eq!(negotiate(&headers), Lang::En);
        assert_eq!(negotiate(&HeaderMap::new()), Lang::En);
        assert_eq!(Lang::from_tag("FR_ca"), Lang::Fr);
    }
}
