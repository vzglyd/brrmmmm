use std::sync::{Arc, Mutex};

use chromiumoxide::Browser;
use chromiumoxide::browser::BrowserConfig;
use chromiumoxide::cdp::browser_protocol::fetch::{
    ContinueRequestParams, EventRequestPaused, HeaderEntry,
};
use chromiumoxide::cdp::browser_protocol::network::SetUserAgentOverrideParams;
use chromiumoxide::cdp::browser_protocol::page::CaptureScreenshotParams;
use futures::StreamExt as _;
use tokio::runtime::Runtime;

use crate::attestation;
use crate::host::HostState;
use crate::host::browser_request::{
    BrowserAction, BrowserActionResponse, BrowserCookie, SelectorKind,
};
use crate::mission_state::{self, CAP_BROWSER};

pub(super) struct BrowserSession {
    pub runtime: Runtime,
    pub browser: Option<Browser>,
    pub active_page: Option<chromiumoxide::Page>,
    pub shared: Arc<Mutex<HostState>>,
    pub last_applied_ua: String,
    pub interception_started: bool,
}

impl BrowserSession {
    pub fn new(shared: Arc<Mutex<HostState>>) -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            runtime,
            browser: None,
            active_page: None,
            shared,
            last_applied_ua: String::new(),
            interception_started: false,
        })
    }

    pub fn execute(&mut self, action: BrowserAction) -> BrowserActionResponse {
        match self.ensure_browser() {
            Ok(()) => {}
            Err(e) => {
                return BrowserActionResponse::err("browser_launch_failed", e.to_string());
            }
        }

        // Apply CDP UA override if the sidecar changed the UA after the browser was launched.
        let current_ua = self.shared.lock().unwrap().full_user_agent(None);
        if current_ua != self.last_applied_ua {
            if let Some(page) = self.active_page.as_ref().cloned() {
                let ua_clone = current_ua.clone();
                self.runtime.block_on(async move {
                    let _ = page
                        .execute(SetUserAgentOverrideParams::new(ua_clone))
                        .await;
                });
            }
            self.last_applied_ua = current_ua.clone();
        }

        let browser = self.browser.as_ref().unwrap();
        let active_page = &mut self.active_page;
        let shared = self.shared.clone();
        let interception_started = &mut self.interception_started;
        let user_agent = current_ua;
        self.runtime.block_on(run_action(
            browser,
            active_page,
            action,
            shared,
            interception_started,
            &user_agent,
        ))
    }

    fn ensure_browser(&mut self) -> anyhow::Result<()> {
        if self.browser.is_some() {
            return Ok(());
        }
        let ua = self.shared.lock().unwrap().full_user_agent(None);
        let mut config = BrowserConfig::builder()
            .new_headless_mode()
            .enable_request_intercept()
            .arg(format!("--user-agent={ua}"));
        if browser_headless_disabled() {
            config = config.with_head();
        }
        let config = config.build().map_err(|e| anyhow::anyhow!("{e}"))?;
        let (browser, mut handler) = self.runtime.block_on(Browser::launch(config))?;
        self.runtime.spawn(async move {
            loop {
                match handler.next().await {
                    Some(_) => {}
                    None => break,
                }
            }
        });
        self.browser = Some(browser);
        self.last_applied_ua = ua;
        Ok(())
    }
}

