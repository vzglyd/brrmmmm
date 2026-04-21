use serde_json::json;

// Browser login fixture: drives an inline data: URL login form through the full
// navigate → fill → click → wait_for_selector sequence, then publishes the result.
// Requires Chrome/Chromium installed on the host. No external network needed.

const DESCRIBE: &[u8] = include_bytes!("describe.json");
const STEP_DELAY_MS: i64 = 1_000;
const FINAL_DELAY_MS: i64 = 2_500;

#[link(wasm_import_module = "brrmmmm_host")]
unsafe extern "C" {
    fn host_call(ptr: i32, len: i32) -> i32;
    fn host_response_len() -> i32;
    fn host_response_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn mission_outcome_report(ptr: i32, len: i32) -> i32;
    fn log_info(ptr: i32, len: i32) -> i32;
    fn sleep_ms(duration_ms: i64) -> i32;
}

#[no_mangle]
pub extern "C" fn brrmmmm_module_abi_version() -> u32 {
    4
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
    let page_url = data_url(concat!(
        "<style>",
        "body{font-family:Arial,sans-serif;background:#f7f7fb;display:grid;place-items:center;min-height:100vh;margin:0;}",
        "main{display:grid;gap:16px;width:min(380px,90vw);}",
        "h1{margin:0;font-size:28px;}",
        "p{margin:0;color:#444;font-size:16px;line-height:1.4;}",
        "form{display:grid;gap:12px;}",
        "input,button{font-size:20px;padding:14px;border-radius:8px;border:1px solid #888;}",
        "button{background:#1f7a4d;color:white;border-color:#1f7a4d;}",
        "#ok{font-size:22px;background:#d8ffe7;border:1px solid #2d8b57;padding:14px;border-radius:8px;}",
        "</style>",
        "<main>",
        "<h1>Browser login fixture</h1>",
        "<p>The sidecar will wait, fill both fields, submit the form, then publish the result.</p>",
        "<form id=f>",
        "<input id=username type=text placeholder=Username>",
        "<input id=password type=password placeholder=Password>",
        "<button id=submit type=submit>Login</button>",
        "</form>",
        "<div id=ok hidden>Logged in</div>",
        "</main>",
        "<script>",
        "document.getElementById('f').onsubmit=function(e){",
        "  e.preventDefault();",
        "  document.getElementById('ok').hidden=false;",
        "};",
        "</script>",
    ));

    if let Err(error) = run(&page_url) {
        publish_failure(&error);
    }
}

fn run(page_url: &str) -> Result<(), String> {
    log("navigating to login form");
    browser_call(json!({"action":"navigate","url":page_url}))?;
    pause("page loaded; pausing before selector wait", STEP_DELAY_MS);

    log("waiting for form to be ready");
    browser_call(json!({"action":"wait_for_selector","selector":"#username","timeout_ms":5000}))?;
    pause("form is ready; pausing before username fill", STEP_DELAY_MS);

    log("filling username");
    browser_call(json!({"action":"fill","selector":"#username","value":"testuser"}))?;
    pause("username filled; pausing before password fill", STEP_DELAY_MS);

    log("filling password");
    browser_call(json!({"action":"fill","selector":"#password","value":"hunter2"}))?;
    pause("password filled; pausing before submit", STEP_DELAY_MS);

    log("clicking submit");
    browser_call(json!({"action":"click","selector":"#submit"}))?;
    pause("submitted; pausing before success selector wait", STEP_DELAY_MS);

    log("waiting for #ok");
    browser_call(json!({"action":"wait_for_selector","selector":"#ok","timeout_ms":5000}))?;
    pause("success marker is visible; pausing before publishing", FINAL_DELAY_MS);

    log("getting current url");
    let url = browser_call(json!({"action":"current_url"}))?
        .get("url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_string();

    log("done — publishing result");
    let out = format!(r#"{{"ok":true,"logged_in":true,"url":"{url}"}}"#);
    publish("published_output", out.as_bytes());
    report_outcome("published", "published_output", "browser login fixture published output");
    Ok(())
}

fn browser_call(request: serde_json::Value) -> Result<serde_json::Value, String> {
    host_call_json("browser", request)
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

    let value: serde_json::Value = serde_json::from_slice(&buf).map_err(|error| error.to_string())?;
    if value.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        Ok(value.get("data").cloned().unwrap_or_else(|| json!({})))
    } else {
        Err(value["error"]["message"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(&buf).into_owned()))
    }
}

// ── Publish helpers ───────────────────────────────────────────────────

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

fn publish_failure(msg: &str) {
    let json = format!(r#"{{"ok":false,"error":{msg:?}}}"#);
    publish("published_output", json.as_bytes());
    report_outcome("terminal_failure", "browser_login_failed", msg);
}

fn report_outcome(status: &str, reason_code: &str, message: &str) {
    let outcome = format!(
        r#"{{"status":"{status}","reason_code":"{reason_code}","message":{message:?},"primary_artifact_kind":"published_output"}}"#
    );
    unsafe {
        mission_outcome_report(outcome.as_ptr() as i32, outcome.len() as i32);
    }
}

fn log(msg: &str) {
    unsafe { log_info(msg.as_ptr() as i32, msg.len() as i32) };
}

fn pause(msg: &str, duration_ms: i64) {
    log(msg);
    unsafe {
        sleep_ms(duration_ms);
    }
}

// ── data: URL builder ─────────────────────────────────────────────────

/// Percent-encodes an HTML string into a `data:text/html,` URL.
/// Encodes chars that are meaningful in URLs: `#` would break the URL,
/// `%` would cause misparse, space and control chars are unsafe.
fn data_url(html: &str) -> String {
    let mut out = String::from("data:text/html,");
    for byte in html.bytes() {
        match byte {
            // Unreserved: pass through
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9'
            | b'-' | b'_' | b'.' | b'~'
            // Safe in data: URL data section
            | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
            | b'*' | b'+' | b',' | b';' | b'=' | b':'
            | b'@' | b'/' | b'?' => out.push(byte as char),
            // Must encode
            b => {
                out.push('%');
                out.push(hex_nibble(b >> 4));
                out.push(hex_nibble(b & 0xf));
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
