//! The AWS view: an account read the way the cluster view reads a
//! cluster.
//!
//! One table at a time, keyboard first: `:` picks the service, `Enter`
//! goes in, `Esc` comes back, `/` filters, `1`…`9` change region and `p`
//! changes profile. Everything is read through your own `aws` CLI on a
//! worker thread (see `qell-aws`), so your profiles, SSO sessions and
//! roles are used as they are and Qell holds no credentials.
//!
//! Unlike the cluster view nothing here is on a timer: the CLI is slow
//! and the APIs are throttled, so a list is read when you open it and
//! again when you press `r`. This first version only reads.

use std::sync::mpsc::{channel, Receiver};
use std::time::Instant;

use egui::text::LayoutJob;
use egui::{Align2, Color32, FontId, Key, Pos2, Rect, Sense, TextFormat, Vec2};
use qell_aws::{Client, Error, Health, Identity, Listing, Table};

use super::k8s::{cells_line, column_header, elide, field_of, help_box, keep_visible, Colors, TextView, LOGO};
use crate::app::{Focus, QellApp};

/// A row's height is the line height; this is its padding.
const ROW_PAD: f32 = 2.0;

/// What the worker sends back. The number is which account-and-region
/// the question was asked of, so an answer that arrives after a switch
/// is dropped rather than shown under the wrong name.
enum Msg {
    Table(u64, Listing, Box<Table>),
    Failed(u64, Listing, Error),
    Who(u64, Result<Identity, Error>, Option<String>),
    Profiles(Vec<String>),
    /// What to run to sign in, worked out from the profile.
    Login(Vec<String>),
}

/// What the bottom line is collecting, if anything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Typing {
    No,
    Filter,
    Command,
}

pub struct AwsState {
    pub open: bool,
    pub typing: Typing,
    pub client: Client,
    pub listing: Listing,
    pub table: Table,
    /// Where Esc goes back to.
    stack: Vec<(Listing, usize)>,
    pub error: Option<String>,
    loading: bool,
    jobs: Vec<Receiver<Msg>>,
    pub selected: usize,
    pub filter: String,
    /// What is being typed on the bottom line.
    line: String,
    /// The completion offered for it, and which one is shown.
    matches: Vec<String>,
    match_at: usize,
    /// When the list on screen was read.
    fetched: Option<Instant>,
    pub profiles: Vec<String>,
    pub identity: Option<Identity>,
    /// Who was asked for already, so opening the view asks once.
    asked_who: bool,
    /// The region the profile itself is set to.
    pub default_region: Option<String>,
    /// Bumped whenever the profile or the region changes.
    epoch: u64,
    /// Whether the sidebar was showing before the view took over.
    sidebar_was: Option<bool>,
    pub help: bool,
    page: usize,
    /// A row's details, shown over the table.
    pub text: Option<TextView>,
    scroll_to: bool,
    offset: f32,
    offset_x: f32,
    /// The terminal tab a sign-in is running in. When it closes, the
    /// sign-in is over and what is on screen is read again.
    signing_in: Option<usize>,
}

impl Default for AwsState {
    fn default() -> Self {
        Self {
            open: false,
            typing: Typing::No,
            client: Client::default(),
            listing: Listing::Functions,
            table: Table::default(),
            stack: Vec::new(),
            error: None,
            loading: false,
            jobs: Vec::new(),
            selected: 0,
            filter: String::new(),
            line: String::new(),
            matches: Vec::new(),
            match_at: 0,
            fetched: None,
            profiles: Vec::new(),
            identity: None,
            asked_who: false,
            default_region: None,
            epoch: 0,
            sidebar_was: None,
            help: false,
            page: 20,
            text: None,
            scroll_to: false,
            offset: 0.0,
            offset_x: 0.0,
            signing_in: None,
        }
    }
}

impl AwsState {
    fn visible(&self) -> Vec<usize> {
        self.table.matching(&self.filter)
    }

    /// The region calls go to: the one picked here, else the profile's.
    pub fn region(&self) -> Option<String> {
        self.client.region.clone().or_else(|| self.default_region.clone())
    }

    /// The profile's name as the CLI would resolve it.
    pub fn profile(&self) -> String {
        self.client.profile.clone().or_else(|| std::env::var("AWS_PROFILE").ok()).unwrap_or_else(|| "default".to_string())
    }

    fn spawn(&mut self, name: &str, ctx: &egui::Context, work: impl FnOnce() -> Msg + Send + 'static) -> bool {
        let (tx, rx) = channel();
        let ctx = ctx.clone();
        let started = std::thread::Builder::new().name(name.to_string()).spawn(move || {
            let _ = tx.send(work());
            ctx.request_repaint();
        });
        match started {
            Ok(_) => {
                self.jobs.push(rx);
                true
            }
            Err(e) => {
                self.error = Some(format!("cannot run aws: {e}"));
                false
            }
        }
    }

    /// Ask for this listing; the answer arrives in a later frame.
    fn fetch(&mut self, ctx: &egui::Context) {
        let (client, listing, epoch) = (self.client.clone(), self.listing.clone(), self.epoch);
        self.loading = self.spawn("aws-list", ctx, move || match client.list(&listing, qell_table::now()) {
            Ok(table) => Msg::Table(epoch, listing, Box::new(table)),
            Err(e) => Msg::Failed(epoch, listing, e),
        });
    }

