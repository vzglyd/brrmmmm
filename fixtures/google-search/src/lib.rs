use serde::{Deserialize, Serialize};
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

const DESCRIBE: &[u8] = include_bytes!("describe.json");
const SEARCH_FIELD: &str = r#"textarea[name="q"], input[name="q"]"#;
const RESULT_TITLE: &str = "#search a[href] h3, #rso a[href] h3";
const GOOGLE_RESULTS_JS: &str = r#"
(() => {
    const normalize = (value) => (value || '').replace(/\s+/g, ' ').trim();
    const linesFor = (value) => (value || '')
        .split(/\n+/)
        .map(normalize)
        .filter(Boolean);
    const unwrapGoogleUrl = (href) => {
        try {
            const parsed = new URL(href);
            const q = parsed.searchParams.get('q');
            if (parsed.hostname.endsWith('google.com') && parsed.pathname === '/url' && q) {
                return q;
            }
        } catch (_) {}
        return href;
    };
    const displayUrlFor = (url, container) => {
        const cite = container ? container.querySelector('cite') : null;
        const citeText = normalize(cite && (cite.innerText || cite.textContent));
        if (citeText) return citeText;
        try {
            return new URL(url).hostname;
        } catch (_) {
            return null;
        }
    };
    const isNoise = (line, title, displayUrl) => {
        if (!line || line === title || line === displayUrl) return true;
        if (line === 'Cached' || line === 'Similar' || line === 'Translate this page') return true;
        if (line === 'More results' || line === 'About this result') return true;
        return false;
    };
    const containerFor = (anchor) => (
        anchor.closest('div.g') ||
        anchor.closest('div.MjjYud') ||
        anchor.closest('div[data-sokoban-container]') ||
        anchor.closest('div[jscontroller]') ||
        anchor.closest('div')
    );

    const anchors = Array.from(document.querySelectorAll('#search a[href], #rso a[href]'));
    const seen = new Set();
    const results = [];
    for (const anchor of anchors) {
        const heading = anchor.querySelector('h3');
        if (!heading) continue;
        const title = normalize(heading.innerText || heading.textContent);
        const url = unwrapGoogleUrl(anchor.href || anchor.getAttribute('href') || '');
        if (!title || !url || seen.has(`${title}\n${url}`)) continue;

        const container = containerFor(anchor);
        const rawText = (container && (container.innerText || container.textContent)) ||
            anchor.innerText ||
            anchor.textContent ||
            '';
        const text = normalize(rawText);
        const display_url = displayUrlFor(url, container);
        const snippet = linesFor(rawText)
            .filter((line) => !isNoise(line, title, display_url))
            .slice(0, 4)
            .join(' ');

        seen.add(`${title}\n${url}`);
        results.push({
            rank: results.length + 1,
            title,
            url,
            display_url,
            snippet,
            text,
            html: container ? container.outerHTML : anchor.outerHTML,
        });
        if (results.length >= 10) break;
    }
    return results;
})()
"#;

#[derive(Clone, Copy, Serialize)]
struct Topic {
    trec_id: u16,
    title: &'static str,
}

