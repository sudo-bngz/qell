# Plugins

Plugins are WebAssembly modules: put a `.wasm` file in Qell's `plugins`
folder — `~/Library/Application Support/qell/plugins/` on macOS,
`~/.config/qell/plugins/` on Linux — and restart Qell. They run sandboxed (Wasmtime)
and can only use the small API below — no files, no network.

## Build the example

The repository has a complete example in
[`plugins/hello`](https://github.com/sudo-bngz/qell/tree/main/plugins/hello),
which adds two commands: `hello` (a notification) and `shout` (upper-cases
the selection).

```sh
rustup target add wasm32-unknown-unknown
cd plugins/hello
cargo build --release --target wasm32-unknown-unknown
dir="$HOME/.config/qell/plugins"   # macOS: "$HOME/Library/Application Support/qell/plugins"
mkdir -p "$dir"
cp target/wasm32-unknown-unknown/release/qell_plugin_hello.wasm "$dir/hello.wasm"
```

Restart Qell, open the palette and type `hello`.

## The API (version 0)

Plugins are plain core-WebAssembly modules — no component model, no WASI
— so any language that targets `wasm32` with a few `extern "C"` functions
can write one.

A plugin **exports**:

| | |
|---|---|
| `memory` | its linear memory |
| `qell_alloc(len) -> ptr` | `len` writable bytes, for the host to pass strings in |
| `qell_init()` | called once after loading; register commands here |
| `qell_command(name_ptr, name_len, arg_ptr, arg_len) -> i32` | run a command; 0 on success |

It may **import**, from module `"qell"`:

| | |
|---|---|
| `register_command(ptr, len)` | add a command to the palette |
| `log(ptr, len)` | write to Qell's log |
| `notify(ptr, len)` | show a message in the status bar |
| `insert_text(ptr, len)` | insert text at the cursor |
| `selection(ptr, cap) -> len` | copy the selection into a buffer (call again with a bigger one if `len > cap`) |
