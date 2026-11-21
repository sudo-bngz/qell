//! The cluster view: k9s in a Qell panel.
//!
//! A table of objects, keyboard first, no mouse needed and no chrome:
//! `j`/`k` to move, `Enter` to go in, `Esc` to come back, `/` to filter,
//! `:` to switch resource. Everything is read through `kubectl` on a
//! worker thread (see `qell-k8s`); the list refreshes every couple of
//! seconds while the panel has the keyboard and stops the moment it
//! doesn't, so nothing talks to a cluster in the background.

use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

use egui::text::LayoutJob;
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, Sense, TextFormat, Vec2};
use qell_k8s::{Client, Error, Health, Listing, Table};

use crate::app::{Focus, QellApp};

/// The context list is Qell's own table, not a kubectl listing.
const CONTEXTS: &str = "contexts";

/// How often the list is re-read while the panel has the keyboard.
const REFRESH: Duration = Duration::from_secs(2);
/// A row's height is the line height; this is its padding.
const ROW_PAD: f32 = 2.0;

/// What the worker sends back.
enum Msg {
    Table(Listing, Box<Table>),
    Failed(Listing, Error),
    Contexts(Vec<String>, Option<String>),
}

/// What the bottom line is collecting, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    No,
    Filter,
    Command,
}

pub struct K8sState {
    pub open: bool,
    /// The panel has the keyboard: only then does it poll.
    pub typing: Typing,
    pub client: Client,
    pub listing: Listing,
    pub table: Table,
    /// Where Esc goes back to.
    stack: Vec<(Listing, usize)>,
    pub error: Option<String>,
    /// There is no point asking again until something changes (no
    /// kubectl to run), so the timer stops.
    blocked: bool,
    loading: bool,
    jobs: Vec<Receiver<Msg>>,
    pub selected: usize,
    pub filter: String,
    /// What is being typed on the bottom line.
    line: String,
    fetched: Option<Instant>,
    pub contexts: Vec<String>,
    pub context: Option<String>,
    pub help: bool,
    /// Rows that fit on screen, for Ctrl+D and page keys.
    page: usize,
}

impl Default for K8sState {
    fn default() -> Self {
        Self {
            open: false,
            typing: Typing::No,
            client: Client::default(),
            listing: Listing::new("pods", None),
            table: Table::default(),
            stack: Vec::new(),
            error: None,
            blocked: false,
            loading: false,
            jobs: Vec::new(),
            selected: 0,
            filter: String::new(),
            line: String::new(),
            fetched: None,
            contexts: Vec::new(),
            context: None,
            help: false,
            page: 20,
        }
    }
}

impl K8sState {
    /// The rows the filter leaves, as indexes into the table.
    fn visible(&self) -> Vec<usize> {
        if self.filter.is_empty() {
            return (0..self.table.rows.len()).collect();
        }
        let needle = self.filter.to_lowercase();
        (0..self.table.rows.len())
            .filter(|i| self.table.rows[*i].cells.iter().any(|c| c.to_lowercase().contains(&needle)))
            .collect()
    }

    /// Ask for this listing; the answer arrives in a later frame.
    fn fetch(&mut self, ctx: &egui::Context) {
        let (client, listing) = (self.client.clone(), self.listing.clone());
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.loading = true;
        self.fetched = Some(Instant::now());
        let started = std::thread::Builder::new().name("kubectl-list".into()).spawn(move || {
            let msg = match client.list(&listing, qell_k8s::now()) {
                Ok(table) => Msg::Table(listing, Box::new(table)),
                Err(e) => Msg::Failed(listing, e),
            };
            let _ = tx.send(msg);
            ctx.request_repaint();
        });
        match started {
            Ok(_) => self.jobs.push(rx),
            Err(e) => {
                self.loading = false;
                self.error = Some(format!("cannot run kubectl: {e}"));
            }
        }
    }

    fn fetch_contexts(&mut self, ctx: &egui::Context) {
        let client = self.client.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        if std::thread::Builder::new()
            .name("kubectl-contexts".into())
            .spawn(move || {
                if let Ok((list, current)) = client.contexts() {
                    let _ = tx.send(Msg::Contexts(list, current));
                    ctx.request_repaint();
                }
            })
            .is_ok()
        {
            self.jobs.push(rx);
        }
    }