    /// Who we are and where the profile points: one cheap call, asked
    /// once per profile.
    fn fetch_who(&mut self, ctx: &egui::Context) {
        let (client, epoch) = (self.client.clone(), self.epoch);
        self.asked_who = true;
        self.spawn("aws-who", ctx, move || Msg::Who(epoch, client.identity(), client.configured_region()));
    }

    /// `i`: work out how this profile signs in. The answer comes back
    /// as a command for a terminal tab, where the browser prompt shows.
    fn sign_in(&mut self, ctx: &egui::Context) {
        if self.signing_in.is_some() {
            return;
        }
        let client = self.client.clone();
        self.spawn("aws-login", ctx, move || Msg::Login(client.login_args()));
    }

    fn fetch_profiles(&mut self, ctx: &egui::Context) {
        let client = self.client.clone();
        self.spawn("aws-profiles", ctx, move || Msg::Profiles(client.profiles().unwrap_or_default()));
    }

    /// Show `listing`, remembering where we were so Esc comes back.
    fn go(&mut self, listing: Listing, ctx: &egui::Context) {
        self.stack.push((self.listing.clone(), self.selected));
        self.show(listing, ctx);
    }

    /// Show `listing` from the top (switching service with `:`).
    fn set(&mut self, listing: Listing, ctx: &egui::Context) {
        self.stack.clear();
        self.show(listing, ctx);
    }

    fn show(&mut self, listing: Listing, ctx: &egui::Context) {
        self.listing = listing;
        self.text = None;
        self.table = Table::default();
        self.selected = 0;
        self.filter.clear();
        self.error = None;
        self.fetched = None;
        self.fetch(ctx);
    }

    fn back(&mut self, ctx: &egui::Context) -> bool {
        let Some((listing, selected)) = self.stack.pop() else { return false };
        self.show(listing, ctx);
        self.selected = selected;
        self.scroll_to = true;
        true
    }

    /// Look somewhere else: every answer still on its way is for the old
    /// place, and what is on screen is read again.
    fn switch(&mut self, profile: Option<String>, region: Option<String>, ctx: &egui::Context) {
        self.epoch += 1;
        if let Some(profile) = profile {
            self.client.profile = Some(profile);
            // The new profile has a region of its own.
            self.client.region = None;
            self.default_region = None;
            self.identity = None;
            self.fetch_who(ctx);
        }
        if let Some(region) = region {
            self.client.region = Some(region);
        }
        // A picker was only the way here: go back to what it covered.
        if self.listing.is_picker() {
            if !self.back(ctx) {
                self.set(Listing::Functions, ctx);
            }
            return;
        }
        let listing = self.listing.clone();
        let selected = self.selected;
        self.show(listing, ctx);
        self.selected = selected;
    }
}

pub fn open(app: &mut QellApp, ctx: &egui::Context) {
    app.aws.client.bin = app.config.aws.clone();
    // One place at a time: the account, the cluster or the editor.
    if app.k8s.open {
        super::k8s::close(app);
    }
    if !app.aws.open {
        app.aws.sidebar_was = Some(app.show_sidebar);
    }
    app.show_sidebar = false;
    app.aws.open = true;
    app.focus = Focus::Aws;
    if app.aws.profiles.is_empty() {
        app.aws.fetch_profiles(ctx);
    }
    if !app.aws.asked_who {
        app.aws.fetch_who(ctx);
    }
    // Opening the view reads what it shows, once; after that it is `r`.
    if app.aws.fetched.is_none() && !app.aws.loading {
        app.aws.error = None;
        app.aws.fetch(ctx);
    }
}

pub fn close(app: &mut QellApp) {
    if let Some(was) = app.aws.sidebar_was.take() {
        app.show_sidebar = was;
    }
    app.aws.open = false;
    app.aws.typing = Typing::No;
    app.aws.help = false;
    if app.focus == Focus::Aws {
        app.focus = Focus::Editor;
    }
}

