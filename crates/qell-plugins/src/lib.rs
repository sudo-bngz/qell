//! WebAssembly plugin host for Qell.
//!
//! Plugins are plain core-WebAssembly modules (no component model, no WASI)
//! so they can be written in any language that targets `wasm32` with a few
//! `extern "C"` functions. They run sandboxed in Wasmtime and talk to the
//! editor only through the small host API below.
//!
//! # ABI (version 0)
//!
//! A plugin **exports**:
//!
//! - `memory` — its linear memory.
//! - `qell_alloc(len: i32) -> i32` — returns a pointer to `len` writable
//!   bytes; used by the host to pass strings in.
//! - `qell_init()` — called once after loading; the plugin registers its
//!   commands here.
//! - `qell_command(name_ptr, name_len, arg_ptr, arg_len) -> i32` — run a
//!   registered command; return 0 on success.
//!
//! A plugin may **import** from module `"qell"`:
//!
//! - `register_command(name_ptr, name_len)` — add a command to the palette.
//! - `log(ptr, len)` — write to the editor log.
//! - `notify(ptr, len)` — show a message in the status bar.
//! - `insert_text(ptr, len)` — insert text at the cursor of the active buffer.
//! - `selection(ptr, cap) -> i32` — copy the current selection (or empty)
//!   into the given buffer, returning the full length (call `qell_alloc`
//!   with that length and call again if it was larger than `cap`).
//!
//! See `plugins/hello` in the repository for a complete Rust example.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context as _, Result};
use wasmtime::{Caller, Engine, Instance, Linker, Memory, Module, Store, TypedFunc};

/// Something a plugin asked the editor to do. Drained by the app after each
/// call into a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Notify(String),
    InsertText(String),
}

/// Per-instance state visible to host functions.
#[derive(Default)]
struct State {
    plugin_name: String,
    commands: Vec<String>,
    actions: Vec<Action>,
    /// Snapshot of editor state the plugin may read during a call.
    selection: String,
}

struct LoadedPlugin {
    name: String,
    path: PathBuf,
    store: Store<State>,
    _instance: Instance,
    memory: Memory,
    alloc: TypedFunc<i32, i32>,
    command: TypedFunc<(i32, i32, i32, i32), i32>,
}

/// Information about one loaded plugin.
#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    pub path: PathBuf,
    pub commands: Vec<String>,
}

/// Editor state handed to a plugin for the duration of one command.
#[derive(Debug, Default, Clone)]
pub struct EditorContext {
    pub selection: String,
}

pub struct Host {
    engine: Engine,
    linker: Linker<State>,
    plugins: Vec<LoadedPlugin>,
    /// command name → index into `plugins`.
    commands: BTreeMap<String, usize>,
}

impl Host {
    pub fn new() -> Result<Self> {
        let mut config = wasmtime::Config::new();
        // On macOS Wasmtime catches guest faults with a Mach exception-port
        // thread that aborts the process if any signal interrupts its wait.
        // The terminal installs a SIGCHLD handler and every finished child
        // process (git, language servers) sends one, so sooner or later one
        // lands on that thread. Plain signal handlers have no such thread.
        config.macos_use_mach_ports(false);
        let engine = Engine::new(&config).map_err(|e| anyhow!("{e}"))?;
        let mut linker = Linker::new(&engine);
        define_host_api(&mut linker)?;
        Ok(Self { engine, linker, plugins: Vec::new(), commands: BTreeMap::new() })
    }

    /// Whether `dir` holds any plugin. The editor only starts the plugin
    /// runtime when it does.
    pub fn has_plugins(dir: &Path) -> bool {
        std::fs::read_dir(dir).map(|rd| rd.flatten().any(|e| e.path().extension().is_some_and(|x| x == "wasm"))).unwrap_or(false)
    }