    /// Show `listing`, remembering where we were so Esc comes back.
    fn go(&mut self, listing: Listing, ctx: &egui::Context) {
        self.stack.push((self.listing.clone(), self.selected));
        self.set(listing, ctx);
    }

    /// Show `listing` without stacking it (switching resource with `:`).
    fn set(&mut self, listing: Listing, ctx: &egui::Context) {
        self.listing = listing;
        self.table = Table::default();
        self.selected = 0;
        self.filter.clear();
        self.error = None;
        self.blocked = false;
        self.fetch(ctx);
    }

    fn back(&mut self, ctx: &egui::Context) -> bool {
        let Some((listing, selected)) = self.stack.pop() else { return false };
        self.listing = listing;
        self.table = Table::default();
        self.filter.clear();
        self.error = None;
        self.fetch(ctx);
        self.selected = selected;
        true
    }
}

/// Open the panel (or bring it back), and give it the keyboard.
pub fn open(app: &mut QellApp, ctx: &egui::Context) {
    app.k8s.client.bin = app.config.kubectl.clone();
    app.k8s.open = true;
    app.focus = Focus::K8s;
    if app.k8s.contexts.is_empty() {
        app.k8s.fetch_contexts(ctx);
    }
    if app.k8s.table.rows.is_empty() {
        let listing = app.k8s.listing.clone();
        app.k8s.set(listing, ctx);
    }
}

pub fn close(app: &mut QellApp) {
    app.k8s.open = false;
    app.k8s.typing = Typing::No;
    app.k8s.help = false;
    if app.focus == Focus::K8s {
        app.focus = Focus::Editor;
    }
}

