# hello — example Qell plugin

A minimal plugin that registers two commands:

- `hello` — shows a notification.
- `shout` — replaces the selection with its upper-cased version (inserts at
  the cursor when nothing is selected).

## Build

```sh
rustup target add wasm32-unknown-unknown
cargo build --release --target wasm32-unknown-unknown
cp target/wasm32-unknown-unknown/release/qell_plugin_hello.wasm \
   ~/.config/qell/plugins/hello.wasm
```

Restart Qell, open the command palette (`Ctrl+Shift+P`) and type `hello`.

See `crates/qell-plugins/src/lib.rs` for the full host API.
