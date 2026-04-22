# brrmmmm CLI demo — validate / inspect / run --once
#
# Prerequisites (from repo root):
#   cargo build --release
#   cargo build --manifest-path missions/demo-weather/Cargo.toml \
#     --target wasm32-wasip1 --release
#
# Record:
#   vhs demos/cli.tape

Output demos/cli.gif

Set Shell "bash"
Set FontSize 14
Set Width  1000
Set Height 600
Set Theme "Dracula"
Set Padding 16

Sleep 500ms

Type `export PATH="$(pwd)/target/release:$PATH"`
Enter
Sleep 300ms

Type `WASM=missions/demo-weather/target/wasm32-wasip1/release/demo_weather_mission.wasm`
Enter
Sleep 300ms

# validate (default text)
Type "brrmmmm validate $WASM"
Enter
Sleep 2s

# validate as table
Type "brrmmmm --output table validate $WASM"
Enter
Sleep 2s

# inspect as table
Type "brrmmmm --output table inspect $WASM"
Enter
Sleep 2s

# one-shot fetch (pretty JSON)
Type "brrmmmm --output json run $WASM --once"
Enter
Sleep 5s