/// Take whatever the worker finished, and re-read the list when it is
/// time. Called once a frame while the panel is open.
pub fn update(app: &mut QellApp, ctx: &egui::Context) {
    let mut done = Vec::new();
    for (i, rx) in app.k8s.jobs.iter().enumerate() {
        match rx.try_recv() {
            Ok(msg) => {
                done.push(i);
                match msg {
                    // An answer to a listing we have since left is dropped.
                    Msg::Table(listing, table) if listing == app.k8s.listing => {
                        app.k8s.table = *table;
                        app.k8s.error = None;
                        app.k8s.loading = false;
                        let rows = app.k8s.visible().len();
                        app.k8s.selected = app.k8s.selected.min(rows.saturating_sub(1));
                    }
                    Msg::Failed(listing, e) if listing == app.k8s.listing => {
                        app.k8s.blocked = e == Error::NotInstalled;
                        app.k8s.error = Some(e.to_string());
                        app.k8s.loading = false;
                    }
                    Msg::Contexts(list, current) => {
                        app.k8s.contexts = list;
                        if app.k8s.client.context.is_none() {
                            app.k8s.context = current;
                        }
                    }
                    _ => app.k8s.loading = false,
                }
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => done.push(i),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    for i in done.into_iter().rev() {
        app.k8s.jobs.remove(i);
    }
    // Live only while the panel has the keyboard: leaving it stops the
    // polling, and nothing runs when Qell is doing something else. The
    // context list comes from the kubeconfig, not the cluster, so it has
    // nothing to poll.
    let live = app.focus == Focus::K8s && app.k8s.open && app.k8s.listing.kind != CONTEXTS && !app.k8s.blocked;
    if live && !app.k8s.loading && app.k8s.fetched.is_some_and(|t| t.elapsed() >= REFRESH) {
        app.k8s.fetch(ctx);
    }
    if live {
        ctx.request_repaint_after(REFRESH);
    }
}

/// The whole panel, drawn where the editor would be.
pub fn show(app: &mut QellApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    update(app, ctx);
    if app.focus == Focus::K8s {
        keys(app, ctx);
    }
    let (dim, text, accent, warn, err, hover) =
        (app.palette.text_dim, app.palette.text, app.palette.accent, app.palette.warning, app.palette.error, app.palette.hover);
    let font = FontId::monospace(app.config.font_size.max(11.0) - 1.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + ROW_PAD;

    header(app, ui, &font, dim, accent);
    ui.add_space(2.0);

    let visible = app.k8s.visible();
    let widths = app.k8s.table.widths();
    // The bottom line is drawn last but reserves its space first.
    // Two rows: a long kubectl error wraps instead of running off.
    let bottom = row_h * 2.0 + 6.0;
    let area = ui.available_rect_before_wrap();
    let table_rect = Rect::from_min_max(area.min, Pos2::new(area.max.x, area.max.y - bottom));
    app.k8s.page = ((table_rect.height() / row_h) as usize).saturating_sub(2).max(1);

    let mut open_row = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(table_rect));
    child.spacing_mut().item_spacing.y = 0.0;
    // Column header, then the rows.
    column_header(&app.k8s.table, &widths, &mut child, &font, row_h, accent);
    let rows = visible.len();
    let selected = app.k8s.selected.min(rows.saturating_sub(1));
    if rows == 0 {
        let what = if app.k8s.loading {
            "reading the cluster…".to_string()
        } else if app.k8s.error.is_some() {
            String::new() // The bottom line says what went wrong.
        } else if !app.k8s.filter.is_empty() {
            format!("nothing matches {}", app.k8s.filter)
        } else {
            format!("no {} here", app.k8s.listing.kind)
        };
        child.add_space(6.0);
        child.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(what).font(font.clone()).color(dim));
        });
    }
    egui::ScrollArea::vertical().auto_shrink([false, false]).show_rows(&mut child, row_h, rows, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let row = &app.k8s.table.rows[visible[i]];
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::click());
            let painter = ui.painter_at(rect);
            if i == selected {
                painter.rect_filled(rect, 0.0, accent.gamma_multiply(0.35));
            } else if resp.hovered() {
                painter.rect_filled(rect, 0.0, hover);
            }
            let color = match row.health {
                Health::Ok => text,
                Health::Warn => warn,
                Health::Bad => err,
                Health::Plain => text.gamma_multiply(0.9),
            };
            painter.text(
                Pos2::new(rect.left() + 8.0, rect.center().y),
                Align2::LEFT_CENTER,
                cells_line(&row.cells, &widths),
                font.clone(),
                color,
            );
            if resp.clicked() {
                open_row = Some((i, resp.double_clicked()));
            }
        }
    });
    if let Some((i, enter)) = open_row {
        app.k8s.selected = i;
        app.focus = Focus::K8s;
        if enter {
            drill_in(app, ctx);
        }
    }
    // Keep the cursor row on screen when the keyboard moved it.
    if app.k8s.selected != selected {
        app.k8s.selected = selected;
    }

    // Clicking anywhere in the panel gives it the keyboard again, which
    // is what starts the refreshing.
    if ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_interact_pos().is_some_and(|p| area.contains(p)) {
        app.focus = Focus::K8s;
    }

    let line_rect = Rect::from_min_max(Pos2::new(area.min.x, area.max.y - bottom), area.max);
    bottom_line(app, ui, line_rect, &font, dim, accent, err);
    if app.k8s.help {
        help_overlay(ctx, &font, text, dim);
    }
}

/// `NAME        READY  STATUS` — cells padded to the column widths.
fn cells_line(cells: &[String], widths: &[usize]) -> String {
    let mut out = String::new();
    for (i, cell) in cells.iter().enumerate() {
        let w = widths.get(i).copied().unwrap_or(0);
        out.push_str(cell);
        if i + 1 < cells.len() {
            let pad = w.saturating_sub(cell.chars().count()) + 2;
            out.extend(std::iter::repeat_n(' ', pad));
        }
    }
    out
}

fn column_header(table: &Table, widths: &[usize], ui: &mut egui::Ui, font: &FontId, row_h: f32, accent: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
    ui.painter_at(rect).text(
        Pos2::new(rect.left() + 8.0, rect.center().y),
        Align2::LEFT_CENTER,
        cells_line(&table.columns, widths),
        font.clone(),
        accent,
    );
}