/// Take whatever the workers finished. Called once a frame.
pub fn update(app: &mut QellApp, ctx: &egui::Context) {
    // The sign-in's tab closed: it is over. Whether it worked is what
    // the next read says, so read.
    if let Some(tab) = app.aws.signing_in {
        if !app.terminals.iter().any(|t| t.number == tab) {
            app.aws.signing_in = None;
            app.aws.error = None;
            app.aws.fetch_who(ctx);
            app.aws.fetch(ctx);
            // Back to where `i` was pressed.
            if app.aws.open {
                app.focus = Focus::Aws;
            }
        }
    }
    let mut login = None;
    let a = &mut app.aws;
    let mut received = Vec::new();
    a.jobs.retain(|rx| match rx.try_recv() {
        Ok(msg) => {
            received.push(msg);
            false
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => false,
        Err(std::sync::mpsc::TryRecvError::Empty) => true,
    });
    for msg in received {
        match msg {
            Msg::Table(epoch, listing, table) if epoch == a.epoch && listing == a.listing => {
                a.table = *table;
                a.error = None;
                a.loading = false;
                a.fetched = Some(Instant::now());
                a.selected = a.selected.min(a.visible().len().saturating_sub(1));
            }
            Msg::Failed(epoch, listing, e) if epoch == a.epoch && listing == a.listing => {
                a.error = Some(e.to_string());
                a.loading = false;
                a.fetched = Some(Instant::now());
            }
            Msg::Who(epoch, who, region) if epoch == a.epoch => {
                a.default_region = region;
                // A failed sign-in shows up on the list as well, in the
                // CLI's own words; the header just stays empty.
                a.identity = who.ok();
            }
            Msg::Profiles(list) => a.profiles = list,
            Msg::Login(args) => login = Some(args),
            _ => {}
        }
    }
    // `sso login` names its own profile or session, and a region means
    // nothing to it, so it runs without the usual leading arguments.
    if let Some(args) = login {
        let bin = app.aws.client.bin.clone();
        let before = app.terminals.len();
        app.run_in_terminal(format!("sign in {}", app.aws.profile()), &bin, args);
        if app.terminals.len() > before {
            app.aws.signing_in = app.terminals.last().map(|t| t.number);
        }
    }
}

/// The whole panel, drawn where the editor would be.
pub fn show(app: &mut QellApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    if app.focus == Focus::Aws {
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
        key_ns: app.palette.token(qell_core::TokenKind::Keyword),
        chart: accent,
        dim,
        logo: accent.gamma_multiply(0.8),
    };
    let keys_in_header = header(app, ui, &font, &colors);
    ui.add_space(2.0);

    let visible = app.aws.visible();
    let widths = app.aws.table.widths();
    let bottom = row_h * 2.0 + 6.0;
    let area = ui.available_rect_before_wrap();
    let frame_rect = Rect::from_min_max(area.min + Vec2::new(6.0, 8.0), Pos2::new(area.max.x - 6.0, area.max.y - bottom));
    let table_rect = frame_rect.shrink2(Vec2::new(6.0, 5.0));
    let line_rect = Rect::from_min_max(Pos2::new(area.min.x, area.max.y - bottom), area.max);
    app.aws.page = ((table_rect.height() / row_h) as usize).saturating_sub(2).max(1);
    table_frame(app, ui, frame_rect, &font, accent, dim);

    if app.aws.text.is_some() {
        text_view(app, ui, table_rect, &font, row_h, text, accent);
        bottom_line(app, ui, line_rect, &font, dim, accent, err, keys_in_header);
        return;
    }

    let mut open_row = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(table_rect));
    child.spacing_mut().item_spacing.y = 0.0;
    column_header(&app.aws.table.columns, &widths, &mut child, &font, row_h, accent, app.aws.offset_x);
    let rows = visible.len();
    let selected = app.aws.selected.min(rows.saturating_sub(1));
    if rows == 0 {
        let what = if app.aws.loading {
            "asking aws…".to_string()
        } else if app.aws.error.is_some() {
            String::new() // The bottom line says what went wrong.
        } else if !app.aws.filter.is_empty() {
            format!("nothing matches {}", app.aws.filter)
        } else if app.aws.listing.global() || app.aws.listing.is_picker() {
            "nothing here".to_string()
        } else {
            format!("nothing here in {}", app.aws.region().unwrap_or_else(|| "this region".into()))
        };
        child.add_space(6.0);
        child.horizontal(|ui| {
            ui.add_space(8.0);
            ui.label(egui::RichText::new(what).font(font.clone()).color(dim));
        });
    }
    let mut list = egui::ScrollArea::both().auto_shrink([false, false]);
    if app.aws.scroll_to {
        let view_h = (table_rect.height() - row_h).max(row_h);
        list = list.vertical_scroll_offset(keep_visible(app.aws.offset, view_h, selected, row_h));
    }
    let char_w = ui.fonts_mut(|f| f.glyph_width(&font, 'M'));
    let line_w = (widths.iter().sum::<usize>() + 2 * widths.len()) as f32 * char_w + 16.0;
    let scrolled = list.show_rows(&mut child, row_h, rows, |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let row = &app.aws.table.rows[visible[i]];
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
    app.aws.offset = scrolled.state.offset.y;
    app.aws.offset_x = scrolled.state.offset.x;
    app.aws.scroll_to = false;
    app.aws.selected = selected;
    if let Some((i, enter)) = open_row {
        app.aws.selected = i;
        app.focus = Focus::Aws;
        if enter {
            enter_row(app, ctx);
        }
    }
    if ctx.input(|i| i.pointer.any_pressed()) && ctx.pointer_interact_pos().is_some_and(|p| area.contains(p)) {
        app.focus = Focus::Aws;
    }

    bottom_line(app, ui, line_rect, &font, dim, accent, err, keys_in_header);
    if app.aws.help {
        help_overlay(ctx, &font, text, dim);
    }
}

/// The block at the top: which account, where, and the keys. Says
/// whether the key columns fitted, so the bottom line only repeats them
/// when they didn't.
fn header(app: &mut QellApp, ui: &mut egui::Ui, font: &FontId, colors: &Colors) -> bool {
    let a = &app.aws;
    let dash = |text: &str| if text.is_empty() { "—".to_string() } else { text.to_string() };
    let (account, who) = match &a.identity {
        Some(id) => (dash(&id.account), dash(&id.who)),
        None if a.asked_who && a.error.is_none() => ("…".to_string(), "…".to_string()),
        None => ("—".to_string(), "—".to_string()),
    };
    let region = a.region();
    // CloudFront and the list of buckets are the same from everywhere.
    let region_shown = if a.listing.global() { "global".to_string() } else { region.clone().unwrap_or_else(|| "—".into()) };
    let read = match (a.loading, a.fetched) {
        (true, _) => "reading…".to_string(),
        (false, Some(at)) => format!("{} ago, r reads again", qell_table::age(at.elapsed().as_secs() as i64)),
        (false, None) => "—".to_string(),
    };
    let facts = [
        ("Profile", a.profile()),
        ("Account", account),
        ("Identity", who),
        ("Region", region_shown),
        ("Read", read),
        ("Qell Rev", env!("CARGO_PKG_VERSION").to_string()),
    ];
    let regions: Vec<(String, String, bool)> = qell_aws::regions(a.default_region.as_deref())
        .into_iter()
        .enumerate()
        .map(|(i, r)| ((i + 1).to_string(), r.clone(), region.as_deref() == Some(r.as_str())))
        .collect();
    let keys_left = [
        ("<enter>", "In"),
        ("<esc>", "Back"),
        ("<j/k>", "Move"),
        ("<g/G>", "Top/End"),
        ("<ctrl-f/u>", "Page"),
        ("<r>", "Read again"),
    ];
    let keys_mid =
        [("<d>", "Describe"), ("<l>", "Logs"), ("<s>", "Shell"), ("<c>", "Copy name"), ("<i>", "Sign in"), ("<?>", "Help")];
    let keys_right = [
        ("<:>", "Service"),
        ("</>", "Filter"),
        ("<tab>", "Complete"),
        ("<1-9>", "Region"),
        ("<p>", "Profiles"),
        ("<:q>", "Close"),
    ];
    let columns_of_keys: [&[(&str, &str)]; 3] = [&keys_left, &keys_mid, &keys_right];

    let char_w = ui.fonts_mut(|f| f.glyph_width(font, 'M'));
    let line_h = ui.fonts_mut(|f| f.row_height(font));
    let rows = facts.len().max(regions.len()).max(keys_left.len());
    let full = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(full.min, Vec2::new(full.width(), rows as f32 * line_h + 8.0));
    let painter = ui.painter_at(rect);
    let at = |col: usize, row: usize| Pos2::new(rect.left() + 8.0 + col as f32 * char_w, rect.top() + 4.0 + row as f32 * line_h);
    let put = |pos: Pos2, text: &str, color: Color32| {
        painter.text(pos, Align2::LEFT_TOP, text, font.clone(), color);
    };
    let label_w = facts.iter().map(|(l, _)| l.chars().count() + 2).max().unwrap_or(0);
    let value_w = facts.iter().map(|(_, v)| elide(v, 40).chars().count()).max().unwrap_or(0) + 3;
    let region_key_w = 5;
    let region_w = region_key_w + regions.iter().map(|(_, r, _)| r.chars().count()).max().unwrap_or(0) + 3;

    let columns = ((rect.width() - 16.0) / char_w) as usize;
    for (row, (name, value)) in facts.iter().enumerate() {
        put(at(0, row), &format!("{name}:"), colors.label);
        put(at(label_w, row), &elide(value, 40), colors.value);
    }
    let mut x = label_w + value_w;
    if columns >= x + region_w {
        for (row, (key, name, on)) in regions.iter().enumerate() {
            put(at(x, row), &format!("<{key}>"), colors.key_ns);
            put(at(x + region_key_w, row), name, if *on { colors.value } else { colors.dim });
        }
        x += region_w;
    }
    let mut keys_shown = false;
    for column in columns_of_keys {
        let key_w = column.iter().map(|(k, _)| k.chars().count()).max().unwrap_or(0) + 1;
        let width = key_w + column.iter().map(|(_, w)| w.chars().count()).max().unwrap_or(0) + 2;
        if columns < x + width {
            break;
        }
        for (row, (key, what)) in column.iter().enumerate() {
            put(at(x, row), key, colors.key);
            put(at(x + key_w, row), what, colors.dim);
        }
        x += width;
        keys_shown = true;
    }
    let logo_w = LOGO.iter().map(|l| l.chars().count()).max().unwrap_or(0);
    if columns >= x + logo_w + 2 {
        for (row, line) in LOGO.iter().enumerate() {
            let left = rect.right() - 12.0 - logo_w as f32 * char_w;
            painter.text(Pos2::new(left, at(0, row).y), Align2::LEFT_TOP, *line, font.clone(), colors.logo);
        }
    }
    ui.advance_cursor_after_rect(rect);
    keys_shown
}

/// The border around the table, with what it lists on the top edge:
/// `Lambda(eu-west-3)[42]`.
fn table_frame(app: &QellApp, ui: &mut egui::Ui, rect: Rect, font: &FontId, accent: Color32, dim: Color32) {
    let painter = ui.painter_at(rect.expand(10.0));
    painter.rect_stroke(rect, 4.0, egui::Stroke::new(1.0_f32, accent.gamma_multiply(0.5)), egui::StrokeKind::Inside);
    let a = &app.aws;
    let fmt = |c: Color32| TextFormat { font_id: font.clone(), color: c, ..Default::default() };
    let mut job = LayoutJob::default();
    if let Some(view) = &a.text {
        job.append(&view.title, 0.0, fmt(accent));
    } else {
        job.append(&a.listing.title(), 0.0, fmt(accent));
        if !a.listing.is_picker() {
            let where_ = if a.listing.global() { "global".to_string() } else { a.region().unwrap_or_else(|| "—".into()) };
            job.append(&format!("({where_})"), 0.0, fmt(dim));
        }
        job.append(&format!("[{}]", a.visible().len()), 0.0, fmt(accent));
    }
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top() - galley.size().y / 2.0);
    // The border runs behind the title, so it is painted over.
    painter.rect_filled(Rect::from_min_size(at, galley.size()).expand2(Vec2::new(4.0, 0.0)), 0.0, app.palette.editor_bg);
    painter.galley(at, galley, accent);
}

