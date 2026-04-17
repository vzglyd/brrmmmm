# brrmmmm terminal demo
#
# Prerequisites (from repo root):
#   cargo build --release
#   npm --prefix tui run build
#   cargo build --manifest-path sidecars/demo-weather/Cargo.toml \
#     --target wasm32-wasip1 --release
#
# Record:
#   vhs demos/terminal.tape
#
# Output: demos/terminal.gif

Output demos/terminal.gif

Set Shell "bash"
Set FontSize 14
Set Width  1800
Set Height 1100
Set Theme "Dracula"
Set Padding 16

Sleep 500ms

# Set WASM path
Type `WASM=sidecars/demo-weather/target/wasm32-wasip1/release/demo_weather_sidecar.wasm`
Enter
Sleep 300ms

# Step 1: validate (default text output)
Type "brrmmmm validate $WASM"
Enter
Sleep 2s

# Step 2: validate as table
Type "brrmmmm --output table validate $WASM"
Enter
Sleep 2s

# Step 3: inspect as table
Type "brrmmmm --output table inspect $WASM"
Enter
Sleep 2s

# Step 4: one-shot fetch (pretty JSON)
Type "brrmmmm --output json run $WASM --once"
Enter
Sleep 5s

# Step 5: TUI
Type "brrmmmm $WASM"
Enter
Sleep 5s
Ctrl+C
Sleep 500ms