/// The block at the top: where we are and what we are looking at.
fn header(app: &mut QellApp, ui: &mut egui::Ui, font: &FontId, dim: Color32, accent: Color32) {
    let k = &app.k8s;
    let context = k.client.context.clone().or_else(|| k.context.clone()).unwrap_or_else(|| "—".into());
    let namespace = k.listing.namespace.clone().unwrap_or_else(|| "all".into());
    let live = app.focus == Focus::K8s;
    let rows = k.visible().len();
    let pairs = [
        ("Context", context),
        ("Namespace", namespace),
        ("Resource", k.listing.title()),
        ("Count", rows.to_string()),
        (
            "Refresh",
            match (k.listing.kind.as_str(), live) {
                (CONTEXTS, _) => "from your kubeconfig".to_string(),
                (_, true) => "live, every 2s".to_string(),
                (_, false) => "paused — click to resume".to_string(),
            },
        ),
    ];
    let mut job = LayoutJob::default();
    let label = TextFormat { font_id: font.clone(), color: dim, ..Default::default() };
    let value = TextFormat { font_id: font.clone(), color: accent, ..Default::default() };
    for (i, (name, text)) in pairs.iter().enumerate() {
        if i > 0 {
            job.append("   ", 0.0, label.clone());
        }
        job.append(&format!("{name}: "), 0.0, label.clone());
        job.append(text, 0.0, value.clone());
    }
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        ui.label(job);
    });
}

/// The last line: what is being typed, the error, or the keys.
fn bottom_line(app: &mut QellApp, ui: &mut egui::Ui, rect: Rect, font: &FontId, dim: Color32, accent: Color32, error: Color32) {
    let painter = ui.painter_at(rect);
    let at = Pos2::new(rect.left() + 8.0, rect.top() + 2.0);
    let (text, color) = match (app.k8s.typing, &app.k8s.error) {
        (Typing::Filter, _) => (format!("/{}", app.k8s.line), accent),
        (Typing::Command, _) => (format!(":{}", app.k8s.line), accent),
        (_, Some(e)) => (e.clone(), error),
        _ if !app.k8s.filter.is_empty() => (format!("filter: {}  (esc clears)", app.k8s.filter), accent),
        _ => {
            ("j/k move   enter open   esc back   / filter   : resource   0 all namespaces   r refresh   ? help".to_string(), dim)
        }
    };
    let galley = painter.layout(text, font.clone(), color, rect.width() - 24.0);
    painter.galley(at, galley, color);
    if app.k8s.loading {
        painter.text(Pos2::new(rect.right() - 8.0, rect.top() + 2.0), Align2::RIGHT_TOP, "…", font.clone(), dim);
    }
}

fn help_overlay(ctx: &egui::Context, font: &FontId, text: Color32, dim: Color32) {
    let lines = [
        ("j / k, ↓ / ↑", "move"),
        ("g / G", "first / last"),
        ("Ctrl+d / Ctrl+u", "half a page"),
        ("Enter", "go in — a namespace, a workload, a node, a pod"),
        ("Esc", "back, or clear the filter"),
        ("/", "filter the rows"),
        (":", "a resource (:po, :svc, :ns, :ctx) or :q to close"),
        ("0", "every namespace"),
        ("n / c", "namespaces / contexts"),
        ("r", "refresh now"),
        ("?", "this list"),
    ];
    egui::Area::new(egui::Id::new("qell-k8s-help"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                for (keys, what) in lines {
                    let mut job = LayoutJob::default();
                    job.append(
                        &format!("{keys:<16}"),
                        0.0,
                        TextFormat { font_id: font.clone(), color: text, ..Default::default() },
                    );
                    job.append(what, 0.0, TextFormat { font_id: font.clone(), color: dim, ..Default::default() });
                    ui.label(job);
                }
            });
        });
}