async fn run_action(
    browser: &Browser,
    active_page: &mut Option<chromiumoxide::Page>,
    action: BrowserAction,
    shared: Arc<Mutex<HostState>>,
    interception_started: &mut bool,
    user_agent: &str,
) -> BrowserActionResponse {
    match action {
        BrowserAction::Navigate { url } => {
            let page = match active_page.as_ref().cloned() {
                Some(page) => page,
                None => {
                    let pages = browser.pages().await.unwrap_or_default();
                    match pages.into_iter().next() {
                        Some(page) => page,
                        None => match browser.new_page("about:blank").await {
                            Ok(page) => {
                                *active_page = Some(page);
                                active_page.as_ref().cloned().unwrap()
                            }
                            Err(e) => {
                                return BrowserActionResponse::err(
                                    "navigation_failed",
                                    e.to_string(),
                                );
                            }
                        },
                    }
                }
            };
            if let Err(e) =
                ensure_page_attestation(&page, shared.clone(), interception_started, user_agent)
                    .await
            {
                return BrowserActionResponse::err("browser_attestation_failed", e);
            }
            match page.goto(&url).await {
                Ok(_) => {
                    *active_page = Some(page);
                    BrowserActionResponse::ok()
                }
                Err(e) => BrowserActionResponse::err("navigation_failed", e.to_string()),
            }
        }
        BrowserAction::Fill { selector, value } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match page.find_element(&selector).await {
                Ok(el) => match el.click().await {
                    Ok(_) => match el.type_str(&value).await {
                        Ok(_) => BrowserActionResponse::ok(),
                        Err(e) => BrowserActionResponse::err("fill_failed", e.to_string()),
                    },
                    Err(e) => BrowserActionResponse::err("fill_failed", e.to_string()),
                },
                Err(e) => BrowserActionResponse::err(
                    "element_not_found",
                    format!("no element matches '{selector}': {e}"),
                ),
            }
        }
        BrowserAction::Click { selector } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match page.find_element(&selector).await {
                Ok(el) => match el.click().await {
                    Ok(_) => BrowserActionResponse::ok(),
                    Err(e) => BrowserActionResponse::err("click_failed", e.to_string()),
                },
                Err(e) => BrowserActionResponse::err(
                    "element_not_found",
                    format!("no element matches '{selector}': {e}"),
                ),
            }
        }
        BrowserAction::Press { selector, key } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match page.find_element(&selector).await {
                Ok(el) => match el.press_key(&key).await {
                    Ok(_) => BrowserActionResponse::ok(),
                    Err(e) => BrowserActionResponse::err("press_failed", e.to_string()),
                },
                Err(e) => BrowserActionResponse::err(
                    "element_not_found",
                    format!("no element matches '{selector}': {e}"),
                ),
            }
        }
        BrowserAction::WaitForSelector {
            selector,
            timeout_ms,
        } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            let timeout = std::time::Duration::from_millis(timeout_ms as u64);
            match tokio::time::timeout(timeout, wait_for_selector_match(&page, &selector)).await {
                Ok(()) => BrowserActionResponse::ok(),
                Err(_) => BrowserActionResponse::err(
                    "selector_timeout",
                    format!("'{selector}' not found within {timeout_ms}ms"),
                ),
            }
        }
        BrowserAction::WaitForUrl {
            pattern,
            timeout_ms,
        } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            let timeout = std::time::Duration::from_millis(timeout_ms as u64);
            match tokio::time::timeout(timeout, wait_for_url_match(&page, &pattern)).await {
                Ok(Ok(())) => BrowserActionResponse::ok(),
                Ok(Err(e)) => BrowserActionResponse::err("wait_for_url_failed", e),
                Err(_) => BrowserActionResponse::err(
                    "url_timeout",
                    format!("URL matching '{pattern}' not seen within {timeout_ms}ms"),
                ),
            }
        }
        BrowserAction::CurrentUrl => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match page.url().await {
                Ok(Some(url)) => BrowserActionResponse::ok_url(url),
                Ok(None) => BrowserActionResponse::err("no_url", "page has no URL"),
                Err(e) => BrowserActionResponse::err("current_url_failed", e.to_string()),
            }
        }
        BrowserAction::GetCookies => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match page.get_cookies().await {
                Ok(cookies) => {
                    let out = cookies
                        .into_iter()
                        .map(|c| BrowserCookie {
                            name: c.name,
                            value: c.value,
                            domain: c.domain,
                            path: c.path,
                            secure: c.secure,
                            http_only: c.http_only,
                        })
                        .collect();
                    BrowserActionResponse::ok_cookies(out)
                }
                Err(e) => BrowserActionResponse::err("get_cookies_failed", e.to_string()),
            }
        }
        BrowserAction::GetText { selector, limit } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match get_text(&page, &selector, limit).await {
                Ok(texts) => BrowserActionResponse::ok_text(texts),
                Err(e) => BrowserActionResponse::err("get_text_failed", e),
            }
        }
        BrowserAction::GetHtml {
            selector,
            selector_kind,
            limit,
        } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match get_html(&page, selector.as_deref(), selector_kind, limit).await {
                Ok((html, count)) => BrowserActionResponse::ok_html(html, count),
                Err(e) => BrowserActionResponse::err("get_html_failed", e),
            }
        }
        BrowserAction::EvaluateJson { expression } => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match evaluate_json(&page, &expression).await {
                Ok(value) => BrowserActionResponse::ok_json(value),
                Err(e) => BrowserActionResponse::err("evaluate_json_failed", e),
            }
        }
        BrowserAction::Screenshot => {
            let page = match current_page(
                browser,
                active_page,
                shared.clone(),
                interception_started,
                user_agent,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            let params = CaptureScreenshotParams::builder().build();
            match page.screenshot(params).await {
                Ok(bytes) => BrowserActionResponse::ok_screenshot(base64_encode(&bytes)),
                Err(e) => BrowserActionResponse::err("screenshot_failed", e.to_string()),
            }
        }
    }
}

