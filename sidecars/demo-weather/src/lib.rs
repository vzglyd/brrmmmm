use base64::Engine as _;
use serde::Deserialize;

const DESCRIBE: &[u8] = include_bytes!("describe.json");

// Default coordinates: Berlin
const DEFAULT_LAT: &str = "52.52";
const DEFAULT_LON: &str = "13.41";
const DEFAULT_LOCATION: &str = "Berlin";

#[link(wasm_import_module = "vzglyd_host")]
unsafe extern "C" {
    fn host_call(ptr: i32, len: i32) -> i32;
    fn host_response_len() -> i32;
    fn host_response_read(ptr: i32, len: i32) -> i32;
    fn artifact_publish(kind_ptr: i32, kind_len: i32, data_ptr: i32, data_len: i32) -> i32;
    fn log_info(ptr: i32, len: i32) -> i32;
    fn params_len() -> i32;
    fn params_read(ptr: i32, len: i32) -> i32;
}

#[derive(Deserialize)]
struct WeatherParams {
    location_name: Option<String>,
}

#[unsafe(no_mangle)]
pub extern "C" fn vzglyd_sidecar_abi_version() -> u32 {
    2
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
    let location = params_location_name()
        .or_else(|| env_var("LOCATION_NAME"))
        .unwrap_or_else(|| DEFAULT_LOCATION.to_string());

    let url = format!(
        "https://api.open-meteo.com/v1/forecast?latitude={lat}&longitude={lon}&current_weather=true&wind_speed_unit=ms"
    );

    log(format!("fetching weather for {location} ({lat},{lon})").as_str());

    let response = match network_call(serde_json::json!({
        "action": "http",
        "method": "GET",
        "url": url,
        "headers": []
    })) {
        Ok(value) => value,
        Err(error) => {
            log(format!("network host call failed: {error}").as_str());
            publish_error(&location, &error);
            return;
        }
    };

    let body = match decode_http_body(&response) {
        Ok(body) => body,
        Err(error) => {
            log(format!("failed to decode network response: {error}").as_str());
            publish_error(&location, &error);
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

fn network_call(request: serde_json::Value) -> Result<serde_json::Value, String> {
    host_call_json("network", request)
}

fn host_call_json(
    capability: &str,
    mut request: serde_json::Value,
) -> Result<serde_json::Value, String> {
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

fn decode_http_body(response: &serde_json::Value) -> Result<Vec<u8>, String> {
    let kind = response
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if kind != "http" {
        return Err(format!("unexpected network response kind: {kind}"));
    }
    let status_code = response
        .get("status_code")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if status_code != 200 {
        return Err(format!("HTTP {status_code}"));
    }
    let body = response
        .get("body_base64")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "network response body_base64 missing".to_string())?;
    base64::engine::general_purpose::STANDARD
        .decode(body)
        .map_err(|error| error.to_string())
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

fn params_location_name() -> Option<String> {
    let len = unsafe { params_len() };
    if len <= 0 {
        return None;
    }

    let mut buf = vec![0u8; len as usize];
    let read = unsafe { params_read(buf.as_mut_ptr() as i32, len) };
    if read != len {
        return None;
    }

    let params: WeatherParams = serde_json::from_slice(&buf).ok()?;
    params
        .location_name
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
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