/// A row's details, over the table. Field names are picked out.
#[allow(clippy::too_many_arguments)]
fn text_view(app: &mut QellApp, ui: &mut egui::Ui, rect: Rect, font: &FontId, row_h: f32, text: Color32, accent: Color32) {
    let Some(view) = &app.aws.text else { return };
    let (lines, at) = (view.lines.clone(), view.at);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    child.spacing_mut().item_spacing.y = 0.0;
    let mut area = egui::ScrollArea::both().auto_shrink([false, false]);
    if app.aws.scroll_to {
        area = area.vertical_scroll_offset(keep_visible(app.aws.offset, rect.height(), at, row_h));
    }
    let scrolled = area.show_rows(&mut child, row_h, lines.len(), |ui, range| {
        ui.spacing_mut().item_spacing.y = 0.0;
        for i in range {
            let line = &lines[i];
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), row_h), Sense::hover());
            let painter = ui.painter_at(r);
            if i == at {
                painter.rect_filled(r, 0.0, accent.gamma_multiply(0.15));
            }
            let fmt = |c: Color32| TextFormat { font_id: font.clone(), color: c, ..Default::default() };
            let mut job = LayoutJob::default();
            match field_of(line) {
                Some(end) => {
                    job.append(&line[..end], 0.0, fmt(accent));
                    job.append(&line[end..], 0.0, fmt(text));
                }
                None => job.append(line, 0.0, fmt(text)),
            }
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            painter.galley(Pos2::new(r.left() + 8.0, r.center().y - galley.size().y / 2.0), galley, text);
        }
    });
    app.aws.offset = scrolled.state.offset.y;
    app.aws.scroll_to = false;
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
    let a = &app.aws;
    let painter = ui.painter_at(rect);
    let at = Pos2::new(rect.left() + 8.0, rect.top() + 2.0);
    if a.typing != Typing::No {
        let fmt = |c: Color32| TextFormat { font_id: font.clone(), color: c, ..Default::default() };
        let mark = if a.typing == Typing::Filter { '/' } else { ':' };
        let mut job = LayoutJob::default();
        job.append(&format!("{mark}{}", a.line), 0.0, fmt(accent));
        // The first match, greyed out after what was typed: Tab takes it.
        let suggestion = a.matches.get(a.match_at).cloned().unwrap_or_default();
        if let Some(rest) = suggestion.strip_prefix(a.line.as_str()).filter(|_| !a.line.is_empty()) {
            job.append(rest, 0.0, fmt(dim));
        }
        if a.matches.len() > 1 {
            job.append(&format!("     {}", a.matches.join("  ")), 0.0, fmt(dim.gamma_multiply(0.8)));
        }
        let galley = ui.fonts_mut(|f| f.layout_job(job));
        painter.galley(at, galley, accent);
        return;
    }
    let (text, color) = match &a.error {
        Some(e) => (e.clone(), error),
        _ if a.text.is_some() => ("j/k move   esc back".to_string(), dim),
        _ if !a.filter.is_empty() => (format!("filter: {}  (esc clears)", a.filter), accent),
        _ if keys_in_header => (String::new(), dim),
        _ => (
            "j/k move   enter open   esc back   / filter   : service   1-9 region   p profiles   r read again   ? help"
                .to_string(),
            dim,
        ),
    };
    let galley = painter.layout(text, font.clone(), color, rect.width() - 24.0);
    painter.galley(at, galley, color);
    if a.loading {
        painter.text(Pos2::new(rect.right() - 8.0, rect.top() + 2.0), Align2::RIGHT_TOP, "…", font.clone(), dim);
    }
}