async fn current_page(
    browser: &Browser,
    active_page: &mut Option<chromiumoxide::Page>,
    shared: Arc<Mutex<HostState>>,
    interception_started: &mut bool,
    user_agent: &str,
) -> Result<chromiumoxide::Page, String> {
    let page = if let Some(page) = active_page.as_ref() {
        page.clone()
    } else {
        let pages = browser.pages().await.map_err(|e| e.to_string())?;
        let page = pages
            .into_iter()
            .next()
            .ok_or_else(|| "no open page".to_string())?;
        *active_page = Some(page.clone());
        page
    };
    ensure_page_attestation(&page, shared, interception_started, user_agent).await?;
    Ok(page)
}

async fn ensure_page_attestation(
    page: &chromiumoxide::Page,
    shared: Arc<Mutex<HostState>>,
    interception_started: &mut bool,
    user_agent: &str,
) -> Result<(), String> {
    if !user_agent.is_empty() {
        page.execute(SetUserAgentOverrideParams::new(user_agent.to_string()))
            .await
            .map_err(|error| error.to_string())?;
    }
    if *interception_started {
        return Ok(());
    }

    let mut events = page
        .event_listener::<EventRequestPaused>()
        .await
        .map_err(|error| error.to_string())?;
    let page = page.clone();
    tokio::spawn(async move {
        while let Some(event) = events.next().await {
            let _ = continue_attested_request(&page, shared.clone(), event.as_ref()).await;
        }
    });
    *interception_started = true;
    Ok(())
}

async fn continue_attested_request(
    page: &chromiumoxide::Page,
    shared: Arc<Mutex<HostState>>,
    event: &EventRequestPaused,
) -> Result<(), String> {
    if event.response_status_code.is_some() {
        return page
            .execute(ContinueRequestParams::new(event.request_id.clone()))
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
    }

    let Some(binding) =
        attestation::binding_from_url(&event.request.method, &event.request.url, None)
    else {
        return page
            .execute(ContinueRequestParams::new(event.request_id.clone()))
            .await
            .map(|_| ())
            .map_err(|error| error.to_string());
    };

    let original = cdp_headers_to_pairs(event.request.headers.inner());
    let headers = {
        let mut host = shared.lock().unwrap();
        let behavior =
            mission_state::network_event(&binding.method, &binding.authority, &binding.path);
        let envelope =
            host.signed_envelope_for_request(CAP_BROWSER, "browser_http", &behavior, &binding);
        let user_agent = host.full_user_agent(envelope.as_ref());
        attestation::merge_host_headers(original, &user_agent, envelope.as_ref())
    };
    let entries: Vec<HeaderEntry> = headers
        .into_iter()
        .map(|(name, value)| HeaderEntry::new(name, value))
        .collect();
    let params = ContinueRequestParams::builder()
        .request_id(event.request_id.clone())
        .headers(entries)
        .build()
        .map_err(|error| error.to_string())?;
    page.execute(params)
        .await
        .map(|_| ())
        .map_err(|error| error.to_string())
}

fn cdp_headers_to_pairs(headers: &serde_json::Value) -> Vec<(String, String)> {
    headers
        .as_object()
        .map(|object| {
            object
                .iter()
                .map(|(name, value)| {
                    let value = value
                        .as_str()
                        .map(ToOwned::to_owned)
                        .unwrap_or_else(|| value.to_string());
                    (name.clone(), value)
                })
                .collect()
        })
        .unwrap_or_default()
}

