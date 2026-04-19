// Browser login fixture: drives an inline data: URL login form through the full
// navigate → fill → click → wait_for_selector sequence, then publishes the result.
// Requires Chrome/Chromium installed on the host. No external network needed.

const DESCRIBE: &[u8] = include_bytes!("describe.json");
const STEP_DELAY_MS: i64 = 1_000;
const FINAL_DELAY_MS: i64 = 2_500;

#[link(wasm_import_module = "vzglyd_host")]
unsafe extern "C" {
    fn browser_action(ptr: i32, len: i32) -> i32;
    fn browser_response_len() -> i32;
    fn browser_response_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn log_info(ptr: i32, len: i32) -> i32;
    fn sleep_ms(duration_ms: i64) -> i32;
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_abi_version() -> u32 {
    1
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_describe_ptr() -> i32 {
    DESCRIBE.as_ptr() as i32
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_describe_len() -> i32 {
    DESCRIBE.len() as i32
}

#[no_mangle]
pub extern "C" fn vzglyd_sidecar_start() {
    // Inline login form as a data: URL. On submit, reveals #ok.
    // No external server, no network required — everything runs in the browser process.
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

    log("navigating to login form");
    let nav = action(&format!(
        r#"{{"wire_version":1,"action":"navigate","url":"{page_url}"}}"#
    ));
    if !nav.ok {
        publish_failure(&format!("navigate failed: {}", nav.error));
        return;
    }
    pause("page loaded; pausing before selector wait", STEP_DELAY_MS);

    log("waiting for form to be ready");
    let wait_form = action(
        r##"{"wire_version":1,"action":"wait_for_selector","selector":"#username","timeout_ms":5000}"##,
    );
    if !wait_form.ok {
        publish_failure(&format!("form did not appear: {}", wait_form.error));
        return;
    }
    pause("form is ready; pausing before username fill", STEP_DELAY_MS);

    log("filling username");
    let fill_u =
        action(r##"{"wire_version":1,"action":"fill","selector":"#username","value":"testuser"}"##);
    if !fill_u.ok {
        publish_failure(&format!("fill username failed: {}", fill_u.error));
        return;
    }
    pause(
        "username filled; pausing before password fill",
        STEP_DELAY_MS,
    );

    log("filling password");
    let fill_p =
        action(r##"{"wire_version":1,"action":"fill","selector":"#password","value":"hunter2"}"##);
    if !fill_p.ok {
        publish_failure(&format!("fill password failed: {}", fill_p.error));
        return;
    }
    pause("password filled; pausing before submit", STEP_DELAY_MS);

    log("clicking submit");
    let click = action(r##"{"wire_version":1,"action":"click","selector":"#submit"}"##);
    if !click.ok {
        publish_failure(&format!("click failed: {}", click.error));
        return;
    }
    pause(
        "submitted; pausing before success selector wait",
        STEP_DELAY_MS,
    );

    log("waiting for #ok");
    let wait = action(
        r##"{"wire_version":1,"action":"wait_for_selector","selector":"#ok","timeout_ms":5000}"##,
    );
    if !wait.ok {
        publish_failure(&format!("wait_for_selector failed: {}", wait.error));
        return;
    }
    pause(
        "success marker is visible; pausing before publishing",
        FINAL_DELAY_MS,
    );

    log("getting current url");
    let url_resp = action(r#"{"wire_version":1,"action":"current_url"}"#);
    let url = if url_resp.ok {
        url_resp.value
    } else {
        String::new()
    };

    log("done — publishing result");
    let out = format!(r#"{{"ok":true,"logged_in":true,"url":"{url}"}}"#);
    publish("published_output", out.as_bytes());
}

// ── Browser action helpers ────────────────────────────────────────────

struct ActionResult {
    ok: bool,
    error: String,
    value: String,
}

fn action(json: &str) -> ActionResult {
    let rc = unsafe { browser_action(json.as_ptr() as i32, json.len() as i32) };
    if rc != 0 {
        return ActionResult {
            ok: false,
            error: format!("browser_action rc={rc}"),
            value: String::new(),
        };
    }

    let len = unsafe { browser_response_len() };
    if len <= 0 {
        return ActionResult {
            ok: false,
            error: "empty response".to_string(),
            value: String::new(),
        };
    }

    let mut buf = vec![0u8; len as usize];
    let read = unsafe { browser_response_read(buf.as_mut_ptr() as i32, len) };
    if read != len {
        return ActionResult {
            ok: false,
            error: format!("read mismatch: got={read} want={len}"),
            value: String::new(),
        };
    }

    let text = String::from_utf8_lossy(&buf).into_owned();

    // Parse "ok" field from the JSON response.
    let ok = text.contains(r#""ok":true"#);
    // Extract "url" field if present (simple scan, no dep on a JSON parser).
    let url_value = extract_string_field(&text, "url").unwrap_or_default();
    let error = if ok {
        String::new()
    } else {
        extract_string_field(&text, "message").unwrap_or_else(|| text.clone())
    };

    ActionResult {
        ok,
        error,
        value: url_value,
    }
}

fn extract_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!(r#""{key}":""#);
    let start = json.find(&needle)? + needle.len();
    let rest = &json[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
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
