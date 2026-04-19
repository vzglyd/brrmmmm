use serde::Deserialize;

const DESCRIBE: &[u8] = include_bytes!("describe.json");

// Default coordinates: Berlin
const DEFAULT_LAT: &str = "52.52";
const DEFAULT_LON: &str = "13.41";
const DEFAULT_LOCATION: &str = "Berlin";

#[link(wasm_import_module = "vzglyd_host")]
unsafe extern "C" {
    fn network_request(ptr: i32, len: i32) -> i32;
    fn network_response_len() -> i32;
    fn network_response_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn log_info(ptr: i32, len: i32) -> i32;
}

#[unsafe(no_mangle)]
pub extern "C" fn vzglyd_sidecar_abi_version() -> u32 {
    1
}

#[unsafe(no_mangle)]
pub extern "C" fn vzglyd_sidecar_describe_ptr() -> i32 {
    DESCRIBE.as_ptr() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn vzglyd_sidecar_describe_len() -> i32 {
    DESCRIBE.len() as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn vzglyd_sidecar_start() {
    let lat = env_var("LATITUDE").unwrap_or_else(|| DEFAULT_LAT.to_string());
    let lon = env_var("LONGITUDE").unwrap_or_else(|| DEFAULT_LON.to_string());
    let location = env_var("LOCATION_NAME").unwrap_or_else(|| DEFAULT_LOCATION.to_string());

    let path = format!(
        "/v1/forecast?latitude={lat}&longitude={lon}&current_weather=true&wind_speed_unit=ms"
    );

    let request = serde_json::json!({
        "wire_version": 1,
        "kind": "https_get",
        "host": "api.open-meteo.com",
        "path": path,
        "headers": []
    });
    let request_bytes = request.to_string();

    log(format!("fetching weather for {location} ({lat},{lon})").as_str());

    let rc = unsafe { network_request(request_bytes.as_ptr() as i32, request_bytes.len() as i32) };
    if rc != 0 {
        log(format!("network_request failed: {rc}").as_str());
        publish_error(&location, &format!("network_request rc={rc}"));
        return;
    }

    let resp_len = unsafe { network_response_len() };
    if resp_len <= 0 {
        log("empty response from network_request");
        publish_error(&location, "empty response");
        return;
    }

    let mut buf = vec![0u8; resp_len as usize];
    let read_rc = unsafe { network_response_read(buf.as_mut_ptr() as i32, resp_len) };
    if read_rc != resp_len {
        log(format!("network_response_read returned {read_rc}, expected {resp_len}").as_str());
        publish_error(&location, &format!("read mismatch: got={read_rc} want={resp_len}"));
        return;
    }

    // Parse the host-wire response envelope
    let envelope: VersionedResponse = match serde_json::from_slice(&buf) {
        Ok(v) => v,
        Err(e) => {
            log(format!("failed to parse wire envelope: {e}").as_str());
            publish_error(&location, &format!("envelope parse error: {e}"));
            return;
        }
    };

    let body = match envelope.payload {
        WirePayload::Http { status_code, body, .. } => {
            if status_code != 200 {
                log(format!("HTTP {status_code}").as_str());
                publish_error(&location, &format!("HTTP {status_code}"));
                return;
            }
            body
        }
        WirePayload::Error { message, .. } => {
            log(format!("host error: {message}").as_str());
            publish_error(&location, &format!("host error: {message}"));
            return;
        }
        WirePayload::TcpConnect { .. } => {
            publish_error(&location, "unexpected tcp_connect response");
            return;
        }
    };

    // Publish raw response body as-is
    publish("raw_source_payload", &body);

    // Parse Open-Meteo response
    let weather: OpenMeteoResponse = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(e) => {
            log(format!("failed to parse Open-Meteo response: {e}").as_str());
            publish_error(&location, &format!("parse error: {e}"));
            return;
        }
    };

    let cw = &weather.current_weather;
    let condition = weathercode_to_condition(cw.weathercode);

    // Normalized payload: typed fields, no location metadata
    let normalized = serde_json::json!({
        "temperature_c": cw.temperature,
        "wind_speed_ms": cw.windspeed,
        "weathercode": cw.weathercode,
        "condition": condition,
        "is_day": cw.is_day == 1
    });
    publish("normalized_payload", normalized.to_string().as_bytes());

    // Published output: what downstream consumers should use
    let published = serde_json::json!({
        "ok": true,
        "location": location,
        "temperature_c": cw.temperature,
        "condition": condition,
        "wind_speed_ms": cw.windspeed,
        "is_day": cw.is_day == 1
    });
    publish("published_output", published.to_string().as_bytes());

    log(format!("done: {location} {temp}°C {condition}", temp = cw.temperature).as_str());
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

fn publish_error(location: &str, message: &str) {
    let payload = serde_json::json!({
        "ok": false,
        "location": location,
        "error": message
    });
    let bytes = payload.to_string();
    publish("published_output", bytes.as_bytes());
}

fn log(msg: &str) {
    unsafe { log_info(msg.as_ptr() as i32, msg.len() as i32) };
}

fn env_var(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

fn weathercode_to_condition(code: u32) -> &'static str {
    match code {
        0 => "clear sky",
        1 => "mainly clear",
        2 => "partly cloudy",
        3 => "overcast",
        45 | 48 => "fog",
        51 | 53 | 55 => "drizzle",
        61 | 63 | 65 => "rain",
        71 | 73 | 75 => "snow",
        80 | 81 | 82 => "rain showers",
        95 => "thunderstorm",
        96 | 99 => "thunderstorm with hail",
        _ => "unknown",
    }
}

// --- wire types ---

// Mirrors the host's HostResponse wire format:
// {"wire_version":1,"kind":"http","status_code":200,"headers":[...],"body":[...]}
// {"wire_version":1,"kind":"error","error_kind":"dns","message":"..."}
#[derive(Deserialize)]
struct VersionedResponse {
    #[allow(dead_code)]
    wire_version: u8,
    #[serde(flatten)]
    payload: WirePayload,
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum WirePayload {
    Http {
        status_code: u16,
        #[serde(default)]
        #[allow(dead_code)]
        headers: Vec<WireHeader>,
        body: Vec<u8>,
    },
    TcpConnect {
        #[allow(dead_code)]
        elapsed_ms: u64,
    },
    Error {
        #[allow(dead_code)]
        error_kind: String,
        message: String,
    },
}

#[derive(Deserialize)]
#[allow(dead_code)]
struct WireHeader {
    name: String,
    value: String,
}

#[derive(Deserialize)]
struct OpenMeteoResponse {
    current_weather: CurrentWeather,
}

#[derive(Deserialize)]
struct CurrentWeather {
    temperature: f64,
    windspeed: f64,
    weathercode: u32,
    is_day: u8,
}
