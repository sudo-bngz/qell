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
/// How often the header's cluster facts are read again.
const INFO_REFRESH: Duration = Duration::from_secs(10);
/// A row's height is the line height; this is its padding.
const ROW_PAD: f32 = 2.0;

/// What the worker sends back.
enum Msg {
    Table(Listing, Box<Table>),
    Failed(Listing, Error),
    Info(Box<qell_k8s::Info>),
    /// What everything is using now, from `kubectl top`: the listing's
    /// own rows, and the cluster's CPU and memory for the header.
    Usage(qell_k8s::Metrics, Option<u8>, Option<u8>),
    /// `kubectl describe`, the YAML, or a manifest to edit.
    Text(Shown, Box<qell_k8s::Object>, String),
    TextFailed(String),
    /// What `kubectl apply` said about a saved manifest.
    Applied(Result<String, String>),
    /// What `kubectl delete` said.
    Deleted(Result<String, String>),
    Contexts(Vec<String>, Option<String>),
}

/// The delete dialog: what it will delete, and how.
pub struct Confirm {
    pub object: qell_k8s::Object,
    pub cascade: qell_k8s::Cascade,
    pub force: bool,
    /// Which line the keys are on.
    pub field: Field,
}

/// The rows of the dialog, in the order Tab walks them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Propagation,
    Force,
    Cancel,
    Ok,
}

impl Field {
    fn next(self, back: bool) -> Self {
        let order = [Field::Propagation, Field::Force, Field::Cancel, Field::Ok];
        let at = order.iter().position(|f| *f == self).unwrap_or(0);
        let n = order.len();
        order[if back { (at + n - 1) % n } else { (at + 1) % n }]
    }
}

/// `kubectl describe` or an object's YAML, shown in the panel itself.
pub struct TextView {
    pub title: String,
    pub lines: Vec<String>,
    /// The line the keys are on, which is what is kept on screen.
    pub at: usize,
}

/// What `d`, `y` and `e` fetch about the selected object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Describe,
    Yaml,
    /// The manifest, in a tab you can change and save back.
    Edit,
}