/// Everything the panel does with the keyboard.
fn keys(app: &mut QellApp, ctx: &egui::Context) {
    // The help overlay is up: the next key of its own puts it away. This
    // is a frame after the one that opened it, so `?` doesn't close it
    // the instant it appears.
    if app.k8s.help {
        if ctx.input(|i| {
            i.key_pressed(Key::Escape) || i.key_pressed(Key::Questionmark) || i.key_pressed(Key::Enter) || i.key_pressed(Key::Q)
        }) {
            app.k8s.help = false;
        }
        return;
    }
    if app.k8s.typing != Typing::No {
        typing_keys(app, ctx);
        return;
    }
    let rows = app.k8s.visible().len();
    let page = app.k8s.page;
    let moved = |k: &mut K8sState, to: usize| k.selected = to.min(rows.saturating_sub(1));
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
        let k = &mut app.k8s;
        match (key, modifiers.ctrl || modifiers.command) {
            (Key::J | Key::ArrowDown, false) => moved(k, k.selected + 1),
            (Key::K | Key::ArrowUp, false) => k.selected = k.selected.saturating_sub(1),
            (Key::G, false) if modifiers.shift => moved(k, rows.saturating_sub(1)),
            (Key::G, false) => k.selected = 0,
            (Key::Home, false) => k.selected = 0,
            (Key::End, false) => moved(k, rows.saturating_sub(1)),
            (Key::D, true) | (Key::PageDown, false) => moved(k, k.selected + page),
            (Key::U, true) | (Key::PageUp, false) => k.selected = k.selected.saturating_sub(page),
            (Key::Slash, false) => {
                k.typing = Typing::Filter;
                k.line = k.filter.clone();
            }
            (Key::Colon | Key::Semicolon, false) => {
                k.typing = Typing::Command;
                k.line.clear();
            }
            (Key::Questionmark, false) => k.help = true,
            // Esc goes back a step; at the top it stays put, so a stray
            // Esc never drops you into the editor mid-keystroke. The
            // panel closes with its own shortcut or `:q`.
            (Key::Escape, false) => {
                if !k.filter.is_empty() {
                    k.filter.clear();
                } else {
                    k.back(ctx);
                }
            }
            (Key::Enter, false) => drill_in(app, ctx),
            (Key::R, false) => {
                app.k8s.error = None;
                app.k8s.blocked = false;
                if app.k8s.listing.kind == CONTEXTS {
                    show_contexts(app, ctx);
                } else {
                    app.k8s.fetch(ctx);
                }
            }
            (Key::Num0, false) => {
                let mut l = app.k8s.listing.clone();
                l.namespace = None;
                app.k8s.set(l, ctx);
            }
            (Key::N, false) => app.k8s.set(Listing::new("namespaces", None), ctx),
            (Key::C, false) => show_contexts(app, ctx),
            _ => {}
        }
    }
}

/// The filter and the `:` line.
fn typing_keys(app: &mut QellApp, ctx: &egui::Context) {
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        match event {
            egui::Event::Text(t) => app.k8s.line.push_str(&t),
            egui::Event::Key { key: Key::Backspace, pressed: true, .. } => {
                app.k8s.line.pop();
            }
            egui::Event::Key { key: Key::Escape, pressed: true, .. } => {
                app.k8s.typing = Typing::No;
                app.k8s.line.clear();
            }
            egui::Event::Key { key: Key::Enter, pressed: true, .. } => {
                let line = std::mem::take(&mut app.k8s.line);
                let mode = app.k8s.typing;
                app.k8s.typing = Typing::No;
                match mode {
                    Typing::Filter => {
                        app.k8s.filter = line;
                        app.k8s.selected = 0;
                    }
                    Typing::Command => run_command(app, &line, ctx),
                    Typing::No => {}
                }
            }
            _ => {}
        }
    }
    // The filter shows its effect as it is typed, like k9s.
    if app.k8s.typing == Typing::Filter {
        app.k8s.filter = app.k8s.line.clone();
        app.k8s.selected = 0;
    }
}

/// `:pods`, `:po`, `:ns`, `:ctx`, `:q`.
fn run_command(app: &mut QellApp, line: &str, ctx: &egui::Context) {
    let word = line.trim();
    if word.is_empty() {
        return;
    }
    match word {
        "q" | "quit" | "close" => close(app),
        "ctx" | "context" | "contexts" => show_contexts(app, ctx),
        _ => {
            let kind = qell_k8s::resolve(word);
            let namespace = if qell_k8s::is_namespaced(&kind) { app.k8s.listing.namespace.clone() } else { None };
            app.k8s.set(Listing::new(&kind, namespace), ctx);
        }
    }
}

/// The contexts of the kubeconfig, as a table of their own.
fn show_contexts(app: &mut QellApp, ctx: &egui::Context) {
    app.k8s.fetch_contexts(ctx);
    let current = app.k8s.client.context.clone().or_else(|| app.k8s.context.clone());
    let rows = app
        .k8s
        .contexts
        .iter()
        .map(|name| qell_k8s::Row {
            cells: vec![name.clone(), if Some(name) == current.as_ref() { "current".into() } else { String::new() }],
            name: name.clone(),
            namespace: None,
            health: if Some(name) == current.as_ref() { Health::Ok } else { Health::Plain },
            object: serde_json::Value::Null,
        })
        .collect();
    app.k8s.stack.push((app.k8s.listing.clone(), app.k8s.selected));
    app.k8s.listing = Listing::new(CONTEXTS, None);
    app.k8s.table = Table { columns: vec!["CONTEXT".into(), "".into()], rows };
    app.k8s.selected = 0;
    app.k8s.filter.clear();
    app.k8s.error = None;
}

