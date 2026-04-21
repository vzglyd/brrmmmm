// CAPTCHA solver demo: navigates to an inline data: URL containing a styled challenge,
// takes a screenshot via browser_*, sends it to Claude vision via ai_*, then publishes
// the AI-interpreted answer. No external network needed for the browser step.
// Requires ANTHROPIC_API_KEY set on the brrmmmm host process.

const DESCRIBE: &[u8] = include_bytes!("describe.json");

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
    // Inline CAPTCHA page: styled letters in a visible challenge box.
    // CSS transforms make it visually challenging while remaining legible to AI vision.
    let challenge_text = "BR7MX";
    let page_url = data_url(&format!(
        concat!(
            "<!DOCTYPE html><html><head>",
            "<style>",
            "body{{margin:0;background:#1a1a2e;display:grid;place-items:center;min-height:100vh;font-family:monospace;}}",
            ".box{{background:#16213e;border:2px solid #0f3460;border-radius:12px;padding:40px;text-align:center;}}",
            "h2{{color:#e94560;margin:0 0 24px;font-size:18px;letter-spacing:2px;}}",
            "#challenge{{",
            "  display:inline-flex;gap:6px;padding:16px 24px;",
            "  background:#0f3460;border-radius:8px;",
            "}}",
            ".ch{{",
            "  font-size:42px;font-weight:bold;color:#e94560;",
            "  text-shadow:2px 2px 4px rgba(0,0,0,0.5);",
            "  display:inline-block;",
            "}}",
            ".ch:nth-child(1){{transform:rotate(-8deg) skewX(5deg);color:#ff6b9d;}}",
            ".ch:nth-child(2){{transform:rotate(5deg) skewX(-3deg);color:#c44dff;}}",
            ".ch:nth-child(3){{transform:rotate(-3deg) skewX(8deg);color:#4dffb4;}}",
            ".ch:nth-child(4){{transform:rotate(7deg) skewX(-5deg);color:#ffd700;}}",
            ".ch:nth-child(5){{transform:rotate(-6deg) skewX(4deg);color:#ff6b6b;}}",
            "</style>",
            "</head><body>",
            "<div class='box'>",
            "<h2>CAPTCHA VERIFICATION</h2>",
            "<div id='challenge'>{}</div>",
            "</div>",
            "</body></html>"
        ),
        challenge_text
            .chars()
            .map(|c| format!("<span class='ch'>{c}</span>"))
            .collect::<Vec<_>>()
            .join("")
    ));

    log("navigating to CAPTCHA page");
    let nav = browser_do(&format!(
        r#"{{"wire_version":1,"action":"navigate","url":"{page_url}"}}"#
    ));
    if !nav.ok {
        publish_failure(&format!("navigate failed: {}", nav.error));
        return;
    }

    log("waiting for challenge element");
    let wait = browser_do(
        r##"{"wire_version":1,"action":"wait_for_selector","selector":"#challenge","timeout_ms":5000}"##,
    );
    if !wait.ok {
        publish_failure(&format!("challenge element not found: {}", wait.error));
        return;
    }

    // Brief pause so the page renders fully before screenshotting.
    unsafe { sleep_ms(800) };

    log("taking screenshot");
    let shot = browser_do(r#"{"wire_version":1,"action":"screenshot"}"#);
    if !shot.ok {
        publish_failure(&format!("screenshot failed: {}", shot.error));
        return;
    }
    let png_b64 = shot.value;
    if png_b64.is_empty() {
        publish_failure("screenshot returned empty png_b64");
        return;
    }

    log(&format!("screenshot captured ({} base64 chars); sending to AI", png_b64.len()));

    let ai_prompt = "This screenshot shows a CAPTCHA challenge. \
        The challenge consists of styled, rotated, colourful letters displayed in a dark box. \
        What are the exact characters shown in the challenge, in order from left to right? \
        Reply with ONLY the characters - no spaces, no explanation.";

    // Build ai_request JSON. The png_b64 field is large; build it in parts.
    let ai_json = format!(
        r#"{{"wire_version":1,"action":"vision","prompt":{prompt_json},"image_png_b64":{png_json}}}"#,
        prompt_json = json_string(ai_prompt),
        png_json = json_string(&png_b64),
    );

    let ai_result = ai_do(&ai_json);
    if !ai_result.ok {
        publish_failure(&format!("AI request failed: {}", ai_result.error));
        return;
    }

    let answer = ai_result.value.trim().to_string();
    log(&format!("AI answered: {answer}"));

    let out = format!(
        r#"{{"ok":true,"captcha_text":{answer_json},"challenge":{challenge_json}}}"#,
        answer_json = json_string(&answer),
        challenge_json = json_string(challenge_text),
    );
    publish("published_output", out.as_bytes());
    report_outcome("published", "published_output", "captcha fixture published output");
}

// -- Browser action helpers --------------------------------------------

struct ActionResult {
    ok: bool,
    error: String,
    value: String,
}

fn browser_do(json: &str) -> ActionResult {
    let response = host_call_json("browser", json);
    let Ok(value) = response else {
        return ActionResult {
            ok: false,
            error: response.unwrap_err(),
            value: String::new(),
        };
    };
    ActionResult {
        ok: true,
        error: String::new(),
        value: value
            .get("png_b64")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

// -- AI action helpers --------------------------------------------------

fn ai_do(json: &str) -> ActionResult {
    let response = host_call_json("ai", json);
    let Ok(value) = response else {
        return ActionResult {
            ok: false,
            error: response.unwrap_err(),
            value: String::new(),
        };
    };
    ActionResult {
        ok: true,
        error: String::new(),
        value: value
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
    }
}

fn host_call_json(capability: &str, json: &str) -> Result<serde_json::Value, String> {
    let mut request: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
    let object = request
        .as_object_mut()
        .ok_or_else(|| "host call request must be an object".to_string())?;
    object.insert("wire_version".to_string(), serde_json::json!(2));
    object.insert("capability".to_string(), serde_json::json!(capability));
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
            .unwrap_or_else(|| serde_json::json!({})))
    } else {
        Err(response["error"]["message"]
            .as_str()
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| String::from_utf8_lossy(&buf).into_owned()))
    }
}

// -- Publish helpers ----------------------------------------------------

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
    let json = format!(r#"{{"ok":false,"error":{}}}"#, json_string(msg));
    publish("published_output", json.as_bytes());
    report_outcome("terminal_failure", "captcha_solver_failed", msg);
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
    unsafe { log_info(msg.as_ptr() as i32, msg.len() as i32) };
}

// -- JSON helpers -------------------------------------------------------

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

// -- data: URL builder --------------------------------------------------

fn data_url(html: &str) -> String {
    let mut out = String::from("data:text/html,");
    for byte in html.bytes() {
        match byte {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'~'
            | b'!'
            | b'$'
            | b'&'
            | b'\''
            | b'('
            | b')'
            | b'*'
            | b'+'
            | b','
            | b';'
            | b'='
            | b':'
            | b'@'
            | b'/'
            | b'?' => out.push(byte as char),
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
    if n < 10 { (b'0' + n) as char } else { (b'A' + n - 10) as char }
}