const TOPICS: [Topic; 50] = [
    Topic {
        trec_id: 301,
        title: "International Organized Crime",
    },
    Topic {
        trec_id: 302,
        title: "Poliomyelitis and Post-Polio",
    },
    Topic {
        trec_id: 303,
        title: "Hubble Telescope Achievements",
    },
    Topic {
        trec_id: 304,
        title: "Endangered Species Mammals",
    },
    Topic {
        trec_id: 305,
        title: "Most Dangerous Vehicles",
    },
    Topic {
        trec_id: 306,
        title: "African Civilian Deaths",
    },
    Topic {
        trec_id: 307,
        title: "New Hydroelectric Projects",
    },
    Topic {
        trec_id: 308,
        title: "Implant Dentistry",
    },
    Topic {
        trec_id: 309,
        title: "Rap and Crime",
    },
    Topic {
        trec_id: 310,
        title: "Radio Waves and Brain Cancer",
    },
    Topic {
        trec_id: 311,
        title: "Industrial Espionage",
    },
    Topic {
        trec_id: 312,
        title: "Hydroponics",
    },
    Topic {
        trec_id: 313,
        title: "Magnetic Levitation Maglev",
    },
    Topic {
        trec_id: 314,
        title: "Marine Vegetation",
    },
    Topic {
        trec_id: 315,
        title: "Unexplained Highway Accidents",
    },
    Topic {
        trec_id: 316,
        title: "Polygamy Polyandry Polygyny",
    },
    Topic {
        trec_id: 317,
        title: "Unsolicited Faxes",
    },
    Topic {
        trec_id: 318,
        title: "Best Retirement Country",
    },
    Topic {
        trec_id: 319,
        title: "New Fuel Sources",
    },
    Topic {
        trec_id: 320,
        title: "Undersea Fiber Optic Cable",
    },
    Topic {
        trec_id: 321,
        title: "Women in Parliaments",
    },
    Topic {
        trec_id: 322,
        title: "International Art Crime",
    },
    Topic {
        trec_id: 323,
        title: "Literary Journalism",
    },
    Topic {
        trec_id: 324,
        title: "Argentina British Relations",
    },
    Topic {
        trec_id: 325,
        title: "Cult Lifestyles",
    },
    Topic {
        trec_id: 326,
        title: "Ferry Sinkings",
    },
    Topic {
        trec_id: 327,
        title: "Modern Slavery",
    },
    Topic {
        trec_id: 328,
        title: "Pope Beatifications",
    },
    Topic {
        trec_id: 329,
        title: "Mexican Air Pollution",
    },
    Topic {
        trec_id: 330,
        title: "Iran Iraq Cooperation",
    },
    Topic {
        trec_id: 331,
        title: "World Bank Criticism",
    },
    Topic {
        trec_id: 332,
        title: "Income Tax Evasion",
    },
    Topic {
        trec_id: 333,
        title: "Antibiotics Bacteria Disease",
    },
    Topic {
        trec_id: 334,
        title: "Export Controls Cryptography",
    },
    Topic {
        trec_id: 335,
        title: "Adoption Biological Parents",
    },
    Topic {
        trec_id: 336,
        title: "Black Bear Attacks",
    },
    Topic {
        trec_id: 337,
        title: "Viral Hepatitis",
    },
    Topic {
        trec_id: 338,
        title: "Risk of Aspirin",
    },
    Topic {
        trec_id: 339,
        title: "Alzheimer's Drug Treatment",
    },
    Topic {
        trec_id: 340,
        title: "Land Mine Ban",
    },
    Topic {
        trec_id: 341,
        title: "Airport Security",
    },
    Topic {
        trec_id: 342,
        title: "Diplomatic Expulsion",
    },
    Topic {
        trec_id: 343,
        title: "Police Deaths",
    },
    Topic {
        trec_id: 344,
        title: "Abuses of E-Mail",
    },
    Topic {
        trec_id: 345,
        title: "Overseas Tobacco Sales",
    },
    Topic {
        trec_id: 346,
        title: "Educational Standards",
    },
    Topic {
        trec_id: 347,
        title: "Wildlife Extinction",
    },
    Topic {
        trec_id: 348,
        title: "Agoraphobia",
    },
    Topic {
        trec_id: 349,
        title: "Metabolism",
    },
    Topic {
        trec_id: 350,
        title: "Health and Computer Terminals",
    },
];

#[link(wasm_import_module = "brrmmmm_host")]
unsafe extern "C" {
    fn host_call(ptr: i32, len: i32) -> i32;
    fn host_response_len() -> i32;
    fn host_response_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn mission_outcome_report(ptr: i32, len: i32) -> i32;
    fn log_info(ptr: i32, len: i32) -> i32;
}

