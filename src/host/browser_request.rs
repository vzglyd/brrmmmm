use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const WIRE_VERSION: u32 = 2;

fn default_timeout_ms() -> u32 {
    10_000
}

fn default_text_limit() -> u32 {
    20
}

fn default_selector_kind() -> SelectorKind {
    SelectorKind::Css
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectorKind {
    Css,
    #[serde(rename = "xpath")]
    XPath,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum BrowserAction {
    Navigate {
        url: String,
    },
    Fill {
        selector: String,
        value: String,
    },
    Click {
        selector: String,
    },
    Press {
        selector: String,
        key: String,
    },
    WaitForSelector {
        selector: String,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u32,
    },
    WaitForUrl {
        pattern: String,
        #[serde(default = "default_timeout_ms")]
        timeout_ms: u32,
    },
    CurrentUrl,
    GetCookies,
    GetText {
        selector: String,
        #[serde(default = "default_text_limit")]
        limit: u32,
    },
    GetHtml {
        #[serde(default)]
        selector: Option<String>,
        #[serde(default = "default_selector_kind")]
        selector_kind: SelectorKind,
        #[serde(default = "default_text_limit")]
        limit: u32,
    },
    EvaluateJson {
        expression: String,
    },
    Screenshot,
}

impl BrowserAction {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Navigate { .. } => "navigate",
            Self::Fill { .. } => "fill",
            Self::Click { .. } => "click",
            Self::Press { .. } => "press",
            Self::WaitForSelector { .. } => "wait_for_selector",
            Self::WaitForUrl { .. } => "wait_for_url",
            Self::CurrentUrl => "current_url",
            Self::GetCookies => "get_cookies",
            Self::GetText { .. } => "get_text",
            Self::GetHtml { .. } => "get_html",
            Self::EvaluateJson { .. } => "evaluate_json",
            Self::Screenshot => "screenshot",
        }
    }

    /// A loggable detail string that never includes secret values.
    pub fn detail(&self) -> String {
        match self {
            Self::Navigate { url } => url.clone(),
            Self::Fill { selector, .. } => selector.clone(), // value intentionally omitted
            Self::Click { selector } => selector.clone(),
            Self::Press { selector, key } => format!("{selector} key={key}"),
            Self::WaitForSelector {
                selector,
                timeout_ms,
            } => {
                format!("{selector} timeout={timeout_ms}ms")
            }
            Self::WaitForUrl {
                pattern,
                timeout_ms,
            } => {
                format!("{pattern} timeout={timeout_ms}ms")
            }
            Self::CurrentUrl => String::new(),
            Self::GetCookies => String::new(),
            Self::GetText { selector, limit } => format!("{selector} limit={limit}"),
            Self::GetHtml {
                selector,
                selector_kind,
                limit,
            } => match selector {
                Some(selector) => format!("{selector_kind:?} {selector} limit={limit}"),
                None => "document".to_string(),
            },
            Self::EvaluateJson { expression } => {
                format!("expression_len={}", expression.len())
            }
            Self::Screenshot => String::new(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum BrowserActionResponse {
    Ok {
        wire_version: u32,
        ok: bool,
    },
    OkUrl {
        wire_version: u32,
        ok: bool,
        url: String,
    },
    OkCookies {
        wire_version: u32,
        ok: bool,
        cookies: Vec<BrowserCookie>,
    },
    OkText {
        wire_version: u32,
        ok: bool,
        texts: Vec<String>,
    },
    OkHtml {
        wire_version: u32,
        ok: bool,
        html: String,
        count: usize,
    },
    OkJson {
        wire_version: u32,
        ok: bool,
        value: Value,
    },
    OkScreenshot {
        wire_version: u32,
        ok: bool,
        png_b64: String,
    },
    Err {
        wire_version: u32,
        ok: bool,
        error: String,
        message: String,
    },
}

impl BrowserActionResponse {
    pub fn ok() -> Self {
        Self::Ok {
            wire_version: WIRE_VERSION,
            ok: true,
        }
    }

    pub fn ok_url(url: String) -> Self {
        Self::OkUrl {
            wire_version: WIRE_VERSION,
            ok: true,
            url,
        }
    }

    pub fn ok_cookies(cookies: Vec<BrowserCookie>) -> Self {
        Self::OkCookies {
            wire_version: WIRE_VERSION,
            ok: true,
            cookies,
        }
    }

    pub fn ok_text(texts: Vec<String>) -> Self {
        Self::OkText {
            wire_version: WIRE_VERSION,
            ok: true,
            texts,
        }
    }

    pub fn ok_html(html: String, count: usize) -> Self {
        Self::OkHtml {
            wire_version: WIRE_VERSION,
            ok: true,
            html,
            count,
        }
    }

    pub fn ok_json(value: Value) -> Self {
        Self::OkJson {
            wire_version: WIRE_VERSION,
            ok: true,
            value,
        }
    }

    pub fn ok_screenshot(png_b64: String) -> Self {
        Self::OkScreenshot {
            wire_version: WIRE_VERSION,
            ok: true,
            png_b64,
        }
    }

    pub fn err(error: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Err {
            wire_version: WIRE_VERSION,
            ok: false,
            error: error.into(),
            message: message.into(),
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(
            self,
            Self::Ok { .. }
                | Self::OkUrl { .. }
                | Self::OkCookies { .. }
                | Self::OkText { .. }
                | Self::OkHtml { .. }
                | Self::OkJson { .. }
                | Self::OkScreenshot { .. }
        )
    }
}

#[derive(Debug, Serialize)]
pub struct BrowserCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    pub path: String,
    pub secure: bool,
    pub http_only: bool,
}

pub fn decode_action(bytes: &[u8]) -> anyhow::Result<BrowserAction> {
    let val: serde_json::Value = serde_json::from_slice(bytes)?;
    let version = val
        .get("wire_version")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    anyhow::ensure!(
        version == WIRE_VERSION,
        "unsupported browser wire_version {version}; expected {WIRE_VERSION}"
    );
    let action: BrowserAction = serde_json::from_value(val)?;
    Ok(action)
}

pub fn encode_response(response: &BrowserActionResponse) -> anyhow::Result<Vec<u8>> {
    Ok(serde_json::to_vec(response)?)
}
