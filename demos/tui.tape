# brrmmmm TUI demo
#
# Prerequisites (from repo root):
#   cargo build --release
#   npm --prefix tui run build
#   cargo build --manifest-path sidecars/demo-weather/Cargo.toml \
#     --target wasm32-wasip1 --release
#
# Record:
#   vhs demos/tui.tape

Output demos/tui.gif

Set Shell "bash"
Set FontSize 14
Set Width  1800
Set Height 1100
Set Theme "Dracula"
Set Padding 16

Sleep 500ms

Type `export PATH="$(pwd)/target/release:$PATH"`
Enter
Sleep 300ms

Type `WASM=sidecars/demo-weather/target/wasm32-wasip1/release/demo_weather_sidecar.wasm`
Enter
Sleep 300ms

Type "brrmmmm $WASM"
Enter
Sleep 6s
Ctrl+C
Sleep 500ms