    /// Load every `*.wasm` in `dir`. Missing dir is fine; a broken plugin is
    /// logged and skipped so one bad file never blocks the editor.
    pub fn load_dir(&mut self, dir: &Path) -> Vec<PluginInfo> {
        let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
        let mut loaded = Vec::new();
        let mut paths: Vec<PathBuf> =
            entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "wasm")).collect();
        paths.sort();
        for p in paths {
            match self.load(&p) {
                Ok(info) => loaded.push(info),
                Err(e) => log::error!("plugin {}: {e:#}", p.display()),
            }
        }
        loaded
    }

    pub fn load(&mut self, path: &Path) -> Result<PluginInfo> {
        let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "plugin".into());
        let module = wt(Module::from_file(&self.engine, path)).context("compiling module")?;
        let mut store = Store::new(&self.engine, State { plugin_name: name.clone(), ..Default::default() });
        let instance = wt(self.linker.instantiate(&mut store, &module)).context("instantiating")?;

        let memory = instance.get_memory(&mut store, "memory").ok_or_else(|| anyhow!("plugin exports no `memory`"))?;
        let alloc = wt(instance.get_typed_func::<i32, i32>(&mut store, "qell_alloc")).context("missing export qell_alloc")?;
        let command = wt(instance.get_typed_func::<(i32, i32, i32, i32), i32>(&mut store, "qell_command"))
            .context("missing export qell_command")?;
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "qell_init") {
            wt(init.call(&mut store, ())).context("qell_init trapped")?;
        }

        let commands = store.data().commands.clone();
        let idx = self.plugins.len();
        for c in &commands {
            if let Some(prev) = self.commands.insert(c.clone(), idx) {
                log::warn!("command `{c}` from {name} shadows the one from {}", self.plugins[prev].name);
            }
        }
        self.plugins.push(LoadedPlugin {
            name: name.clone(),
            path: path.to_path_buf(),
            store,
            _instance: instance,
            memory,
            alloc,
            command,
        });
        Ok(PluginInfo { name, path: path.to_path_buf(), commands })
    }

    pub fn plugins(&self) -> Vec<PluginInfo> {
        self.plugins
            .iter()
            .map(|p| PluginInfo { name: p.name.clone(), path: p.path.clone(), commands: p.store.data().commands.clone() })
            .collect()
    }

    /// All commands offered by plugins, sorted.
    pub fn commands(&self) -> Vec<String> {
        self.commands.keys().cloned().collect()
    }

    pub fn has_command(&self, name: &str) -> bool {
        self.commands.contains_key(name)
    }

    /// Run a command. Returns the actions the plugin requested.
    pub fn run(&mut self, name: &str, arg: &str, ctx: &EditorContext) -> Result<Vec<Action>> {
        let idx = *self.commands.get(name).ok_or_else(|| anyhow!("unknown plugin command `{name}`"))?;
        let plugin = &mut self.plugins[idx];
        plugin.store.data_mut().selection = ctx.selection.clone();
        plugin.store.data_mut().actions.clear();

        let (name_ptr, name_len) = write_guest_string(&mut plugin.store, &plugin.memory, &plugin.alloc, name)?;
        let (arg_ptr, arg_len) = write_guest_string(&mut plugin.store, &plugin.memory, &plugin.alloc, arg)?;
        let rc = wt(plugin.command.call(&mut plugin.store, (name_ptr, name_len, arg_ptr, arg_len)))
            .with_context(|| format!("plugin {} trapped running `{name}`", plugin.name))?;
        if rc != 0 {
            return Err(anyhow!("plugin {} returned error code {rc} for `{name}`", plugin.name));
        }
        Ok(std::mem::take(&mut plugin.store.data_mut().actions))
    }
}

/// wasmtime has its own error type; lift it into anyhow so `.context()` works.
fn wt<T>(r: wasmtime::Result<T>) -> Result<T> {
    r.map_err(anyhow::Error::from)
}

fn write_guest_string(store: &mut Store<State>, memory: &Memory, alloc: &TypedFunc<i32, i32>, s: &str) -> Result<(i32, i32)> {
    let len = s.len() as i32;
    let ptr = wt(alloc.call(&mut *store, len)).context("qell_alloc trapped")?;
    memory.write(&mut *store, ptr as usize, s.as_bytes()).context("writing into plugin memory")?;
    Ok((ptr, len))
}