#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn random_get(buf: *mut u8, buf_len: usize) -> u16;
}

#[derive(Debug, Default, Deserialize)]
struct BrowserResponse {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    value: Option<serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
struct SearchResult {
    rank: usize,
    title: String,
    url: String,
    display_url: Option<String>,
    snippet: String,
    text: String,
    html: String,
}

#[derive(Serialize)]
struct PublishedOutput {
    ok: bool,
    source: &'static str,
    topic: Topic,
    query: String,
    url: String,
    result_count: usize,
    results: Vec<SearchResult>,
}

#[derive(Serialize)]
struct FailureOutput {
    ok: bool,
    source: &'static str,
    error: String,
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_abi_version() -> u32 {
    3
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_describe_ptr() -> i32 {
    DESCRIBE.as_ptr() as i32
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_describe_len() -> i32 {
    DESCRIBE.len() as i32
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_start() {
    match run() {
        Ok(output) => {
            publish_json(&output);
            report_outcome("published", "published_output", "google fixture published output");
        }
        Err(error) => {
            publish_json(&FailureOutput {
                ok: false,
                source: "google",
                error: error.clone(),
            });
            report_outcome("terminal_failure", "google_search_failed", &error);
        }
    }
}

fn run() -> Result<PublishedOutput, String> {
    let topic = random_topic();
    let query = topic.title.to_string();

    log("opening google.com");
    action(json!({
        "wire_version": 1,
        "action": "navigate",
        "url": "https://www.google.com/"
    }))?;

    log("waiting for search field");
    action(json!({
        "wire_version": 1,
        "action": "wait_for_selector",
        "selector": SEARCH_FIELD,
        "timeout_ms": 15_000
    }))?;

    log(&format!(
        "selected TREC topic {}: {}",
        topic.trec_id, topic.title
    ));
    action(json!({
        "wire_version": 1,
        "action": "fill",
        "selector": SEARCH_FIELD,
        "value": query
    }))?;

    log("submitting query");
    action(json!({
        "wire_version": 1,
        "action": "press",
        "selector": SEARCH_FIELD,
        "key": "Enter"
    }))?;

    log("waiting for Google search navigation");
    if let Err(error) = action(json!({
        "wire_version": 1,
        "action": "wait_for_url",
        "pattern": "*://www.google.com/search*",
        "timeout_ms": 10_000
    })) {
        log(&format!(
            "search navigation did not appear after Enter ({error}); navigating directly"
        ));
        action(json!({
            "wire_version": 1,
            "action": "navigate",
            "url": format!("https://www.google.com/search?hl=en&num=10&q={}", url_encode(&query))
        }))?;
    }

    log("waiting for page body");
    action(json!({
        "wire_version": 1,
        "action": "wait_for_selector",
        "selector": "body",
        "timeout_ms": 15_000
    }))?;

    let mut url = action(json!({
        "wire_version": 1,
        "action": "current_url"
    }))?
    .url
    .unwrap_or_default();
    if url.contains("/sorry/") {
        return Err(format!(
            "Google served an interstitial instead of search results: {url}"
        ));
    }

    log("waiting for search result nodes");
    if let Err(error) = action(json!({
        "wire_version": 1,
        "action": "wait_for_selector",
        "selector": RESULT_TITLE,
        "timeout_ms": 20_000
    })) {
        url = action(json!({
            "wire_version": 1,
            "action": "current_url"
        }))?
        .url
        .unwrap_or_default();
        return Err(format!(
            "Google page loaded but no search result nodes appeared at '{RESULT_TITLE}': {error}; current_url={url}"
        ));
    }

    url = action(json!({
        "wire_version": 1,
        "action": "current_url"
    }))?
    .url
    .unwrap_or_default();

    let results = get_search_results()?;
    if results.is_empty() {
        return Err(format!(
            "Google result nodes were present, but no structured results could be extracted; current_url={url}"
        ));
    }

    Ok(PublishedOutput {
        ok: true,
        source: "google",
        topic,
        query,
        url,
        result_count: results.len(),
        results,
    })
}

fn get_search_results() -> Result<Vec<SearchResult>, String> {
    let response = action(json!({
        "wire_version": 1,
        "action": "evaluate_json",
        "expression": GOOGLE_RESULTS_JS
    }))?;
    let value = response
        .value
        .ok_or_else(|| "evaluate_json did not return a value".to_string())?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

fn action(request: serde_json::Value) -> Result<BrowserResponse, String> {
    let data = host_call_json("browser", request)?;
    serde_json::from_value(data).map_err(|error| error.to_string())
}

fn host_call_json(capability: &str, mut request: serde_json::Value) -> Result<serde_json::Value, String> {
    let object = request
        .as_object_mut()
        .ok_or_else(|| "host call request must be an object".to_string())?;
    object.insert("wire_version".to_string(), json!(2));
    object.insert("capability".to_string(), json!(capability));
    let payload = serde_json::to_vec(&request).map_err(|error| error.to_string())?;

    let rc = unsafe { host_call(payload.as_ptr() as i32, payload.len() as i32) };
    if rc != 0 {
        return Err(format!("host_call rc={rc}"));
    }

    let len = unsafe { host_response_len() };
    if len <= 0 {
        return Err("empty host response".to_string());
    }

    let mut buf = vec![0u8; len as usize];
    let read = unsafe { host_response_read(buf.as_mut_ptr() as i32, len) };
    if read != len {
        return Err(format!("host response read mismatch: got={read} want={len}"));
    }

    let response: serde_json::Value =
        serde_json::from_slice(&buf).map_err(|error| error.to_string())?;
    if response.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok(response
            .get("data")
            .cloned()
            .unwrap_or_else(|| json!({})))
    } else {
        Err(response["error"]["message"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(&buf).into_owned()))
    }
}

fn random_topic() -> Topic {
    TOPICS[random_index(TOPICS.len())]
}

fn random_index(len: usize) -> usize {
    let mut bytes = [0u8; 8];
    let errno = unsafe { random_get(bytes.as_mut_ptr(), bytes.len()) };
    let seed = if errno == 0 {
        u64::from_le_bytes(bytes)
    } else {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0)
    };
    seed as usize % len
}

fn url_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(hex_nibble(byte >> 4));
                out.push(hex_nibble(byte & 0xf));
            }
        }
    }
    out
}