/// What the bottom line is collecting, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    No,
    Filter,
    Command,
    /// The ports of a port-forward (`local:remote`).
    Forward,
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
    /// The completion offered for it, and which one is shown.
    matches: Vec<String>,
    match_at: usize,
    fetched: Option<Instant>,
    pub contexts: Vec<String>,
    pub context: Option<String>,
    /// Whether the sidebar was showing before the view took over, so
    /// leaving it puts the editor back the way it was.
    sidebar_was: Option<bool>,
    /// Cluster, user, versions, CPU/MEM and the namespace keys.
    pub info: qell_k8s::Info,
    info_at: Option<Instant>,
    pub help: bool,
    /// Rows that fit on screen, for Ctrl+D and page keys.
    page: usize,
    /// `d` and `y` show their text here, over the table, the way k9s
    /// keeps everything in the one window.
    pub text: Option<TextView>,
    /// The delete dialog, when it is up.
    pub confirm: Option<Confirm>,
    /// The cursor moved by key: bring its row back on screen.
    scroll_to: bool,
    /// Where the list is scrolled to, to work out whether it has to move.
    offset: f32,
    /// How far right the list is scrolled, so the column header follows.
    offset_x: f32,
    /// What `kubectl top` last said, for the usage columns.
    metrics: qell_k8s::Metrics,
    /// Cluster CPU and memory over the last few minutes, for the header
    /// sparklines: how it is going, not only where it is now.
    history: std::collections::VecDeque<(u8, u8)>,
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
            matches: Vec::new(),
            match_at: 0,
            fetched: None,
            contexts: Vec::new(),
            context: None,
            sidebar_was: None,
            info: qell_k8s::Info::default(),
            info_at: None,
            help: false,
            page: 20,
            text: None,
            confirm: None,
            scroll_to: false,
            offset: 0.0,
            offset_x: 0.0,
            metrics: qell_k8s::Metrics::default(),
            history: std::collections::VecDeque::new(),
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
        let metrics = self.metrics.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.loading = true;
        self.fetched = Some(Instant::now());
        let started = std::thread::Builder::new().name("kubectl-list".into()).spawn(move || {
            let msg = match client.list(&listing, qell_k8s::now(), &metrics) {
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

    /// Ask what things are using. One command for the kinds that have
    /// usage columns, skipped when the cluster has no metrics-server.
    fn fetch_usage(&mut self, ctx: &egui::Context) {
        if self.info.no_metrics {
            return;
        }
        let client = self.client.clone();
        let kind = self.listing.kind.clone();
        let namespace = self.listing.namespace.clone();
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("kubectl-top".into()).spawn(move || {
            // One `top nodes` gives the header its CPU and memory and,
            // when the list is of nodes, their rows as well.
            let nodes = client.run(&["top", "nodes", "--no-headers"]).ok();
            let (cpu, mem) = nodes.as_deref().map(qell_k8s::parse_top).unwrap_or((None, None));
            let metrics = match kind.as_str() {
                "pods" => client.top_pods(namespace.as_deref()).unwrap_or_default(),
                "nodes" => nodes.as_deref().map(|t| qell_k8s::tables::parse_usage(t, false)).unwrap_or_default(),
                _ => qell_k8s::Metrics::default(),
            };
            let _ = tx.send(Msg::Usage(metrics, cpu, mem));
            ctx.request_repaint();
        });
        if spawned.is_ok() {
            self.jobs.push(rx);
        }
    }

    /// Read what the header shows. Cheap enough for its own timer, and
    /// it skips `kubectl top` once a cluster has said it has no metrics.
    fn fetch_info(&mut self, ctx: &egui::Context) {
        let client = self.client.clone();
        let with_metrics = !self.info.no_metrics;
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        self.info_at = Some(Instant::now());
        if std::thread::Builder::new()
            .name("kubectl-info".into())
            .spawn(move || {
                let _ = tx.send(Msg::Info(Box::new(client.info(with_metrics))));
                ctx.request_repaint();
            })
            .is_ok()
        {
            self.jobs.push(rx);
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

    /// Send a manifest file back to the cluster on a worker thread.
    pub fn apply(&mut self, client: Client, path: std::path::PathBuf, what: String, ctx: &egui::Context) {
        let (tx, rx) = channel();
        let ctx2 = ctx.clone();
        self.loading = true;
        let spawned = std::thread::Builder::new().name("kubectl-apply".into()).spawn(move || {
            let said = client.apply(&path).map(|out| if out.is_empty() { format!("Applied {what}") } else { out });
            let _ = tx.send(Msg::Applied(said.map_err(|e| e.to_string())));
            ctx2.request_repaint();
        });
        match spawned {
            Ok(_) => self.jobs.push(rx),
            Err(_) => self.loading = false,
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
        self.text = None;
        self.table = Table::default();
        self.selected = 0;
        self.filter.clear();
        self.error = None;
        self.blocked = false;
        self.fetch(ctx);
        self.fetch_usage(ctx);
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
    // One place at a time: the cluster, the account or the editor.
    if app.aws.open {
        super::aws::close(app);
    }
    // The cluster is a place of its own, not a panel beside the editor:
    // the file tree goes away with the editor and comes back with it.
    if !app.k8s.open {
        app.k8s.sidebar_was = Some(app.show_sidebar);
    }
    app.show_sidebar = false;
    app.k8s.open = true;
    app.focus = Focus::K8s;
    if app.k8s.contexts.is_empty() {
        app.k8s.fetch_contexts(ctx);
    }
    if app.k8s.info_at.is_none() {
        app.k8s.fetch_info(ctx);
    }
    if app.k8s.table.rows.is_empty() {
        let listing = app.k8s.listing.clone();
        app.k8s.set(listing, ctx);
    }
    app.k8s.fetch_usage(ctx);
}

pub fn close(app: &mut QellApp) {
    if let Some(was) = app.k8s.sidebar_was.take() {
        app.show_sidebar = was;
    }
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
    // The answers are taken first, then acted on: opening a tab or
    // saying something needs the whole app, which the loop holds.
    let mut received: Vec<Msg> = Vec::new();
    let mut done = Vec::new();
    for (i, rx) in app.k8s.jobs.iter().enumerate() {
        match rx.try_recv() {
            Ok(msg) => {
                done.push(i);
                received.push(msg);
            }
            Err(std::sync::mpsc::TryRecvError::Disconnected) => done.push(i),
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
    }
    for i in done.into_iter().rev() {
        app.k8s.jobs.remove(i);
    }
    for msg in received {
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
            Msg::Text(what, object, text) => {
                app.k8s.loading = false;
                show_object(app, what, &object, &text);
            }
            Msg::Deleted(result) => {
                app.k8s.loading = false;
                match result {
                    Ok(said) => {
                        app.notify(if said.is_empty() { "Deleted.".to_string() } else { said });
                        app.k8s.fetch(ctx);
                    }
                    Err(e) => app.notify_error(format!("kubectl delete: {}", e.lines().next().unwrap_or(&e))),
                }
            }
            Msg::Applied(result) => {
                app.k8s.loading = false;
                match result {
                    Ok(said) => app.notify(if said.is_empty() { "Applied.".to_string() } else { said }),
                    Err(e) => app.notify_error(format!("kubectl apply: {}", e.lines().next().unwrap_or(&e))),
                }
            }
            Msg::TextFailed(message) => {
                app.k8s.loading = false;
                app.notify_error(message);
            }
            // Kept for the next time the list is built, a tick away,
            // rather than asking for the list again here.
            Msg::Usage(metrics, cpu, mem) => {
                if !metrics.is_empty() {
                    app.k8s.metrics = metrics;
                }
                if let (Some(c), Some(m)) = (cpu, mem) {
                    app.k8s.info.cpu = cpu;
                    app.k8s.info.mem = mem;
                    app.k8s.history.push_back((c, m));
                    // Two minutes at the list's own pace.
                    while app.k8s.history.len() > 60 {
                        app.k8s.history.pop_front();
                    }
                }
            }
            Msg::Info(info) => {
                // A cluster that has just said it has no metrics
                // keeps that answer, so the next read doesn't ask again.
                // The usage job keeps CPU and MEM up to date between
                // these, so the slower read doesn't undo them.
                let (cpu, mem) = (app.k8s.info.cpu.or(info.cpu), app.k8s.info.mem.or(info.mem));
                app.k8s.info = *info;
                app.k8s.info.cpu = cpu;
                app.k8s.info.mem = mem;
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
    // Live only while the panel has the keyboard: leaving it stops the
    // polling, and nothing runs when Qell is doing something else. The
    // context list comes from the kubeconfig, not the cluster, so it has
    // nothing to poll.
    let live = app.focus == Focus::K8s && app.k8s.open && app.k8s.listing.kind != CONTEXTS && !app.k8s.blocked;
    if live && !app.k8s.loading && app.k8s.fetched.is_some_and(|t| t.elapsed() >= REFRESH) {
        app.k8s.fetch(ctx);
        app.k8s.fetch_usage(ctx);
    }
    // The header moves slowly (a version never, CPU every few seconds),
    // so it has a timer of its own.
    if live && app.k8s.info_at.is_some_and(|t| t.elapsed() >= INFO_REFRESH) {
        app.k8s.fetch_info(ctx);
    }
    if live {
        ctx.request_repaint_after(REFRESH);
    }
}

/// The whole panel, drawn where the editor would be.
pub fn show(app: &mut QellApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    if app.focus == Focus::K8s {
        keys(app, ctx);
    }
    let (dim, text, accent, warn, err, hover) =
        (app.palette.text_dim, app.palette.text, app.palette.accent, app.palette.warning, app.palette.error, app.palette.hover);
    let font = FontId::monospace(app.config.font_size.max(11.0) - 1.0);
    let row_h = ui.fonts_mut(|f| f.row_height(&font)) + ROW_PAD;

    let colors = Colors {
        label: warn,
        value: text,
        key: accent,
        key_ns: accent,
        chart: accent.gamma_multiply(0.85),
        dim,
        logo: accent.gamma_multiply(0.8),
    };
    let keys_in_header = header(app, ui, &font, &colors);
    ui.add_space(2.0);

    let visible = app.k8s.visible();
    let widths = app.k8s.table.widths();
    // The bottom line is drawn last but reserves its space first.
    // Two rows: a long kubectl error wraps instead of running off.
    let bottom = row_h * 2.0 + 6.0;
    let area = ui.available_rect_before_wrap();
    let frame_rect = Rect::from_min_max(area.min + Vec2::new(6.0, 8.0), Pos2::new(area.max.x - 6.0, area.max.y - bottom));
    let table_rect = frame_rect.shrink2(Vec2::new(6.0, 5.0));
    app.k8s.page = ((table_rect.height() / row_h) as usize).saturating_sub(2).max(1);
    table_frame(app, ui, frame_rect, &font, accent, dim);

    if app.k8s.text.is_some() {
        text_view(app, ui, table_rect, &font, row_h, text, accent);
        let line_rect = Rect::from_min_max(Pos2::new(area.min.x, area.max.y - bottom), area.max);
        bottom_line(app, ui, line_rect, &font, dim, accent, err, keys_in_header);
        return;
    }

    let mut open_row = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(table_rect));
    child.spacing_mut().item_spacing.y = 0.0;
    // Column header, then the rows.
    column_header(&app.k8s.table.columns, &widths, &mut child, &font, row_h, accent, app.k8s.offset_x);
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
    let mut list = egui::ScrollArea::both().auto_shrink([false, false]);
    if app.k8s.scroll_to {
        // `show_rows` only draws what is on screen, so the row to follow
        // may not exist yet: the offset is worked out rather than asked
        // of a widget that isn't there.
        let view_h = (table_rect.height() - row_h).max(row_h);
        list = list.vertical_scroll_offset(keep_visible(app.k8s.offset, view_h, selected, row_h));
    }
    // With the usage columns a pod line is wider than most windows, so
    // the rows are as wide as their content and the view scrolls.
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
    let line_w = (widths.iter().sum::<usize>() + 2 * widths.len()) as f32 * char_w + 16.0;
    let scrolled = list.show_rows(&mut child, row_h, rows, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let row = &app.k8s.table.rows[visible[i]];
            let (rect, resp) = ui.allocate_exact_size(Vec2::new(line_w.max(ui.available_width()), row_h), Sense::click());
            let painter = ui.painter_at(rect);
            if i == selected {
                painter.rect_filled(rect, 0.0, app.palette.selection);
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
    app.k8s.offset = scrolled.state.offset.y;
    app.k8s.offset_x = scrolled.state.offset.x;
    app.k8s.scroll_to = false;
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
    bottom_line(app, ui, line_rect, &font, dim, accent, err, keys_in_header);
    if app.k8s.help {
        help_overlay(ctx, &font, text, dim);
    }
    if app.k8s.confirm.is_some() {
        delete_dialog(app, ctx, &font, text, dim, accent, err);
    }
}

/// k9s's delete box: a frame with its title on the border, what it is
/// about to do, how to do it, and two buttons.
#[allow(clippy::too_many_arguments)]
fn delete_dialog(
    app: &mut QellApp,
    ctx: &egui::Context,
    font: &FontId,
    text: Color32,
    dim: Color32,
    accent: Color32,
    error: Color32,
) {
    let Some(confirm) = &app.k8s.confirm else { return };
    let where_ = match &confirm.object.namespace {
        Some(ns) => format!("{ns}/{}", confirm.object.name),
        None => confirm.object.name.clone(),
    };
    let question = format!("Delete {} {where_}?", confirm.object.kind);
    let (cascade, force, field) = (confirm.cascade, confirm.force, confirm.field);
    let background = app.palette.editor_bg;
    let (mut chosen, mut run, mut cancel) = (None, false, false);

    egui::Area::new(egui::Id::new("qell-k8s-delete"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
        .show(ctx, |ui| {
            let frame = egui::Frame::new()
                .fill(background)
                .stroke(egui::Stroke::new(1.0_f32, accent))
                .inner_margin(egui::Margin::symmetric(18, 12));
            frame.show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.set_min_width(420.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(&question).font(font.clone()).color(error));
                });
                ui.add_space(4.0);
                // Two fields; the one the keys are on is marked.
                let mut row = |ui: &mut egui::Ui, on: Field, name: &str, value: String| {
                    let caret = if field == on { "›" } else { " " };
                    let colour = if field == on { text } else { dim };
                    let label = format!("{caret} {name:<14}{value}");
                    let r = ui.add(egui::Button::new(egui::RichText::new(label).font(font.clone()).color(colour)).frame(false));
                    if r.clicked() {
                        chosen = Some(on);
                    }
                };
                row(ui, Field::Propagation, "Propagation:", cascade.label().to_string());
                row(ui, Field::Force, "Force:", if force { "yes".into() } else { String::new() });
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space(ui.available_width() / 2.0 - 90.0);
                    let button = |ui: &mut egui::Ui, on: Field, name: &str| {
                        let (fg, bg) = if field == on { (background, accent) } else { (dim, Color32::TRANSPARENT) };
                        ui.add(
                            egui::Button::new(egui::RichText::new(format!("  {name}  ")).font(font.clone()).color(fg))
                                .fill(bg)
                                .corner_radius(2.0),
                        )
                        .clicked()
                    };
                    if button(ui, Field::Cancel, "Cancel") {
                        cancel = true;
                    }
                    ui.add_space(12.0);
                    if button(ui, Field::Ok, "OK") {
                        run = true;
                    }
                });
                ui.add_space(2.0);
                ui.vertical_centered(|ui| {
                    let hint = "tab moves   space changes   enter confirms   esc cancels";
                    ui.label(egui::RichText::new(hint).font(font.clone()).color(dim.gamma_multiply(0.8)));
                });
            });
        });

    // The title sits on the frame's top border, as k9s draws it.
    if let Some(rect) = ctx.memory(|m| m.area_rect(egui::Id::new("qell-k8s-delete"))) {
        let layer = egui::LayerId::new(egui::Order::Foreground, egui::Id::new("qell-k8s-delete-title"));
        let painter = ctx.layer_painter(layer);
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap("<Delete>".to_string(), font.clone(), error));
        let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() - galley.size().y / 2.0);
        painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(Vec2::new(4.0, 0.0)), 0.0, background);
        painter.galley(at, galley, error);
    }

    // Tab walks the rows, space changes the one you are on.
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
        let Some(confirm) = &mut app.k8s.confirm else { return };
        match key {
            Key::Tab | Key::ArrowDown => confirm.field = confirm.field.next(modifiers.shift),
            Key::ArrowUp => confirm.field = confirm.field.next(true),
            Key::Space | Key::ArrowRight | Key::ArrowLeft => match confirm.field {
                Field::Propagation => confirm.cascade = confirm.cascade.next(key == Key::ArrowLeft),
                Field::Force => confirm.force = !confirm.force,
                Field::Cancel => confirm.field = Field::Ok,
                Field::Ok => confirm.field = Field::Cancel,
            },
            Key::Enter => match confirm.field {
                Field::Cancel => cancel = true,
                _ => run = true,
            },
            Key::Escape => cancel = true,
            _ => {}
        }
    }
    if let (Some(field), Some(confirm)) = (chosen, &mut app.k8s.confirm) {
        // Clicking a field both moves to it and changes it.
        confirm.field = field;
        match field {
            Field::Propagation => confirm.cascade = confirm.cascade.next(false),
            Field::Force => confirm.force = !confirm.force,
            _ => {}
        }
    }
    if cancel {
        app.k8s.confirm = None;
    } else if run {
        delete_now(app, ctx);
    }
}

/// Send the delete the dialog agreed on.
fn delete_now(app: &mut QellApp, ctx: &egui::Context) {
    let Some(confirm) = app.k8s.confirm.take() else { return };
    let client = app.k8s.client.clone();
    let (object, cascade, force) = (confirm.object, confirm.cascade, confirm.force);
    let what = object.slash();
    let (tx, rx) = channel();
    let ctx2 = ctx.clone();
    app.k8s.loading = true;
    let spawned = std::thread::Builder::new().name("kubectl-delete".into()).spawn(move || {
        let said = client.delete(&object, cascade, force).map(|out| if out.is_empty() { format!("Deleted {what}") } else { out });
        let _ = tx.send(Msg::Deleted(said.map_err(|e| e.to_string())));
        ctx2.request_repaint();
    });
    match spawned {
        Ok(_) => app.k8s.jobs.push(rx),
        Err(e) => {
            app.k8s.loading = false;
            app.notify_error(format!("cannot run kubectl: {e}"));
        }
    }
}

/// Ctrl+D: ask before deleting what the cursor is on.
fn ask_delete(app: &mut QellApp) {
    if app.k8s.listing.containers_of.is_some() {
        app.notify("A container goes with its pod: delete the pod instead.");
        return;
    }
    let Some(object) = selected_object(app) else {
        app.notify("Nothing here to delete.");
        return;
    };
    app.k8s.confirm = Some(Confirm { object, cascade: qell_k8s::Cascade::default(), force: false, field: Field::Ok });
}

/// `NAME        READY  STATUS` — cells padded to the column widths.
pub(super) fn cells_line(cells: &[String], widths: &[usize]) -> String {
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

#[allow(clippy::too_many_arguments)]
pub(super) fn column_header(
    columns: &[String],
    widths: &[usize],
    ui: &mut egui::Ui,
    font: &FontId,
    row_h: f32,
    accent: Color32,
    offset_x: f32,
) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
    // The header follows the rows when the table is scrolled sideways.
    ui.painter_at(rect).text(
        Pos2::new(rect.left() + 8.0 - offset_x, rect.center().y),
        Align2::LEFT_CENTER,
        cells_line(columns, widths),
        font.clone(),
        accent,
    );
}

/// The block at the top: where we are and what we are looking at.
/// Draws the header block; says whether the key columns fitted, so the
/// bottom line only repeats them when they didn't.
fn header(app: &mut QellApp, ui: &mut egui::Ui, font: &FontId, colors: &Colors) -> bool {
    let k = &app.k8s;
    let live = app.focus == Focus::K8s && k.open;
    let context = k.client.context.clone().or_else(|| k.context.clone()).unwrap_or_else(|| "—".into());
    let dash = |text: &str| if text.is_empty() { "—".to_string() } else { text.to_string() };
    let percent = |v: Option<u8>| match (v, k.info.no_metrics) {
        (Some(p), _) => format!("{p}%"),
        (None, true) => "n/a".into(),
        (None, false) => "…".into(),
    };
    // A few minutes of cluster CPU and memory, drawn in the line itself.
    let (cpu_line, mem_line) = sparklines(&k.history);
    // (name, value, the little chart that follows it)
    let facts = [
        ("Context", dash(&context), String::new()),
        ("Refresh", if live { "live, every 2s".into() } else { "paused".to_string() }, String::new()),
        ("Cluster", dash(&k.info.cluster), String::new()),
        ("User", dash(&k.info.user), String::new()),
        ("Qell Rev", env!("CARGO_PKG_VERSION").to_string(), String::new()),
        ("K8s Rev", dash(&k.info.server), String::new()),
        ("CPU", percent(k.info.cpu), cpu_line),
        ("MEM", percent(k.info.mem), mem_line),
    ];
    // <0> every namespace, then the first few of the cluster's own.
    let current = k.listing.namespace.clone();
    let mut namespaces: Vec<(String, String, bool)> = vec![("0".into(), "all".into(), current.is_none())];
    for (i, ns) in k.info.namespaces.iter().take(9).enumerate() {
        namespaces.push(((i + 1).to_string(), ns.clone(), current.as_ref() == Some(ns)));
    }
    let keys_left = [
        ("<enter>", "In"),
        ("<esc>", "Back"),
        ("<j/k>", "Move"),
        ("<g/G>", "Top/End"),
        ("<ctrl-f/u>", "Page"),
        ("<ctrl-d>", "Delete"),
        ("<r>", "Refresh"),
    ];
    let keys_mid = [
        ("<d>", "Describe"),
        ("<y>", "YAML"),
        ("<e>", "Edit"),
        ("<s>", "Shell"),
        ("<l>", "Logs"),
        ("<shift-l>", "Logs previous"),
        ("<f>", "Port-forward"),
        ("<?>", "Help"),
    ];
    let keys_right = [
        ("<:>", "Resource"),
        ("</>", "Filter"),
        ("<tab>", "Complete"),
        ("<0-9>", "Namespace"),
        ("<n>", "Namespaces"),
        ("<c>", "Contexts"),
        ("<:q>", "Close"),
    ];

    let char_w = ui.fonts_mut(|f| f.glyph_width(font, 'M'));
    let line_h = ui.fonts_mut(|f| f.row_height(font));
    let rows = facts.len().max(namespaces.len()).max(keys_left.len());
    let height = rows as f32 * line_h + 8.0;
    let full = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(full.min, Vec2::new(full.width(), height));
    let painter = ui.painter_at(rect);
    let at = |col: usize, row: usize| Pos2::new(rect.left() + 8.0 + col as f32 * char_w, rect.top() + 4.0 + row as f32 * line_h);
    let put = |pos: Pos2, text: &str, color: Color32| {
        painter.text(pos, Align2::LEFT_TOP, text, font.clone(), color);
    };
    // Columns are laid out from what they hold, so a long cluster name
    // pushes the rest along instead of being written over.
    let widest = |it: &mut dyn Iterator<Item = usize>| it.max().unwrap_or(0);
    let label_w = widest(&mut facts.iter().map(|(l, _, _)| l.chars().count() + 1)) + 1;
    let value_w = widest(&mut facts.iter().map(|(_, v, _)| elide(v, 34).chars().count())) + 3;
    // The charts are wider than the longest value and are never cut, so
    // the column leaves room for them.
    let chart_at = 6;
    let value_w = value_w.max(chart_at + SPARK_WIDTH + 3);
    let ns_key_w = 5;
    let ns_w = ns_key_w + widest(&mut namespaces.iter().map(|(_, n, _)| elide(n, 20).chars().count())) + 3;
    let key_w = widest(&mut keys_left.iter().map(|(k, _)| k.chars().count())) + 1;
    let left_w = key_w + widest(&mut keys_left.iter().map(|(_, w)| w.chars().count())) + 2;
    let mid_key_w = widest(&mut keys_mid.iter().map(|(k, _)| k.chars().count())) + 1;
    let mid_w = mid_key_w + widest(&mut keys_mid.iter().map(|(_, w)| w.chars().count())) + 2;
    let right_key_w = widest(&mut keys_right.iter().map(|(k, _)| k.chars().count())) + 1;
    let right_w = right_key_w + widest(&mut keys_right.iter().map(|(_, w)| w.chars().count())) + 2;

    let columns = ((rect.width() - 16.0) / char_w) as usize;
    let mut x = 0;
    let mut keys_shown = false;
    for (row, (name, value, chart)) in facts.iter().enumerate() {
        put(at(x, row), &format!("{name}:"), colors.label);
        put(at(x + label_w, row), &elide(value, 34), colors.value);
        if !chart.is_empty() {
            put(at(x + label_w + chart_at, row), chart, colors.chart);
        }
    }
    x += label_w + value_w;
    if columns >= x + ns_w {
        for (row, (key, ns, on)) in namespaces.iter().enumerate() {
            put(at(x, row), &format!("<{key}>"), colors.key_ns);
            put(at(x + ns_key_w, row), &elide(ns, 20), if *on { colors.value } else { colors.dim });
        }
        x += ns_w;
    }
    if columns >= x + left_w {
        for (row, (key, what)) in keys_left.iter().enumerate() {
            put(at(x, row), key, colors.key);
            put(at(x + key_w, row), what, colors.dim);
        }
        x += left_w;
        keys_shown = true;
    }
    if columns >= x + mid_w {
        for (row, (key, what)) in keys_mid.iter().enumerate() {
            put(at(x, row), key, colors.key);
            put(at(x + mid_key_w, row), what, colors.dim);
        }
        x += mid_w;
    }
    if columns >= x + right_w {
        for (row, (key, what)) in keys_right.iter().enumerate() {
            put(at(x, row), key, colors.key);
            put(at(x + right_key_w, row), what, colors.dim);
        }
        x += right_w;
    }
    // The logo, right-aligned, when the rest has left room for it whole.
    let logo_w = K8S_LOGO.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    if columns >= x + logo_w + 2 {
        for (row, line) in K8S_LOGO.iter().enumerate() {
            let left = rect.right() - 12.0 - logo_w as f32 * char_w;
            painter.text(Pos2::new(left, at(0, row).y), Align2::LEFT_TOP, *line, font.clone(), colors.logo);
        }
    }
    ui.advance_cursor_after_rect(rect);
    keys_shown
}

/// The CPU and memory history as block characters, oldest on the left.
/// Two lines, so the header reads like a tiny chart recorder.
fn sparklines(history: &std::collections::VecDeque<(u8, u8)>) -> (String, String) {
    let line = |values: Vec<u8>| -> String {
        // Scaled against the whole, not against the window's own range:
        // a flat 20% should look flat and low, not flat and high.
        values.iter().map(|v| BLOCKS[(*v as usize * (BLOCKS.len() - 1)).div_ceil(100)]).collect()
    };
    if history.len() < 2 {
        return (String::new(), String::new());
    }
    let recent: Vec<&(u8, u8)> = history.iter().rev().take(SPARK_WIDTH).rev().collect();
    (line(recent.iter().map(|(c, _)| *c).collect()), line(recent.iter().map(|(_, m)| *m).collect()))
}

/// How many samples the header's charts show: at a reading every two
/// seconds, the last minute or so.
const SPARK_WIDTH: usize = 30;

/// Eight heights, from nothing to full.
const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Cut a value that would run into the next column, with an ellipsis.
pub(super) fn elide(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max.saturating_sub(1)).chain(std::iter::once('…')).collect()
}

/// Qell, in the spirit of the thing: a letter's width apart, so the Q's
/// tail doesn't run into the E.
const K8S_LOGO: [&str; 5] = [
    r"  ___    _____   _       _     ",
    r" / _ \  | ____| | |     | |    ",
    r"| | | | |  _|   | |     | |    ",
    r"| |_| | | |___  | |___  | |___ ",
    r" \__\_\ |_____| |_____| |_____|",
];

/// The AWS view's logo.
pub(super) const LOGO: [&str; 5] = [
    r"  ___  _____ _     _     ",
    r" / _ \| ____| |   | |    ",
    r"| | | |  _| | |   | |    ",
    r"| |_| | |___| |___| |___ ",
    r" \__\_\_____|_____|_____|",
];

/// The header's palette, so the colours are named where they are chosen.
pub(super) struct Colors {
    pub label: Color32,
    /// The CPU and memory sparklines.
    pub chart: Color32,
    pub value: Color32,
    pub key: Color32,
    pub key_ns: Color32,
    pub dim: Color32,
    pub logo: Color32,
}

/// `kubectl describe` or a manifest, in the panel. The name of a field
/// is picked out, which is most of what makes describe readable.
#[allow(clippy::too_many_arguments)]
fn text_view(app: &mut QellApp, ui: &mut egui::Ui, rect: Rect, font: &FontId, row_h: f32, text: Color32, accent: Color32) {
    let Some(view) = &app.k8s.text else { return };
    let (lines, at) = (view.lines.clone(), view.at);
    app.k8s.page = ((rect.height() / row_h) as usize).saturating_sub(2).max(1);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.spacing_mut().item_spacing.y = 0.0;
    let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
    if app.k8s.scroll_to {
        area = area.vertical_scroll_offset(keep_visible(app.k8s.offset, rect.height(), at, row_h));
    }
    let scrolled = area.show_rows(&mut child, row_h, lines.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let line = &lines[i];
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
            let painter = ui.painter_at(r);
            let left = Pos2::new(r.left() + 8.0, r.center().y);
            // `Name:` / `  app:` — the field, then its value.
            match field_of(line) {
                Some(end) => {
                    let (name, rest) = line.split_at(end);
                    let mut job = LayoutJob::default();
                    job.append(name, 0.0, TextFormat { font_id: font.clone(), color: accent, ..Default::default() });
                    job.append(rest, 0.0, TextFormat { font_id: font.clone(), color: text, ..Default::default() });
                    let galley = ui.fonts_mut(|f| f.layout_job(job));
                    painter.galley(Pos2::new(left.x, r.center().y - galley.size().y / 2.0), galley, text);
                }
                None => {
                    painter.text(left, Align2::LEFT_CENTER, line, font.clone(), text);
                }
            }
        }
    });
    app.k8s.offset = scrolled.state.offset.y;
    app.k8s.scroll_to = false;
}

