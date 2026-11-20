# Contributing to Qell

Thanks for taking a look. Qell is small on purpose; contributions that keep
it small are the most welcome.

## Building

```sh
cargo build --workspace
cargo test --workspace
cargo run -p qell -- .
```

On Linux you need `libxkbcommon-dev`, `libwayland-dev`, `libgl1-mesa-dev`
and `libgtk-3-dev` (for native file dialogs).

## Layout

- `crates/qell-core` — everything about text. No egui, no processes.
- `crates/qell-lsp` — talking to language servers.
- `crates/qell-term` — the terminal emulator, exposed as cell snapshots.
- `crates/qell-plugins` — the Wasmtime host and the plugin ABI.
- `crates/qell-app` — the window: panels, keymap, palette, rendering.

Keep that dependency direction: `app` depends on everything, the others
depend only on `core`.

## Style

`cargo fmt` and `cargo clippy --workspace --all-targets -- -D warnings`
must pass; CI enforces both. Prefer plain functions over trait machinery,
and comments that say *why* over comments that say *what*.

## Adding a language

See the "Adding a language" section in the README; it is a single entry in
`crates/qell-core/src/syntax.rs` plus a grammar crate.

## Writing a plugin

See `plugins/hello` and the ABI documentation at the top of
`crates/qell-plugins/src/lib.rs`.