fn help_overlay(ctx: &egui::Context, font: &FontId, text: Color32, dim: Color32) {
    let lines: &[(&str, &str)] = &[
        ("j / k, ↓ / ↑", "move"),
        ("g / G", "first / last"),
        ("Ctrl+f / Ctrl+u", "a page down / up"),
        ("Enter", "go in — a pipeline, a cluster, a service, a bucket, an API…"),
        ("Esc", "back, or clear the filter"),
        ("d", "everything aws said about it; Esc comes back"),
        ("l", "follow a function's or a build's logs, in a terminal tab"),
        ("s", "a shell on an instance (SSM) or in an ECS task, in a terminal tab"),
        ("c", "copy its ARN, its s3:// address or its name"),
        ("/", "filter the rows"),
        (":", "a service — : alone lists them all — or :q to close"),
        (":profile x", "use another profile; p lists them"),
        (":region x", "use another region; 1-9 are in the header"),
        ("Tab", "take the suggestion, again for the next"),
        ("i", "sign in again (aws sso login) in a terminal tab; the list is read when it ends"),
        ("r", "read the list again — nothing refreshes on its own"),
        ("?", "this list"),
    ];
    help_box(ctx, "qell-aws-help", lines, font, text, dim);
}

/// Everything the panel does with the keyboard.
fn keys(app: &mut QellApp, ctx: &egui::Context) {
    if app.aws.text.is_some() {
        text_keys(app, ctx);
        return;
    }
    // A frame after the one that opened it, so `?` doesn't close it the
    // instant it appears.
    if app.aws.help {
        if ctx.input(|i| {
            i.key_pressed(Key::Escape) || i.key_pressed(Key::Questionmark) || i.key_pressed(Key::Enter) || i.key_pressed(Key::Q)
        }) {
            app.aws.help = false;
        }
        return;
    }
    if app.aws.typing != Typing::No {
        typing_keys(app, ctx);
        return;
    }
    let last = app.aws.visible().len().saturating_sub(1);
    let page = app.aws.page;
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
        let a = &mut app.aws;
        let before = a.selected;
        match (key, modifiers.ctrl || modifiers.command) {
            (Key::J | Key::ArrowDown, false) => a.selected = (a.selected + 1).min(last),
            (Key::K | Key::ArrowUp, false) => a.selected = a.selected.saturating_sub(1),
            (Key::G, false) if modifiers.shift => a.selected = last,
            (Key::G, false) | (Key::Home, false) => a.selected = 0,
            (Key::End, false) => a.selected = last,
            (Key::F, true) | (Key::PageDown, false) => a.selected = (a.selected + page).min(last),
            (Key::U, true) | (Key::PageUp, false) => a.selected = a.selected.saturating_sub(page),
            (Key::Slash, false) => {
                a.typing = Typing::Filter;
                a.line = a.filter.clone();
            }
            (Key::Colon | Key::Semicolon, false) => {
                a.typing = Typing::Command;
                a.line.clear();
                suggest(a);
            }
            (Key::Questionmark, false) => a.help = true,
            // Esc goes back a step; at the top it stays put. The view
            // closes with its own shortcut or `:q`.
            (Key::Escape, false) => {
                if !a.filter.is_empty() {
                    a.filter.clear();
                } else {
                    a.back(ctx);
                }
            }
            (Key::Enter, false) => enter_row(app, ctx),
            (Key::R, false) => {
                a.error = None;
                if a.identity.is_none() {
                    a.fetch_who(ctx);
                }
                a.fetch(ctx);
            }
            (
                Key::Num1 | Key::Num2 | Key::Num3 | Key::Num4 | Key::Num5 | Key::Num6 | Key::Num7 | Key::Num8 | Key::Num9,
                false,
            ) => {
                let nth = key as usize - Key::Num1 as usize;
                if let Some(region) = qell_aws::regions(a.default_region.as_deref()).get(nth).cloned() {
                    a.switch(None, Some(region), ctx);
                }
            }
            (Key::P, false) if a.listing != Listing::Profiles => a.go(Listing::Profiles, ctx),
            (Key::I, false) => a.sign_in(ctx),
            (Key::D | Key::Y, false) => describe(app),
            (Key::L, false) => logs(app),
            (Key::S, false) => session(app),
            (Key::C, false) => copy_name(app, ctx),
            _ => {}
        }
        if app.aws.selected != before {
            app.aws.scroll_to = true;
        }
    }
}

