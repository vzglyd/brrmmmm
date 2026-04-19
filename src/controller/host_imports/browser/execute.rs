use chromiumoxide::Browser;
use chromiumoxide::browser::BrowserConfig;
use futures::StreamExt as _;
use tokio::runtime::Runtime;

use crate::host::browser_request::{
    BrowserAction, BrowserActionResponse, BrowserCookie, SelectorKind,
};

pub(super) struct BrowserSession {
    pub runtime: Runtime,
    pub browser: Option<Browser>,
    pub active_page: Option<chromiumoxide::Page>,
}

impl BrowserSession {
    pub fn new() -> anyhow::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            runtime,
            browser: None,
            active_page: None,
        })
    }

    pub fn execute(&mut self, action: BrowserAction) -> BrowserActionResponse {
        match self.ensure_browser() {
            Ok(()) => {}
            Err(e) => {
                return BrowserActionResponse::err("browser_launch_failed", e.to_string());
            }
        }

        let browser = self.browser.as_ref().unwrap();
        let active_page = &mut self.active_page;
        self.runtime
            .block_on(run_action(browser, active_page, action))
    }

    fn ensure_browser(&mut self) -> anyhow::Result<()> {
        if self.browser.is_some() {
            return Ok(());
        }
        let mut config = BrowserConfig::builder().new_headless_mode();
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
        Ok(())
    }
}

async fn run_action(
    browser: &Browser,
    active_page: &mut Option<chromiumoxide::Page>,
    action: BrowserAction,
) -> BrowserActionResponse {
    match action {
        BrowserAction::Navigate { url } => {
            let page = match active_page.as_ref().cloned() {
                Some(page) => page,
                None => {
                    let pages = browser.pages().await.unwrap_or_default();
                    match pages.into_iter().next() {
                        Some(page) => page,
                        None => match browser.new_page(&url).await {
                            Ok(page) => {
                                *active_page = Some(page);
                                return BrowserActionResponse::ok();
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
            match page.goto(&url).await {
                Ok(_) => {
                    *active_page = Some(page);
                    BrowserActionResponse::ok()
                }
                Err(e) => BrowserActionResponse::err("navigation_failed", e.to_string()),
            }
        }
        BrowserAction::Fill { selector, value } => {
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
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
            let page = match current_page(browser, active_page).await {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match get_html(&page, selector.as_deref(), selector_kind, limit).await {
                Ok((html, count)) => BrowserActionResponse::ok_html(html, count),
                Err(e) => BrowserActionResponse::err("get_html_failed", e),
            }
        }
        BrowserAction::EvaluateJson { expression } => {
            let page = match current_page(browser, active_page).await {
                Ok(p) => p,
                Err(e) => return BrowserActionResponse::err("no_page", e),
            };
            match evaluate_json(&page, &expression).await {
                Ok(value) => BrowserActionResponse::ok_json(value),
                Err(e) => BrowserActionResponse::err("evaluate_json_failed", e),
            }
        }
    }
}

async fn current_page(
    browser: &Browser,
    active_page: &mut Option<chromiumoxide::Page>,
) -> Result<chromiumoxide::Page, String> {
    if let Some(page) = active_page.as_ref() {
        return Ok(page.clone());
    }

    let pages = browser.pages().await.map_err(|e| e.to_string())?;
    let page = pages
        .into_iter()
        .next()
        .ok_or_else(|| "no open page".to_string())?;
    *active_page = Some(page.clone());
    Ok(page)
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