/// Where a list must be scrolled to for `row` to be on screen: as it is
/// when the row is already there, otherwise just far enough.
pub(super) fn keep_visible(offset: f32, view_h: f32, row: usize, row_h: f32) -> f32 {
    let top = row as f32 * row_h;
    let bottom = top + row_h;
    if top < offset {
        top
    } else if bottom > offset + view_h {
        bottom - view_h
    } else {
        offset
    }
}

/// Where a line's field name ends (`Name:` → 5), for colouring. Only a
/// name at the start of the line counts, so URLs keep their colon.
pub(super) fn field_of(line: &str) -> Option<usize> {
    let name = line.trim_start();
    let indent = line.len() - name.len();
    let end = name.find(':')?;
    let field = &name[..end];
    let plain = |c: char| c.is_alphanumeric() || "-_./ ".contains(c);
    (!field.is_empty() && field.chars().all(plain)).then_some(indent + end + 1)
}

/// The border around the table, with what it is listing on the top edge:
/// `Pods(web)[12]`, as k9s puts it.
fn table_frame(app: &QellApp, ui: &mut egui::Ui, rect: Rect, font: &FontId, accent: Color32, dim: Color32) {
    // The title sits on the top edge, so the clip reaches above it.
    let painter = ui.painter_at(rect.expand(10.0));
    painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    if let Some(view) = &app.k8s.text {
        let galley = ui.fonts_mut(|f| f.layout_no_wrap(view.title.clone(), font.clone(), accent));
        let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() - galley.size().y / 2.0);
        painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(Vec2::new(4.0, 0.0)), 0.0, app.palette.editor_bg);
        painter.galley(at, galley, accent);
        return;
    }
    let kind = &app.k8s.listing.kind;
    let mut title = kind.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
    title.push_str(&kind.chars().skip(1).collect::<String>());
    let where_ = match (&app.k8s.listing.containers_of, &app.k8s.listing.namespace) {
        (Some(pod), _) => pod.clone(),
        (None, Some(ns)) => ns.clone(),
        (None, None) => "all".to_string(),
    };
    let count = app.k8s.visible().len();
    let mut job = LayoutJob::default();
    let fmt = |c: Color32| TextFormat { font_id: font.clone(), color: c, ..Default::default() };
    job.append(&title, 0.0, fmt(accent));
    job.append(&format!("({where_})"), 0.0, fmt(dim));
    job.append(&format!("[{count}]"), 0.0, fmt(accent));
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() - galley.size().y / 2.0);
    // The border runs behind the title, so it is painted over.
    painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(Vec2::new(4.0, 0.0)), 0.0, app.palette.editor_bg);
    painter.galley(at, galley, accent);
}