/// Moving about a row's details, and Esc back to the list.
fn text_keys(app: &mut QellApp, ctx: &egui::Context) {
    let page = app.aws.page;
    let events = ctx.input(|i| i.events.clone());
    let Some(view) = &mut app.aws.text else { return };
    let last = view.lines.len().saturating_sub(1);
    let before = view.at;
    let mut back = false;
    for event in events {
        let egui::Event::Key { key, pressed: true, modifiers, .. } = event else { continue };
        match (key, modifiers.ctrl || modifiers.command) {
            (Key::J | Key::ArrowDown, false) => view.at = (view.at + 1).min(last),
            (Key::K | Key::ArrowUp, false) => view.at = view.at.saturating_sub(1),
            (Key::G, false) if modifiers.shift => view.at = last,
            (Key::G, false) | (Key::Home, false) => view.at = 0,
            (Key::End, false) => view.at = last,
            (Key::F, true) | (Key::PageDown, false) => view.at = (view.at + page).min(last),
            (Key::U, true) | (Key::PageUp, false) => view.at = view.at.saturating_sub(page),
            (Key::Escape, false) | (Key::Q, false) => back = true,
            _ => {}
        }
    }
    let moved = view.at != before;
    if back {
        app.aws.text = None;
    }
    if back || moved {
        app.aws.scroll_to = true;
    }
}