fn read_guest_string(caller: &mut Caller<'_, State>, ptr: i32, len: i32) -> String {
    let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) else { return String::new() };
    let mut buf = vec![0u8; len.max(0) as usize];
    if mem.read(&*caller, ptr as usize, &mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn define_host_api(linker: &mut Linker<State>) -> Result<()> {
    linker.func_wrap("qell", "register_command", |mut caller: Caller<'_, State>, ptr: i32, len: i32| {
        let name = read_guest_string(&mut caller, ptr, len);
        if !name.is_empty() {
            caller.data_mut().commands.push(name);
        }
    })?;
    linker.func_wrap("qell", "log", |mut caller: Caller<'_, State>, ptr: i32, len: i32| {
        let msg = read_guest_string(&mut caller, ptr, len);
        log::info!("[plugin {}] {msg}", caller.data().plugin_name);
    })?;
    linker.func_wrap("qell", "notify", |mut caller: Caller<'_, State>, ptr: i32, len: i32| {
        let msg = read_guest_string(&mut caller, ptr, len);
        caller.data_mut().actions.push(Action::Notify(msg));
    })?;
    linker.func_wrap("qell", "insert_text", |mut caller: Caller<'_, State>, ptr: i32, len: i32| {
        let text = read_guest_string(&mut caller, ptr, len);
        caller.data_mut().actions.push(Action::InsertText(text));
    })?;
    linker.func_wrap("qell", "selection", |mut caller: Caller<'_, State>, ptr: i32, cap: i32| -> i32 {
        let sel = caller.data().selection.clone();
        if let Some(mem) = caller.get_export("memory").and_then(|e| e.into_memory()) {
            let n = sel.len().min(cap.max(0) as usize);
            let _ = mem.write(&mut caller, ptr as usize, &sel.as_bytes()[..n]);
        }
        sel.len() as i32
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny plugin written directly in WAT so the test needs no wasm
    /// toolchain. It registers `hello`, and when run notifies "hi <arg>"
    /// and inserts the current selection back into the buffer.
    const WAT: &str = r#"
    (module
      (import "qell" "register_command" (func $register (param i32 i32)))
      (import "qell" "notify" (func $notify (param i32 i32)))
      (import "qell" "insert_text" (func $insert (param i32 i32)))
      (import "qell" "selection" (func $selection (param i32 i32) (result i32)))
      (memory (export "memory") 1)
      (global $heap (mut i32) (i32.const 1024))
      (data (i32.const 0) "hello")
      (data (i32.const 8) "hi ")
      (func (export "qell_alloc") (param $len i32) (result i32)
        (local $p i32)
        (local.set $p (global.get $heap))
        (global.set $heap (i32.add (global.get $heap) (local.get $len)))
        (local.get $p))
      (func (export "qell_init")
        (call $register (i32.const 0) (i32.const 5)))
      (func (export "qell_command") (param $np i32) (param $nl i32) (param $ap i32) (param $al i32) (result i32)
        (local $n i32)
        ;; notify "hi " + arg: copy arg right after "hi " at 11
        (memory.copy (i32.const 11) (local.get $ap) (local.get $al))
        (call $notify (i32.const 8) (i32.add (i32.const 3) (local.get $al)))
        ;; insert the selection
        (local.set $n (call $selection (i32.const 512) (i32.const 256)))
        (call $insert (i32.const 512) (local.get $n))
        (i32.const 0))
    )"#;

    #[test]
    fn loads_and_runs_a_wat_plugin() {
        let dir = std::env::temp_dir().join(format!("qell-plugin-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("demo.wasm");
        std::fs::write(&path, wat::parse_str(WAT).unwrap()).unwrap();

        let mut host = Host::new().unwrap();
        let loaded = host.load_dir(&dir);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].commands, vec!["hello".to_string()]);

        let actions = host.run("hello", "world", &EditorContext { selection: "SEL".into() }).unwrap();
        assert_eq!(actions, vec![Action::Notify("hi world".into()), Action::InsertText("SEL".into())]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