/// The last line: what is being typed, the error, or the keys.
#[allow(clippy::too_many_arguments)]
fn bottom_line(
    app: &mut QellApp,
    ui: &mut egui::Ui,
    rect: Rect,
    font: &FontId,
    dim: Color32,
    accent: Color32,
    error: Color32,
    keys_in_header: bool,
) {
    let painter = ui.painter_at(rect);
    let at = Pos2::new(rect.left() + 8.0, rect.top() + 2.0);
    // While typing, the first match is shown greyed out after what you
    // typed: Tab takes it, Tab again steps to the next.
    if app.k8s.typing != Typing::No {
        let typed = match app.k8s.typing {
            Typing::Filter => format!("/{}", app.k8s.line),
            Typing::Forward => format!("port-forward  {}   (local:remote, enter to start)", app.k8s.line),
            _ => format!(":{}", app.k8s.line),
        };
        let mut job = LayoutJob::default();
        job.append(&typed, 0.0, TextFormat { font_id: font.clone(), color: accent, ..Default::default() });
        let suggestion = app.k8s.matches.get(app.k8s.match_at).cloned().unwrap_or_default();
        if let Some(rest) = suggestion.strip_prefix(app.k8s.line.trim()) {
            job.append(rest, 0.0, TextFormat { font_id: font.clone(), color: dim, ..Default::default() });
        }
        let others: Vec<&str> = app.k8s.matches.iter().map(String::as_str).collect();
        if others.len() > 1 {
            let rest = others.join("  ");
            job.append(
                &format!("     {rest}"),
                0.0,
                TextFormat { font_id: font.clone(), color: dim.gamma_multiply(0.8), ..Default::default() },
            );
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(at, galley, accent);
        return;
    }
    let (text, color) = match (app.k8s.typing, &app.k8s.error) {
        (Typing::Filter, _) => (format!("/{}", app.k8s.line), accent),
        (Typing::Command, _) => (format!(":{}", app.k8s.line), accent),
        (_, Some(e)) => (e.clone(), error),
        _ if !app.k8s.filter.is_empty() => (format!("filter: {}  (esc clears)", app.k8s.filter), accent),
        // The header lists the keys when it has the room; no point
        // saying them twice.
        _ if keys_in_header => (String::new(), dim),
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
    let lines: &[(&str, &str)] = &[
        ("j / k, ↓ / ↑", "move"),
        ("g / G", "first / last"),
        ("Ctrl+f / Ctrl+u", "a page down / half a page up"),
        ("Ctrl+d", "delete it, after a question"),
        ("Enter", "go in — a namespace, a workload, a node, a pod"),
        ("Esc", "back, or clear the filter"),
        ("d / y", "describe it / its YAML, here; Esc comes back"),
        ("e", "edit the manifest in a tab; saving applies it"),
        ("s", "a shell in the pod, in a terminal tab"),
        ("l / L", "follow its logs / the previous container's"),
        ("f", "forward a port; it runs as long as its terminal tab"),
        ("/", "filter the rows"),
        (":", "a resource (:po, :svc, :ns, :ctx) or :q to close"),
        ("Tab", "take the suggestion, again for the next"),
        ("0", "every namespace"),
        ("n / c", "namespaces / contexts"),
        ("r", "refresh now"),
        ("?", "this list"),
    ];
    help_box(ctx, "qell-k8s-help", lines, font, text, dim);
}

/// A list of keys and what they do, in the middle of the window.
pub(super) fn help_box(ctx: &egui::Context, id: &str, lines: &[(&str, &str)], font: &FontId, text: Color32, dim: Color32) {
    egui::Area::new(egui::Id::new(id.to_string())).order(egui::Order::Foreground).anchor(Align2::CENTER_CENTER, Vec2::ZERO).show(
        ctx,
        |ui| {
            egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                for (keys, what) in lines.iter().copied() {
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
        },
    );
}

/// Everything the panel does with the keyboard.
fn keys(app: &mut QellApp, ctx: &egui::Context) {
    // The dialog takes the keys itself while it is up.
    if app.k8s.confirm.is_some() {
        return;
    }
    if app.k8s.text.is_some() {
        text_keys(app, ctx);
        return;
    }
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
            (Key::J | Key::ArrowDown, false) => {
                moved(k, k.selected + 1);
                k.scroll_to = true;
            }
            (Key::K | Key::ArrowUp, false) => {
                k.selected = k.selected.saturating_sub(1);
                k.scroll_to = true;
            }
            (Key::G, false) if modifiers.shift => {
                moved(k, rows.saturating_sub(1));
                k.scroll_to = true;
            }
            (Key::G, false) | (Key::Home, false) => {
                k.selected = 0;
                k.scroll_to = true;
            }
            (Key::End, false) => {
                moved(k, rows.saturating_sub(1));
                k.scroll_to = true;
            }
            (Key::F, true) | (Key::PageDown, false) => {
                moved(k, k.selected + page);
                k.scroll_to = true;
            }
            (Key::D, true) => ask_delete(app),
            (Key::U, true) | (Key::PageUp, false) => {
                k.selected = k.selected.saturating_sub(page);
                k.scroll_to = true;
            }
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
                app.k8s.info.no_metrics = false;
                app.k8s.fetch_info(ctx);
                if app.k8s.listing.kind == CONTEXTS {
                    show_contexts(app, ctx);
                } else {
                    app.k8s.fetch(ctx);
                }
            }
            // <0> every namespace, <1>…<9> the ones in the header.
            (Key::Num0, false) => {
                let mut l = app.k8s.listing.clone();
                l.namespace = None;
                app.k8s.set(l, ctx);
            }
            (
                Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9,
                false,
            ) => {
                let nth = key as usize - Key::Num1 as usize;
                if let Some(ns) = app.k8s.info.namespaces.get(nth).cloned() {
                    let mut l = app.k8s.listing.clone();
                    l.namespace = Some(ns);
                    l.selector = None;
                    l.field_selector = None;
                    l.containers_of = None;
                    app.k8s.set(l, ctx);
                }
            }
            (Key::S, false) => shell_into(app, ctx),
            (Key::L, false) => logs(app, modifiers.shift),
            (Key::F, false) => ask_ports(app),
            (Key::D, false) => fetch_object(app, Shown::Describe, ctx),
            (Key::Y, false) => fetch_object(app, Shown::Yaml, ctx),
            (Key::E, false) => fetch_object(app, Shown::Edit, ctx),
            (Key::N, false) => app.k8s.set(Listing::new("namespaces", None), ctx),
            (Key::C, false) => show_contexts(app, ctx),
            _ => {}
        }
    }
}