/// Enter: what this row leads to.
fn drill_in(app: &mut QellApp, ctx: &egui::Context) {
    let visible = app.k8s.visible();
    let Some(index) = visible.get(app.k8s.selected).copied() else { return };
    let Some(row) = app.k8s.table.rows.get(index).cloned() else { return };
    let kind = app.k8s.listing.kind.clone();
    match kind.as_str() {
        CONTEXTS => {
            app.k8s.client.context = Some(row.name.clone());
            app.k8s.context = Some(row.name.clone());
            app.k8s.stack.clear();
            app.k8s.set(Listing::new("pods", None), ctx);
            app.notify(format!("Context {}", row.name));
        }
        "namespaces" => {
            let mut listing = Listing::new("pods", Some(row.name.clone()));
            listing.selector = None;
            app.k8s.go(listing, ctx);
        }
        "nodes" => {
            let mut listing = Listing::new("pods", None);
            listing.field_selector = Some(format!("spec.nodeName={}", row.name));
            app.k8s.go(listing, ctx);
        }
        "pods" => {
            let mut listing = Listing::new("pods", row.namespace.clone().or_else(|| app.k8s.listing.namespace.clone()));
            listing.containers_of = Some(row.name.clone());
            app.k8s.go(listing, ctx);
        }
        _ => {
            // Anything with a selector leads to the pods it owns.
            let Some(selector) = selector_of(&row.object) else {
                app.notify(format!("Nothing to open on a {}.", trim_s(&kind)));
                return;
            };
            let mut listing = Listing::new("pods", row.namespace.clone().or_else(|| app.k8s.listing.namespace.clone()));
            listing.selector = Some(selector);
            app.k8s.go(listing, ctx);
        }
    }
}

/// `app=api,tier=web` from a workload's or a service's selector.
fn selector_of(object: &serde_json::Value) -> Option<String> {
    let labels = [&object["spec"]["selector"]["matchLabels"], &object["spec"]["selector"]]
        .into_iter()
        .find_map(|v| v.as_object().filter(|m| m.values().all(serde_json::Value::is_string)))?;
    let pairs: Vec<String> = labels.iter().map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or(""))).collect();
    (!pairs.is_empty()).then(|| pairs.join(","))
}

fn trim_s(kind: &str) -> String {
    kind.strip_suffix('s').unwrap_or(kind).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn cells_are_padded_into_columns() {
        let line = cells_line(&["api".to_string(), "1/1".to_string()], &[10, 3]);
        assert_eq!(line, "api         1/1");
        // The last cell is never padded, so long messages don't stretch.
        assert!(!line.ends_with(' '));
    }

    #[test]
    fn selectors_come_from_either_shape() {
        let deployment = json!({ "spec": { "selector": { "matchLabels": { "app": "api" } } } });
        assert_eq!(selector_of(&deployment).as_deref(), Some("app=api"));
        let service = json!({ "spec": { "selector": { "app": "api" } } });
        assert_eq!(selector_of(&service).as_deref(), Some("app=api"));
        let config_map = json!({ "data": { "a": "b" } });
        assert_eq!(selector_of(&config_map), None);
    }

    #[test]
    fn the_filter_keeps_the_rows_that_match_any_cell() {
        let mut state = K8sState::default();
        let row = |name: &str, status: &str| qell_k8s::Row {
            cells: vec![name.into(), status.into()],
            name: name.into(),
            namespace: None,
            health: Health::Ok,
            object: serde_json::Value::Null,
        };
        state.table = Table {
            columns: vec!["NAME".into(), "STATUS".into()],
            rows: vec![row("api-0", "Running"), row("web-1", "CrashLoopBackOff")],
        };
        assert_eq!(state.visible(), vec![0, 1]);
        state.filter = "crash".into();
        assert_eq!(state.visible(), vec![1]);
        state.filter = "API".into();
        assert_eq!(state.visible(), vec![0]);
    }
}