/// The filter and the `:` line.
fn typing_keys(app: &mut QellApp, ctx: &egui::Context) {
    let events = ctx.input(|i| i.events.clone());
    for event in events {
        let a = &mut app.aws;
        match event {
            egui::Event::Text(t) => {
                a.line.push_str(&t);
                suggest(a);
            }
            egui::Event::Key { key: Key::Backspace, pressed: true, .. } => {
                a.line.pop();
                suggest(a);
            }
            // Tab takes the suggestion; again steps to the next one.
            egui::Event::Key { key: Key::Tab, pressed: true, modifiers, .. } => {
                if a.matches.is_empty() {
                    continue;
                }
                let n = a.matches.len();
                if a.line == a.matches[a.match_at] {
                    a.match_at = if modifiers.shift { (a.match_at + n - 1) % n } else { (a.match_at + 1) % n };
                }
                a.line = a.matches[a.match_at].clone();
            }
            egui::Event::Key { key: Key::Escape, pressed: true, .. } => {
                // Leaving the filter line keeps nothing of it.
                if a.typing == Typing::Filter {
                    a.filter.clear();
                }
                a.typing = Typing::No;
                a.line.clear();
                a.matches.clear();
            }
            egui::Event::Key { key: Key::Enter, pressed: true, .. } => {
                let suggestion = a.matches.get(a.match_at).cloned();
                let line = std::mem::take(&mut a.line);
                let mode = a.typing;
                a.typing = Typing::No;
                a.matches.clear();
                match mode {
                    Typing::Filter => {
                        a.filter = line;
                        a.selected = 0;
                    }
                    // Enter on a half-typed word takes the suggestion
                    // with it, so `:la` then Enter lists the functions.
                    // Enter on nothing is nothing, whatever is listed.
                    Typing::Command if line.trim().is_empty() => {}
                    Typing::Command => run_command(app, &suggestion.unwrap_or(line), ctx),
                    Typing::No => {}
                }
                return;
            }
            _ => {}
        }
    }
    // The filter shows its effect as it is typed.
    if app.aws.typing == Typing::Filter {
        app.aws.filter = app.aws.line.clone();
        app.aws.selected = 0;
    }
}

/// What the line could become: services and commands after `:`, their
/// profile or region after those, and the names on screen when
/// filtering.
fn suggest(a: &mut AwsState) {
    a.match_at = 0;
    a.matches = completions(a.typing, &a.line, &a.profiles, &a.table);
}

fn completions(typing: Typing, line: &str, profiles: &[String], table: &Table) -> Vec<String> {
    let typed = line.trim_start().to_lowercase();
    // `:` alone shows what there is to pick from.
    if typed.is_empty() && typing == Typing::Command {
        return qell_aws::KINDS.iter().map(|(name, _)| name.to_string()).collect();
    }
    if typed.is_empty() {
        return Vec::new();
    }
    if typing == Typing::Filter {
        let mut names: Vec<String> = table
            .rows
            .iter()
            .map(|r| r.cells.first().cloned().unwrap_or_default())
            .filter(|n| n.to_lowercase().contains(&typed) && n.to_lowercase() != typed)
            .collect();
        names.dedup();
        names.truncate(12);
        return names;
    }
    // `profile pr` → the profiles; `region eu` → the regions.
    if let Some(rest) = typed.strip_prefix("profile ") {
        return profiles
            .iter()
            .filter(|p| p.to_lowercase().starts_with(rest.trim()))
            .map(|p| format!("profile {p}"))
            .take(12)
            .collect();
    }
    if let Some(rest) = typed.strip_prefix("region ") {
        return qell_aws::REGIONS
            .iter()
            .filter(|(r, _)| r.starts_with(rest.trim()))
            .map(|(r, _)| format!("region {r}"))
            .take(12)
            .collect();
    }
    let mut matches = qell_aws::complete(&typed);
    for word in ["profile ", "region ", "q"] {
        if word.starts_with(&typed) && word.trim() != typed {
            matches.push(word.to_string());
        }
    }
    matches
}

/// `:lambda`, `:cf`, `:profile prod`, `:region eu-west-3`, `:q`.
fn run_command(app: &mut QellApp, line: &str, ctx: &egui::Context) {
    let mut words = line.split_whitespace();
    let (Some(word), arg) = (words.next(), words.next()) else { return };
    match (word, arg) {
        ("q" | "quit" | "close", _) => close(app),
        ("profile" | "profiles", None) => app.aws.go(Listing::Profiles, ctx),
        ("region" | "regions", None) => app.aws.go(Listing::Regions, ctx),
        ("profile", Some(name)) => app.aws.switch(Some(name.to_string()), None, ctx),
        ("region", Some(name)) => app.aws.switch(None, Some(name.to_string()), ctx),
        _ => match qell_aws::resolve(word) {
            Some(listing) => app.aws.set(listing, ctx),
            None => {
                let known: Vec<&str> = qell_aws::KINDS.iter().map(|(name, _)| *name).collect();
                app.aws.error = Some(format!("{word} isn't a service this view reads yet. It reads: {}.", known.join(" ")));
            }
        },
    }
}

/// The row the cursor is on.
fn selected_row(app: &QellApp) -> Option<qell_aws::Row> {
    let visible = app.aws.visible();
    let at = *visible.get(app.aws.selected)?;
    app.aws.table.rows.get(at).cloned()
}

/// Enter: into what the row contains, the pick it stands for, or its
/// details when there is nothing further down.
fn enter_row(app: &mut QellApp, ctx: &egui::Context) {
    let Some(row) = selected_row(app) else { return };
    match &app.aws.listing {
        Listing::Profiles => app.aws.switch(Some(row.name), None, ctx),
        Listing::Regions => app.aws.switch(None, Some(row.name), ctx),
        listing => match listing.enter(&row) {
            Some(next) => app.aws.go(next, ctx),
            None => describe(app),
        },
    }
}

/// Everything the CLI said about the row, as indented fields.
fn describe(app: &mut QellApp) {
    let Some(row) = selected_row(app) else { return };
    if row.object.is_null() || app.aws.listing.is_picker() {
        return;
    }
    let lines = match &row.object {
        // A CodeDeploy application is only its name.
        serde_json::Value::String(name) => vec![format!("Name: {name}")],
        object => qell_table::describe(object),
    };
    let short = row.cells.first().cloned().unwrap_or(row.name);
    app.aws.text = Some(TextView { title: format!("Describe({short})"), lines, at: 0 });
    app.aws.offset = 0.0;
    app.aws.scroll_to = true;
}