/// Moving about `kubectl describe`, and Esc back to the list.
fn text_keys(app: &mut QellApp, ctx: &egui::Context) {
    let page = app.k8s.page;
    let events = ctx.input(|i| i.events.clone());
    let Some(view) = &app.k8s.text else { return };
    let last = view.lines.len().saturating_sub(1);
    let mut at = view.at;
    let mut back = false;
    for event in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
        let ctrl = modifiers.ctrl || modifiers.command;
        match (key, ctrl) {
            (Key::J | Key::ArrowDown, false) => at = (at + 1).min(last),
            (Key::K | Key::ArrowUp, false) => at = at.saturating_sub(1),
            (Key::G, false) if modifiers.shift => at = last,
            (Key::G, false) | (Key::Home, false) => at = 0,
            (Key::End, false) => at = last,
            (Key::F, true) | (Key::PageDown, false) => at = (at + page).min(last),
            (Key::U, true) | (Key::PageUp, false) => at = at.saturating_sub(page),
            (Key::Escape, false) | (Key::Q, false) => back = true,
            _ => {}
        }
    }
    if back {
        app.k8s.text = None;
        app.k8s.scroll_to = true;
        return;
    }
    if let Some(view) = &mut app.k8s.text {
        if view.at != at {
            view.at = at;
            app.k8s.scroll_to = true;
        }
    }
}