async fn wait_for_selector_match(page: &chromiumoxide::Page, selector: &str) {
    loop {
        if page.find_element(selector).await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn wait_for_url_match(page: &chromiumoxide::Page, pattern: &str) -> Result<(), String> {
    loop {
        match page.url().await {
            Ok(Some(url)) if glob_matches(pattern, &url) => return Ok(()),
            Ok(_) => {}
            Err(e) => return Err(e.to_string()),
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
}

async fn get_text(
    page: &chromiumoxide::Page,
    selector: &str,
    limit: u32,
) -> Result<Vec<String>, String> {
    let selector = serde_json::to_string(selector).map_err(|e| e.to_string())?;
    let limit = limit.clamp(1, 100);
    let expression = format!(
        "Array.from(document.querySelectorAll({selector}))
            .slice(0, {limit})
            .map((el) => (el.innerText || el.textContent || '').trim())
            .filter((text) => text.length > 0)"
    );
    page.evaluate_expression(expression)
        .await
        .map_err(|e| e.to_string())?
        .into_value::<Vec<String>>()
        .map_err(|e| e.to_string())
}

async fn get_html(
    page: &chromiumoxide::Page,
    selector: Option<&str>,
    selector_kind: SelectorKind,
    limit: u32,
) -> Result<(String, usize), String> {
    let Some(selector) = selector else {
        return page
            .content()
            .await
            .map(|html| (html, 1))
            .map_err(|e| e.to_string());
    };

    let selector = serde_json::to_string(selector).map_err(|e| e.to_string())?;
    let limit = limit.clamp(1, 100);
    let expression = match selector_kind {
        SelectorKind::Css => format!(
            "(() => Array.from(document.querySelectorAll({selector}))
                .slice(0, {limit})
                .map((node) => node.outerHTML || new XMLSerializer().serializeToString(node)))()"
        ),
        SelectorKind::XPath => format!(
            "(() => {{
                const result = document.evaluate(
                    {selector},
                    document,
                    null,
                    XPathResult.ORDERED_NODE_SNAPSHOT_TYPE,
                    null
                );
                const html = [];
                const count = Math.min(result.snapshotLength, {limit});
                for (let index = 0; index < count; index += 1) {{
                    const node = result.snapshotItem(index);
                    html.push(node.outerHTML || new XMLSerializer().serializeToString(node));
                }}
                return html;
            }})()"
        ),
    };
    let htmls = page
        .evaluate_expression(expression)
        .await
        .map_err(|e| e.to_string())?
        .into_value::<Vec<String>>()
        .map_err(|e| e.to_string())?;
    Ok((htmls.join("\n"), htmls.len()))
}

async fn evaluate_json(
    page: &chromiumoxide::Page,
    expression: &str,
) -> Result<serde_json::Value, String> {
    page.evaluate_expression(expression)
        .await
        .map_err(|e| e.to_string())?
        .into_value::<serde_json::Value>()
        .map_err(|e| e.to_string())
}

/// Simple glob matching: `*` matches any sequence of chars, `?` matches one char.
fn glob_matches(pattern: &str, s: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = s.chars().collect();
    glob_rec(&p, &t, 0, 0)
}

fn glob_rec(p: &[char], t: &[char], pi: usize, ti: usize) -> bool {
    if pi == p.len() {
        return ti == t.len();
    }
    if p[pi] == '*' {
        if glob_rec(p, t, pi + 1, ti) {
            return true;
        }
        if ti < t.len() {
            return glob_rec(p, t, pi, ti + 1);
        }
        return false;
    }
    if ti < t.len() && (p[pi] == '?' || p[pi] == t[ti]) {
        return glob_rec(p, t, pi + 1, ti + 1);
    }
    false
}

fn base64_encode(bytes: &[u8]) -> String {
    const CHARS: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0];
        let b1 = if chunk.len() > 1 { chunk[1] } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] } else { 0 };
        out.push(CHARS[(b0 >> 2) as usize] as char);
        out.push(CHARS[((b0 & 3) << 4 | b1 >> 4) as usize] as char);
        if chunk.len() > 1 {
            out.push(CHARS[((b1 & 0xf) << 2 | b2 >> 6) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(CHARS[(b2 & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

fn browser_headless_disabled() -> bool {
    std::env::var("BRRMMMM_BROWSER_HEADLESS")
        .map(|value| {
            let value = value.trim();
            value == "0"
                || value.eq_ignore_ascii_case("false")
                || value.eq_ignore_ascii_case("no")
                || value.eq_ignore_ascii_case("off")
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::base64_encode;

    #[test]
    fn base64_encoder_handles_padding() {
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }
}