/// `l`: follow a function's or a build's logs in a terminal tab.
fn logs(app: &mut QellApp) {
    let Some(row) = selected_row(app) else { return };
    let Some(tail) = app.aws.listing.logs(&row) else {
        app.aws.error =
            Some("Logs are for Lambda functions (:lambda) and CodeBuild builds (:cb, then into a project).".to_string());
        return;
    };
    let mut args = app.aws.client.base_args();
    args.extend(tail);
    let bin = app.aws.client.bin.clone();
    let what = row.cells.first().cloned().unwrap_or_else(|| row.name.clone());
    app.run_in_terminal(format!("logs {what}"), &bin, args);
}

/// `s`: a shell in a terminal tab — Session Manager on an instance, ECS
/// Exec in a task. No SSH key and no open port; the CLI says what is
/// missing when its session plugin isn't installed.
fn session(app: &mut QellApp) {
    let Some(row) = selected_row(app) else { return };
    let args = match app.aws.listing.shell(&row) {
        Some(Ok(args)) => args,
        Some(Err(why)) => {
            app.aws.error = Some(why);
            return;
        }
        None => {
            app.aws.error = Some("A shell is for EC2 instances (:ec2) and ECS tasks (:ecs, into a service).".to_string());
            return;
        }
    };
    let mut all = app.aws.client.base_args();
    all.extend(args);
    // An instance goes by its Name tag, a task by its id.
    let label = match app.aws.listing {
        Listing::Instances => row.cells.get(1).filter(|n| n.as_str() != "-").cloned().unwrap_or_else(|| row.name.clone()),
        _ => row.name.clone(),
    };
    let bin = app.aws.client.bin.clone();
    app.run_in_terminal(format!("sh {label}"), &bin, all);
}

/// `c`: what you would paste into a command or a ticket.
fn copy_name(app: &mut QellApp, ctx: &egui::Context) {
    let Some(row) = selected_row(app) else { return };
    let text = address(&app.aws.listing, &row);
    ctx.copy_text(text.clone());
    app.notify(format!("Copied {text}"));
}

/// A row's ARN when it has one, its `s3://` address in a bucket, else
/// its name.
fn address(listing: &Listing, row: &qell_aws::Row) -> String {
    match listing {
        Listing::Buckets => format!("s3://{}", row.name),
        Listing::Objects { bucket, .. } => format!("s3://{bucket}/{}", row.name),
        _ => ["FunctionArn", "ARN", "Arn"]
            .iter()
            .find_map(|key| row.object[*key].as_str().map(str::to_string))
            .unwrap_or_else(|| row.name.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use qell_aws::Row;

    fn row(name: &str, object: serde_json::Value) -> Row {
        Row { cells: vec![name.to_string()], name: name.to_string(), health: Health::Plain, object }
    }

    #[test]
    fn the_command_line_completes_services_then_their_argument() {
        let profiles = vec!["dev".to_string(), "prod".to_string(), "preprod".to_string()];
        let none = Table::default();
        let complete = |line: &str| completions(Typing::Command, line, &profiles, &none);
        assert_eq!(complete("la"), ["lambda"]);
        assert_eq!(complete("code"), ["codedeploy", "codepipeline", "codebuild"]);
        assert_eq!(complete("sq"), ["sqs"]);
        // A command that takes a name completes to itself and a space…
        assert_eq!(complete("pro"), ["profile "]);
        assert_eq!(complete("re"), ["region "]);
        // …and then to the names.
        assert_eq!(complete("profile pr"), ["profile prod", "profile preprod"]);
        assert_eq!(complete("region eu-west"), ["region eu-west-1", "region eu-west-3", "region eu-west-2"]);
        // Nothing typed yet: every service, to pick from.
        assert_eq!(complete("").len(), qell_aws::KINDS.len());
        assert!(completions(Typing::Filter, "", &profiles, &none).is_empty());
    }

    #[test]
    fn the_filter_completes_from_the_names_on_screen() {
        let mut table = Table::new(&["NAME"]);
        for name in ["api-prod", "api-dev", "worker"] {
            table.rows.push(row(name, serde_json::Value::Null));
        }
        assert_eq!(completions(Typing::Filter, "api", &[], &table), ["api-prod", "api-dev"]);
        assert!(completions(Typing::Filter, "worker", &[], &table).is_empty(), "nothing left to add");
    }

    #[test]
    fn what_is_copied_is_what_you_would_paste() {
        let arn = "arn:aws:lambda:eu-west-3:123456789012:function:api";
        assert_eq!(address(&Listing::Functions, &row("api", serde_json::json!({ "FunctionArn": arn }))), arn);
        assert_eq!(address(&Listing::Buckets, &row("assets", serde_json::Value::Null)), "s3://assets");
        let inside = Listing::Objects { bucket: "assets".into(), prefix: "img/".into() };
        assert_eq!(address(&inside, &row("img/logo.png", serde_json::Value::Null)), "s3://assets/img/logo.png");
        assert_eq!(address(&Listing::Instances, &row("i-0a", serde_json::json!({}))), "i-0a");
    }
}