fn hex_nibble(n: u8) -> char {
    if n < 10 {
        (b'0' + n) as char
    } else {
        (b'A' + n - 10) as char
    }
}

fn publish_json<T: Serialize>(value: &T) {
    match serde_json::to_vec_pretty(value) {
        Ok(data) => publish("published_output", &data),
        Err(e) => {
            let fallback = format!(
                r#"{{"ok":false,"source":"google","error":"json serialization failed: {e}"}}"#
            );
            publish("published_output", fallback.as_bytes());
        }
    }
}

fn publish(kind: &str, data: &[u8]) {
    unsafe {
        artifact_publish(
            kind.as_ptr() as i32,
            kind.len() as i32,
            data.as_ptr() as i32,
            data.len() as i32,
        );
    }
}

fn report_outcome(status: &str, reason_code: &str, message: &str) {
    let outcome = format!(
        r#"{{"status":"{status}","reason_code":"{reason_code}","message":{message_json},"primary_artifact_kind":"published_output"}}"#,
        message_json = json_string(message),
    );
    unsafe {
        mission_outcome_report(outcome.as_ptr() as i32, outcome.len() as i32);
    }
}

fn log(msg: &str) {
    unsafe {
        log_info(msg.as_ptr() as i32, msg.len() as i32);
    }
}