/// The filter and the `:` line.
fn typing_keys(app: &mut QellApp, ctx: &egui::Context) {
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        match event {
            egui::Event::Text(t) => {
                app.k8s.line.push_str(&t);
                suggest(app);
            }
            egui::Event::Key { key: Key::Backspace, pressed: true, .. } => {
                app.k8s.line.pop();
                suggest(app);
            }
            // Tab takes the suggestion; again steps to the next one.
            egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. } => {
                if app.k8s.matches.is_empty() {
                    continue;
                }
                let back = modifiers.shift;
                let n = app.k8s.matches.len();
                if app.k8s.line.trim() == app.k8s.matches[app.k8s.match_at] {
                    app.k8s.match_at = if back { (app.k8s.match_at + n - 1) % n } else { (app.k8s.match_at + 1) % n };
                }
                app.k8s.line = app.k8s.matches[app.k8s.match_at].clone();
                if app.k8s.typing == Typing::Filter {
                    app.k8s.filter = app.k8s.line.clone();
                }
            }
            egui::Event::Key { key: Key::Escape, pressed: true, .. } => {
                app.k8s.typing = Typing::No;
                app.k8s.line.clear();
                app.k8s.matches.clear();
            }
            egui::Event::Key { key: Key::Enter, pressed: true, .. } => {
                // Enter on a half-typed word takes the suggestion with it,
                // so `:dep` then Enter lists deployments.
                let suggestion = app.k8s.matches.get(app.k8s.match_at).cloned();
                let line = std::mem::take(&mut app.k8s.line);
                let mode = app.k8s.typing;
                app.k8s.typing = Typing::No;
                app.k8s.matches.clear();
                match mode {
                    Typing::Filter => {
                        app.k8s.filter = line;
                        app.k8s.selected = 0;
                    }
                    Typing::Command => run_command(app, &suggestion.unwrap_or(line), ctx),
                    Typing::Forward => port_forward(app, &line),
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

/// What the line could become: resource kinds after `:`, and the names in
/// front of you when filtering.
fn suggest(app: &mut QellApp) {
    let typed = app.k8s.line.trim().to_string();
    app.k8s.match_at = 0;
    // Ports are typed out; there is nothing to suggest for them.
    app.k8s.matches = if typed.is_empty() || app.k8s.typing == Typing::Forward {
        Vec::new()
    } else if app.k8s.typing == Typing::Command {
        let mut matches = qell_k8s::complete(&typed, &app.k8s.info.kinds);
        // The commands that aren't resources belong in the list too.
        for word in ["q", "ctx"] {
            if word.starts_with(&typed.to_lowercase()) {
                matches.insert(0, word.to_string());
            }
        }
        matches
    } else {
        // Filtering: the names on screen, those that contain what is
        // typed, since that is how the filter itself matches.
        let needle = typed.to_lowercase();
        let mut names: Vec<String> = app
            .k8s
            .table
            .rows
            .iter()
            .map(|r| r.name.clone())
            .filter(|n| n.to_lowercase().contains(&needle) && n.to_lowercase() != needle)
            .collect();
        names.dedup();
        names.truncate(12);
        names
    };
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

/// A shell inside the selected pod, in a terminal tab.
fn shell_into(app: &mut QellApp, ctx: &egui::Context) {
    let Some(target) = target(app) else {
        app.notify("Put the cursor on a pod to open a shell in it.");
        return;
    };
    if target.object.kind != "pods" {
        app.notify(format!("A shell goes into a pod, not a {}.", qell_k8s::singular(&target.object.kind)));
        return;
    }
    let container = match (&target.container, target.containers.len()) {
        (Some(c), _) => Some(c.clone()),
        (None, 0 | 1) => target.containers.first().cloned(),
        // Which one is a real question, so the containers come up and
        // `s` on one of them opens the shell.
        (None, n) => {
            app.notify(format!("{n} containers in this pod: pick one and press s again."));
            let mut listing = Listing::new("pods", target.object.namespace.clone());
            listing.containers_of = Some(target.object.name.clone());
            app.k8s.go(listing, ctx);
            return;
        }
    };
    let args = exec_args(&target.object, container.as_deref());
    let name = match &container {
        Some(c) => format!("shell {}/{c}", target.object.name),
        None => format!("shell {}", target.object.name),
    };
    run(app, name, args);
}

/// `kubectl logs -f` in a terminal tab, below the list.
fn logs(app: &mut QellApp, previous: bool) {
    let Some(target) = target(app) else {
        app.notify("Put the cursor on a pod to read its logs.");
        return;
    };
    if target.object.kind != "pods" {
        app.notify(format!("Logs come from a pod, not a {}.", qell_k8s::singular(&target.object.kind)));
        return;
    }
    let args = log_args(&target.object, target.container.as_deref(), target.containers.len(), previous);
    let what = if previous { "previous logs" } else { "logs" };
    run(app, format!("{what} {}", target.object.name), args);
}

/// Ask which ports to forward, filled in from the object itself.
fn ask_ports(app: &mut QellApp) {
    let Some(target) = target(app) else {
        app.notify("Nothing here to forward a port to.");
        return;
    };
    let port = first_port(&target.row).unwrap_or(8080);
    app.k8s.typing = Typing::Forward;
    app.k8s.line = format!("{port}:{port}");
    app.k8s.matches.clear();
}

/// `kubectl port-forward` in a terminal tab: it runs as long as the tab.
fn port_forward(app: &mut QellApp, ports: &str) {
    let ports = ports.trim().to_string();
    if ports.is_empty() {
        return;
    }
    let Some(target) = target(app) else { return };
    let what = format!("{}/{}", qell_k8s::singular(&target.object.kind), target.object.name);
    let mut args = vec!["port-forward".to_string()];
    args.extend(namespace_args(&target.object));
    args.extend([what.clone(), ports.clone()]);
    run(app, format!("forward {ports} to {what}"), args);
}

/// `kubectl exec` into one container, asking for bash where the image
/// has it and sh everywhere else.
fn exec_args(object: &qell_k8s::Object, container: Option<&str>) -> Vec<String> {
    let mut args = vec!["exec".to_string(), "-it".to_string()];
    args.extend(namespace_args(object));
    args.push(object.name.clone());
    if let Some(c) = container {
        args.extend(["-c".to_string(), c.to_string()]);
    }
    args.extend([
        "--".to_string(),
        "sh".to_string(),
        "-c".to_string(),
        "command -v bash >/dev/null 2>&1 && exec bash || exec sh".to_string(),
    ]);
    args
}

/// `kubectl logs -f`. With several containers and none chosen, all of
/// them, each line saying which, as `--prefix` does.
fn log_args(object: &qell_k8s::Object, container: Option<&str>, containers: usize, previous: bool) -> Vec<String> {
    let mut args = vec!["logs".to_string(), "-f".to_string(), "--tail=200".to_string()];
    args.extend(namespace_args(object));
    args.push(object.name.clone());
    match (container, containers) {
        (Some(c), _) => args.extend(["-c".to_string(), c.to_string()]),
        (None, n) if n > 1 => args.extend(["--all-containers=true".to_string(), "--prefix".to_string()]),
        _ => {}
    }
    if previous {
        args.push("--previous".to_string());
    }
    args
}

/// `-n <namespace>`, for the kinds that live in one.
fn namespace_args(object: &qell_k8s::Object) -> Vec<String> {
    match &object.namespace {
        Some(ns) => vec!["-n".to_string(), ns.clone()],
        None => Vec::new(),
    }
}

/// The first port an object serves, for the port-forward prompt.
fn first_port(row: &serde_json::Value) -> Option<u64> {
    let containers = row["spec"]["containers"].as_array();
    if let Some(port) = containers
        .and_then(|cs| cs.iter().find_map(|c| c["ports"].as_array().and_then(|p| p.first().cloned())))
        .and_then(|p| p["containerPort"].as_u64())
    {
        return Some(port);
    }
    row["spec"]["ports"].as_array()?.first()?["port"].as_u64()
}

/// Start a kubectl command in a terminal tab of its own.
fn run(app: &mut QellApp, name: String, args: Vec<String>) {
    let bin = app.k8s.client.bin.clone();
    let mut all: Vec<String> = Vec::new();
    // The tab is tied to the context the list is showing, not to
    // whatever the kubeconfig's current context becomes later.
    if let Some(context) = app.k8s.client.context.clone().or_else(|| app.k8s.context.clone()) {
        all.extend(["--context".to_string(), context]);
    }
    all.extend(args);
    app.run_in_terminal(name, &bin, all);
}

/// The pod and container the keys act on, with the row they came from.
struct Target {
    object: qell_k8s::Object,
    /// The container chosen, when the cursor is in a pod's containers.
    container: Option<String>,
    containers: Vec<String>,
    row: serde_json::Value,
}

fn target(app: &QellApp) -> Option<Target> {
    let k = &app.k8s;
    let object = selected_object(app)?;
    let row = k.visible().get(k.selected).and_then(|i| k.table.rows.get(*i))?;
    // Inside a pod's containers: that container, and all of its siblings.
    if k.listing.containers_of.is_some() {
        let containers = k.table.rows.iter().map(|r| r.name.clone()).collect();
        return Some(Target { object, container: Some(row.name.clone()), containers, row: row.object.clone() });
    }
    let containers = row.object["spec"]["containers"]
        .as_array()
        .map(|cs| cs.iter().filter_map(|c| c["name"].as_str().map(str::to_string)).collect())
        .unwrap_or_default();
    Some(Target { object, container: None, containers, row: row.object.clone() })
}

/// The object the cursor is on, as kubectl names it.
fn selected_object(app: &QellApp) -> Option<qell_k8s::Object> {
    let k = &app.k8s;
    let row = k.visible().get(k.selected).and_then(|i| k.table.rows.get(*i))?;
    if k.listing.kind == CONTEXTS {
        return None;
    }
    // Inside a pod's containers, the object is the pod itself.
    if let Some(pod) = &k.listing.containers_of {
        return Some(qell_k8s::Object { kind: "pods".into(), name: pod.clone(), namespace: k.listing.namespace.clone() });
    }
    Some(qell_k8s::Object {
        kind: k.listing.kind.clone(),
        name: row.name.clone(),
        namespace: row.namespace.clone().or_else(|| k.listing.namespace.clone()),
    })
}

/// `d`, `y`, `e`: read the object on a worker and open it as a tab.
fn fetch_object(app: &mut QellApp, what: Shown, ctx: &egui::Context) {
    let Some(object) = selected_object(app) else {
        app.notify("Nothing to open here.");
        return;
    };
    let client = app.k8s.client.clone();
    let (tx, rx) = channel();
    let ctx2 = ctx.clone();
    app.k8s.loading = true;
    let spawned = std::thread::Builder::new().name("kubectl-object".into()).spawn(move || {
        let result = match what {
            Shown::Describe => client.describe(&object),
            Shown::Yaml | Shown::Edit => client.manifest(&object),
        };
        let msg = match result {
            Ok(text) => Msg::Text(what, Box::new(object), text),
            Err(e) => Msg::TextFailed(e.to_string()),
        };
        let _ = tx.send(msg);
        ctx2.request_repaint();
    });
    match spawned {
        Ok(_) => app.k8s.jobs.push(rx),
        Err(e) => {
            app.k8s.loading = false;
            app.notify_error(format!("cannot run kubectl: {e}"));
        }
    }
}

/// Put what came back in a tab. The panel steps aside, since it covers
/// the editor; its shortcut brings it back as it was.
fn show_object(app: &mut QellApp, what: Shown, object: &qell_k8s::Object, text: &str) {
    let context = app.k8s.client.context.clone().or_else(|| app.k8s.context.clone()).unwrap_or_default();
    let place = match &object.namespace {
        Some(ns) => format!("{ns}/{}", object.slash()),
        None => object.slash(),
    };
    match what {
        // Reading stays in the panel: `d` and `y` are a step in, like
        // going from a namespace to its pods, and Esc comes back.
        Shown::Describe | Shown::Yaml => {
            let title = if what == Shown::Describe { format!("describe {place}") } else { format!("{place} yaml") };
            app.k8s.text = Some(TextView { title, lines: text.lines().map(str::to_string).collect(), at: 0 });
            app.k8s.scroll_to = true;
        }
        // Editing is the editor's job, so that one gets a tab.
        // The tab brings the editor forward on its own.
        Shown::Edit => {
            if let Err(e) = app.open_manifest_tab(object.clone(), context, text) {
                app.notify_error(format!("Cannot open the manifest: {e}"));
            }
        }
    }
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
            app.k8s.info = qell_k8s::Info::default();
            app.k8s.fetch_info(ctx);
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

    fn pod() -> qell_k8s::Object {
        qell_k8s::Object { kind: "pods".into(), name: "api-0".into(), namespace: Some("web".into()) }
    }

    #[test]
    fn a_shell_goes_into_the_container_that_was_chosen() {
        let args = exec_args(&pod(), Some("app"));
        assert_eq!(args[..6], ["exec", "-it", "-n", "web", "api-0", "-c"]);
        assert_eq!(args[6], "app");
        // The last argument tries bash and settles for sh.
        assert!(args.last().unwrap().contains("exec bash"));
        // Without a container, kubectl picks the pod's only one: no
        // `-c` before the `--` (the one after it belongs to `sh -c`).
        let args = exec_args(&pod(), None);
        let before = &args[..args.iter().position(|a| a == "--").expect("a -- separator")];
        assert!(!before.contains(&"-c".to_string()));
    }

    #[test]
    fn logs_follow_every_container_until_one_is_chosen() {
        let all = log_args(&pod(), None, 2, false);
        assert!(all.contains(&"--all-containers=true".to_string()));
        assert!(all.contains(&"--prefix".to_string()));
        assert!(!all.contains(&"--previous".to_string()));
        let one = log_args(&pod(), Some("app"), 2, true);
        assert!(!one.contains(&"--all-containers=true".to_string()));
        assert_eq!(one[one.len() - 3..], ["-c", "app", "--previous"]);
        // One container, none chosen: nothing to say either way.
        let single = log_args(&pod(), None, 1, false);
        assert_eq!(single, ["logs", "-f", "--tail=200", "-n", "web", "api-0"]);
    }

    #[test]
    fn the_port_prompt_is_filled_in_from_the_object() {
        let pod = json!({ "spec": { "containers": [{ "ports": [{ "containerPort": 8080 }] }] } });
        assert_eq!(first_port(&pod), Some(8080));
        let service = json!({ "spec": { "ports": [{ "port": 443 }] } });
        assert_eq!(first_port(&service), Some(443));
        assert_eq!(first_port(&json!({ "spec": {} })), None);
    }

    #[test]
    fn the_sparkline_grows_with_the_numbers() {
        let history: std::collections::VecDeque<(u8, u8)> = [(0, 100), (50, 50), (100, 0)].into_iter().collect();
        let (cpu, mem) = sparklines(&history);
        assert_eq!(cpu, " ▄█");
        assert_eq!(mem, "█▄ ");
        // Nothing to draw from a single sample.
        assert_eq!(sparklines(&[(5, 5)].into_iter().collect()), (String::new(), String::new()));
    }

    #[test]
    fn the_list_only_scrolls_when_the_cursor_would_be_off_screen() {
        // Ten rows of 20px fit in a 200px view.
        assert_eq!(keep_visible(0.0, 200.0, 5, 20.0), 0.0);
        // Row 12 is below it: scroll just far enough.
        assert_eq!(keep_visible(0.0, 200.0, 12, 20.0), 60.0);
        // Row 1 is above the top: bring it to the top.
        assert_eq!(keep_visible(100.0, 200.0, 1, 20.0), 20.0);
    }

    #[test]
    fn field_names_are_picked_out_only_at_the_start_of_a_line() {
        assert_eq!(field_of("Name:         api-0"), Some(5));
        assert_eq!(field_of("  Image:  nginx"), Some(8));
        // A colon inside a value is part of the value.
        assert_eq!(field_of("    Image: ghcr.io/momo/api:1.4"), Some(10));
        assert_eq!(field_of("no colon here"), None);
        // A YAML list item is a field too.
        assert_eq!(field_of("  - name: app"), Some(9));
    }

    #[test]
    fn long_values_are_cut_so_columns_never_collide() {
        assert_eq!(elide("prod-eks", 10), "prod-eks");
        assert_eq!(elide("arn:aws:eks:eu-west-3:1234:cluster/prod", 10), "arn:aws:e…");
        assert_eq!(elide("abc", 3), "abc");
    }

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
