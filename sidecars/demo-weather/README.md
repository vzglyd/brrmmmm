# demo-weather-sidecar

Fetches current weather conditions from [Open-Meteo](https://open-meteo.com/) (free, no API key) and publishes a structured payload via the `vzglyd_host` ABI. Defaults to Berlin. Override the location with environment variables.

## Build

Requires `wasm32-wasip1` target:

```bash
rustup target add wasm32-wasip1
```

Then build:

```bash
# With Moon (from repo root)
moon run demo-weather-sidecar:build

# Direct cargo (from this directory)
cargo build --target wasm32-wasip1 --release
# Output: target/wasm32-wasip1/release/demo_weather_sidecar.wasm
```

## Usage

Replace `$WASM` with the path to the built binary.

```bash
WASM=target/wasm32-wasip1/release/demo_weather_sidecar.wasm

# 1. Validate: check imports resolve and ABI version matches
brrmmmm validate $WASM

# 2. Inspect: show sidecar manifest (name, capabilities, poll strategy)
brrmmmm inspect $WASM

# 3. One-shot fetch: print published_output to stdout
brrmmmm run $WASM --once

# 4. Interactive TUI: live-updating terminal dashboard
brrmmmm $WASM   # no subcommand → delegates to TUI automatically
```

### Different location

Pass `LATITUDE` and `LONGITUDE` env vars:

```bash
brrmmmm run $WASM --once \
  --env LATITUDE=-37.81 \
  --env LONGITUDE=144.96
```

You can also set a display name:

```bash
brrmmmm run $WASM --once \
  --env LATITUDE=-37.81 \
  --env LONGITUDE=144.96 \
  --env LOCATION_NAME=Melbourne
```

## Sample output

`brrmmmm inspect` shows the manifest:

```
logical_id:   brrmmmm.demo.weather
name:         Demo Weather Sidecar
abi_version:  2
poll:         fixed_interval 300s
artifacts:    raw_source_payload, normalized_payload, published_output
```

`brrmmmm run $WASM --once` prints the `published_output` artifact:

```json
{"ok":true,"location":"Berlin","temperature_c":14.2,"condition":"partly cloudy","wind_speed_ms":3.1,"is_day":true}
```

## Artifacts

| Kind | Contents |
|---|---|
| `raw_source_payload` | Verbatim Open-Meteo JSON response body |
| `normalized_payload` | Typed fields: `temperature_c`, `wind_speed_ms`, `weathercode`, `condition`, `is_day` |
| `published_output` | Consumer-ready payload with location label and `ok` flag |

## How it works

The sidecar uses `vzglyd_host::network_request` (host-mediated HTTPS) to call the Open-Meteo `/v1/forecast` endpoint. The host runtime (brrmmmm) makes the actual network call and returns the response via `network_response_len` / `network_response_read`. The sidecar parses the JSON inside WASM and publishes three artifact kinds via `artifact_publish`.

No networking code runs inside the WASM sandbox — all TCP/TLS is delegated to the host.
