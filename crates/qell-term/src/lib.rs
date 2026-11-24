//! Embedded terminal for Qell, built on the Alacritty terminal core.
//!
//! [`Terminal`] owns a PTY running the user's shell, the VT state machine
//! and the grid. The UI never touches those directly: it asks for a
//! [`Snapshot`] each frame (plain rows of coloured cells), and feeds back
//! keystrokes through [`Terminal::write`] / [`Terminal::send_key`].
//!
//! Nothing here depends on egui, so the same crate could back a TUI or a
//! different GUI toolkit.

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use alacritty_terminal::event::{Event, EventListener, Notify, WindowSize};
use alacritty_terminal::event_loop::{EventLoop, Msg, Notifier};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::{self, Config, Term, TermMode};
use alacritty_terminal::tty;
use alacritty_terminal::vte::ansi::{Color, CursorShape, NamedColor, Rgb};
use anyhow::{Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};

/// Forwards terminal events to a channel the UI drains.
#[derive(Clone)]
struct Listener(Sender<Event>);

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        let _ = self.0.send(event);
    }
}

/// One rendered cell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellSnapshot {
    pub c: char,
    pub fg: [u8; 3],
    pub bg: [u8; 3],
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
    /// Second half of a wide (CJK, emoji) glyph: draw nothing here.
    pub wide_spacer: bool,
    /// Inside the mouse selection.
    pub selected: bool,
}

/// How a mouse selection grows: by character, by word (double click) or
/// by whole line (triple click).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectionKind {
    Chars,
    Words,
    Lines,
}

/// Everything the UI needs to draw one frame of the terminal.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub cols: usize,
    pub rows: usize,
    pub cells: Vec<Vec<CellSnapshot>>,
    /// `(col, row)` of the cursor in screen coordinates, if visible.
    pub cursor: Option<(usize, usize)>,
    pub cursor_shape: Option<CursorShape>,
    /// Lines scrolled back from the bottom.
    pub display_offset: usize,
    pub title: String,
}

/// Keys the UI may send that are not plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Delete,
    Insert,
    /// `Ctrl` + an ASCII letter, e.g. `Ctrl('c')`.
    Ctrl(char),
}

/// A 16-colour palette plus default fg/bg/cursor, in the same spirit as
/// `qell_core::Theme` so the two can be kept consistent.
#[derive(Debug, Clone)]
pub struct Palette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
    pub ansi: [[u8; 3]; 16],
}

impl Default for Palette {
    fn default() -> Self {
        Self {
            // The editor's own surface and text, so the terminal is part
            // of the same sheet. The sixteen colours stay sixteen colours
            // (git, ls and compilers rely on them), a notch quieter.
            foreground: [0xe2, 0xde, 0xd8],
            background: [0x11, 0x0f, 0x0e],
            cursor: [0xff, 0xff, 0xff],
            ansi: [
                [0x1e, 0x1b, 0x19],
                [0xe5, 0x6b, 0x5e],
                [0x9d, 0xb8, 0x82],
                [0xd9, 0xb9, 0x8a],
                [0x86, 0xa6, 0xc8],
                [0xb9, 0x9d, 0xc8],
                [0x86, 0xb9, 0xb1],
                [0xd3, 0xce, 0xc7],
                [0x6b, 0x65, 0x5f],
                [0xf0, 0x87, 0x7b],
                [0xb5, 0xd0, 0x9a],
                [0xee, 0xcf, 0x9f],
                [0xa3, 0xc0, 0xdf],
                [0xcf, 0xb6, 0xdc],
                [0x9f, 0xd0, 0xc8],
                [0xff, 0xff, 0xff],
            ],
        }
    }
}

impl Palette {
    fn resolve(&self, color: Color, colors: &term::color::Colors) -> [u8; 3] {
        let rgb = |c: Rgb| [c.r, c.g, c.b];
        match color {
            Color::Spec(c) => rgb(c),
            Color::Named(n) => match n {
                NamedColor::Foreground | NamedColor::BrightForeground | NamedColor::DimForeground => {
                    colors[n].map(rgb).unwrap_or(self.foreground)
                }
                NamedColor::Background => colors[n].map(rgb).unwrap_or(self.background),
                NamedColor::Cursor => colors[n].map(rgb).unwrap_or(self.cursor),
                _ => {
                    let idx = n as usize;
                    colors[idx].map(rgb).unwrap_or_else(|| self.ansi.get(idx % 16).copied().unwrap_or(self.foreground))
                }
            },
            Color::Indexed(i) => {
                let i = i as usize;
                if let Some(c) = colors[i] {
                    return rgb(c);
                }
                if i < 16 {
                    self.ansi[i]
                } else if i < 232 {
                    // 6×6×6 colour cube.
                    let i = i - 16;
                    let step = |v: usize| if v == 0 { 0 } else { (55 + v * 40) as u8 };
                    [step(i / 36), step((i / 6) % 6), step(i % 6)]
                } else {
                    // Greyscale ramp.
                    let v = (8 + (i - 232) * 10) as u8;
                    [v, v, v]
                }
            }
        }
    }
}

pub struct Terminal {
    term: Arc<FairMutex<Term<Listener>>>,
    notifier: Notifier,
    events: Receiver<Event>,
    palette: Palette,
    title: String,
    exited: bool,
    cols: usize,
    rows: usize,
}

impl Terminal {
    /// Start a shell in `cwd` with a grid of `cols`×`rows` cells. `shell`
    /// is a command line such as `zsh -l`, split on whitespace (no quoting);
    /// `None` or empty uses `$SHELL`.
    pub fn new(cols: usize, rows: usize, cwd: Option<PathBuf>, shell: Option<&str>) -> Result<Self> {
        let argv = shell.map(str::split_whitespace).and_then(|mut words| {
            let program = words.next()?.to_string();
            Some((program, words.map(String::from).collect()))
        });
        Self::start(cols, rows, cwd, argv)
    }

    /// Start one command instead of a shell, with its arguments given one
    /// by one (`kubectl exec … -- sh -c 'exec bash || exec sh'`), which
    /// splitting a command line on spaces could not express.
    pub fn command(cols: usize, rows: usize, cwd: Option<PathBuf>, program: &str, args: Vec<String>) -> Result<Self> {
        Self::start(cols, rows, cwd, Some((program.to_string(), args)))
    }

    fn start(cols: usize, rows: usize, cwd: Option<PathBuf>, argv: Option<(String, Vec<String>)>) -> Result<Self> {
        let (tx, events) = unbounded();
        let listener = Listener(tx);

        let config = Config { scrolling_history: 10_000, ..Default::default() };
        let size = term::test::TermSize::new(cols.max(2), rows.max(1));
        let term = Term::new(config, &size, listener.clone());
        let term = Arc::new(FairMutex::new(term));

        let shell = argv.map(|(program, args)| tty::Shell::new(program, args));
        let mut options = tty::Options { working_directory: cwd, shell, ..Default::default() };
        options.env.insert("TERM".into(), "xterm-256color".into());
        options.env.insert("COLORTERM".into(), "truecolor".into());
        options.env.insert("TERM_PROGRAM".into(), "qell".into());

        let window_size =
            WindowSize { num_lines: rows.max(1) as u16, num_cols: cols.max(2) as u16, cell_width: 8, cell_height: 16 };
        let pty = tty::new(&options, window_size, 0).context("opening a pty for the integrated terminal")?;

        let event_loop = EventLoop::new(term.clone(), listener, pty, false, false)?;
        let notifier = Notifier(event_loop.channel());
        let _handle = event_loop.spawn();

        Ok(Self { term, notifier, events, palette: Palette::default(), title: String::new(), exited: false, cols, rows })
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
    }

    /// Window title set by the shell or the running program (OSC 0/2);
    /// empty if none was set.
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn has_exited(&self) -> bool {
        self.exited
    }

    pub fn size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }

    /// Process pending events (title changes, exit). Call once per frame.
    pub fn poll(&mut self) {
        for ev in self.events.try_iter() {
            match ev {
                Event::Title(t) => self.title = t,
                Event::ResetTitle => self.title.clear(),
                Event::Exit | Event::ChildExit(_) => self.exited = true,
                Event::PtyWrite(s) => self.notifier.notify(s.into_bytes()),
                _ => {}
            }
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        let (cols, rows) = (cols.max(2), rows.max(1));
        if (cols, rows) == (self.cols, self.rows) {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.term.lock().resize(term::test::TermSize::new(cols, rows));
        let _ = self.notifier.0.send(Msg::Resize(WindowSize {
            num_lines: rows as u16,
            num_cols: cols as u16,
            cell_width: 8,
            cell_height: 16,
        }));
    }

    /// Write raw bytes to the shell's stdin.
    pub fn write(&self, bytes: impl Into<Cow<'static, [u8]>>) {
        self.notifier.notify(bytes);
    }

    /// Type text (what the user literally pressed).
    pub fn send_text(&self, text: &str) {
        if !text.is_empty() {
            self.write(text.as_bytes().to_vec());
        }
    }

    /// Send a special key, honouring application-cursor mode for arrows.
    pub fn send_key(&self, key: Key) {
        let app_cursor = self.term.lock().mode().contains(TermMode::APP_CURSOR);
        let seq: &[u8] = match key {
            Key::Enter => b"\r",
            Key::Backspace => b"\x7f",
            Key::Tab => b"\t",
            Key::Escape => b"\x1b",
            Key::Up => {
                if app_cursor {
                    b"\x1bOA"
                } else {
                    b"\x1b[A"
                }
            }
            Key::Down => {
                if app_cursor {
                    b"\x1bOB"
                } else {
                    b"\x1b[B"
                }
            }
            Key::Right => {
                if app_cursor {
                    b"\x1bOC"
                } else {
                    b"\x1b[C"
                }
            }
            Key::Left => {
                if app_cursor {
                    b"\x1bOD"
                } else {
                    b"\x1b[D"
                }
            }
            Key::Home => {
                if app_cursor {
                    b"\x1bOH"
                } else {
                    b"\x1b[H"
                }
            }
            Key::End => {
                if app_cursor {
                    b"\x1bOF"
                } else {
                    b"\x1b[F"
                }
            }
            Key::PageUp => b"\x1b[5~",
            Key::PageDown => b"\x1b[6~",
            Key::Delete => b"\x1b[3~",
            Key::Insert => b"\x1b[2~",
            Key::Ctrl(c) => {
                let c = c.to_ascii_lowercase();
                if c.is_ascii_lowercase() {
                    return self.write(vec![c as u8 - b'a' + 1]);
                }
                match c {
                    '[' => b"\x1b",
                    '\\' => b"\x1c",
                    ']' => b"\x1d",
                    ' ' | '@' => b"\x00",
                    _ => return,
                }
            }
        };
        self.write(seq.to_vec());
    }

    /// Grid point of a screen cell (`row` counted from the top of what is
    /// shown, which may be scrolled back).
    fn point_at(term: &Term<Listener>, col: usize, row: usize) -> Point {
        let offset = term.grid().display_offset() as i32;
        let row = row.min(term.screen_lines().saturating_sub(1));
        Point::new(Line(row as i32 - offset), Column(col.min(term.columns().saturating_sub(1))))
    }

    /// Start a selection at a screen cell; `right_half` when the pointer is
    /// on the right half of it (so a drag from there skips that char).
    pub fn select_start(&self, col: usize, row: usize, right_half: bool, kind: SelectionKind) {
        let mut term = self.term.lock();
        let p = Self::point_at(&term, col, row);
        let ty = match kind {
            SelectionKind::Chars => SelectionType::Simple,
            SelectionKind::Words => SelectionType::Semantic,
            SelectionKind::Lines => SelectionType::Lines,
        };
        term.selection = Some(Selection::new(ty, p, if right_half { Side::Right } else { Side::Left }));
    }

    /// Extend the selection to a screen cell (while dragging).
    pub fn select_update(&self, col: usize, row: usize, right_half: bool) {
        let mut term = self.term.lock();
        let p = Self::point_at(&term, col, row);
        if let Some(sel) = term.selection.as_mut() {
            sel.update(p, if right_half { Side::Right } else { Side::Left });
        }
    }

    /// Select everything: the scrollback and the screen.
    pub fn select_all(&self) {
        let mut term = self.term.lock();
        let top = Point::new(term.grid().topmost_line(), Column(0));
        let bottom = Point::new(term.grid().bottommost_line(), Column(term.columns().saturating_sub(1)));
        let mut sel = Selection::new(SelectionType::Simple, top, Side::Left);
        sel.update(bottom, Side::Right);
        term.selection = Some(sel);
    }

    pub fn clear_selection(&self) {
        self.term.lock().selection = None;
    }

    /// The selected text, if any (empty selections count as none).
    pub fn selection_text(&self) -> Option<String> {
        self.term.lock().selection_to_string().filter(|s| !s.is_empty())
    }

    /// Clear the screen and the scrollback, like ⌘K in Terminal.app. The
    /// shell is asked to redraw its prompt (Ctrl+L).
    pub fn clear(&self) {
        {
            let mut term = self.term.lock();
            term.selection = None;
            term.grid_mut().clear_history();
        }
        self.write(&b"\x0c"[..]);
    }

    /// Scroll the view by `lines` (positive = towards older output).
    pub fn scroll(&self, lines: i32) {
        self.term.lock().scroll_display(alacritty_terminal::grid::Scroll::Delta(lines));
    }

    pub fn scroll_to_bottom(&self) {
        self.term.lock().scroll_display(alacritty_terminal::grid::Scroll::Bottom);
    }

    /// Copy the grid into a plain snapshot for rendering.
    pub fn snapshot(&self) -> Snapshot {
        let term = self.term.lock();
        let content = term.renderable_content();
        let cols = term.columns();
        let rows = term.screen_lines();
        let colors = content.colors;
        let selection = content.selection;
        let mut cells = vec![vec![CellSnapshot::blank(&self.palette); cols]; rows];

        for indexed in content.display_iter {
            let Point { line, column } = indexed.point;
            // display_iter yields lines relative to the top of the display.
            let row = (line.0 + content.display_offset as i32) as usize;
            if row >= rows || column.0 >= cols {
                continue;
            }
            let cell = &indexed.cell;
            let flags = cell.flags;
            let mut fg = self.palette.resolve(cell.fg, colors);
            let mut bg = self.palette.resolve(cell.bg, colors);
            if flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            if flags.contains(Flags::DIM) {
                fg = fg.map(|v| (v as u16 * 2 / 3) as u8);
            }
            cells[row][column.0] = CellSnapshot {
                c: if flags.contains(Flags::HIDDEN) { ' ' } else { cell.c },
                fg,
                bg,
                bold: flags.contains(Flags::BOLD),
                italic: flags.contains(Flags::ITALIC),
                underline: flags.intersects(Flags::ALL_UNDERLINES),
                inverse: flags.contains(Flags::INVERSE),
                wide_spacer: flags.contains(Flags::WIDE_CHAR_SPACER),
                selected: selection.is_some_and(|s| s.contains(indexed.point)),
            };
        }

        let cursor_visible = term.mode().contains(TermMode::SHOW_CURSOR) && content.cursor.shape != CursorShape::Hidden;
        let cursor = cursor_visible.then(|| {
            let p = content.cursor.point;
            (p.column.0, (p.line.0 + content.display_offset as i32).max(0) as usize)
        });

        Snapshot {
            cols,
            rows,
            cells,
            cursor,
            cursor_shape: cursor_visible.then_some(content.cursor.shape),
            display_offset: content.display_offset,
            title: self.title.clone(),
        }
    }

    /// All text currently on screen, for tests and "copy terminal" actions.
    pub fn screen_text(&self) -> String {
        let snap = self.snapshot();
        snap.cells
            .iter()
            .map(|row| row.iter().map(|c| c.c).collect::<String>().trim_end().to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl CellSnapshot {
    fn blank(p: &Palette) -> Self {
        Self {
            c: ' ',
            fg: p.foreground,
            bg: p.background,
            bold: false,
            italic: false,
            underline: false,
            inverse: false,
            wide_spacer: false,
            selected: false,
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        let _ = self.notifier.0.send(Msg::Shutdown);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg(unix)]
    fn echo_round_trip() {
        let Ok(mut t) = Terminal::new(40, 5, None, None) else {
            eprintln!("no pty available in this environment; skipping");
            return;
        };
        t.send_text("echo qell_ok\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            t.poll();
            let text = t.screen_text();
            // The echoed command appears first; the output line is the second occurrence.
            if text.matches("qell_ok").count() >= 2 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "shell never echoed: {text:?}");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    #[cfg(unix)]
    fn custom_shell_command() {
        let Ok(mut t) = Terminal::new(40, 5, None, Some("/bin/sh -i")) else { return };
        t.send_text("echo qell_sh_ok\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while t.screen_text().matches("qell_sh_ok").count() < 2 {
            t.poll();
            assert!(std::time::Instant::now() < deadline, "custom shell never echoed");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[test]
    #[cfg(unix)]
    fn select_and_copy_output() {
        let Ok(mut t) = Terminal::new(40, 6, None, Some("/bin/sh -i")) else { return };
        t.send_text("echo hello_qell_world\n");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while t.screen_text().matches("hello_qell_world").count() < 2 {
            t.poll();
            assert!(std::time::Instant::now() < deadline, "shell never echoed");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        // The output line: the second line holding the word.
        let text = t.screen_text();
        let row = text.lines().enumerate().filter(|(_, l)| l.contains("hello_qell_world")).nth(1).unwrap().0;
        let col = text.lines().nth(row).unwrap().find("qell").unwrap();
        // Double click on "qell": the whole word (underscores included).
        t.select_start(col, row, false, SelectionKind::Words);
        assert_eq!(t.selection_text().as_deref(), Some("hello_qell_world"));
        assert!(t.snapshot().cells[row][col].selected);
        // A drag over "qell".
        t.select_start(col, row, false, SelectionKind::Chars);
        t.select_update(col + 3, row, true);
        assert_eq!(t.selection_text().as_deref(), Some("qell"));
        t.clear_selection();
        assert_eq!(t.selection_text(), None);
        assert!(!t.snapshot().cells[row][col].selected);
    }

    #[test]
    fn colour_cube_and_grey_ramp() {
        let p = Palette::default();
        let colors = term::color::Colors::default();
        assert_eq!(p.resolve(Color::Indexed(16), &colors), [0, 0, 0]);
        assert_eq!(p.resolve(Color::Indexed(231), &colors), [255, 255, 255]);
        assert_eq!(p.resolve(Color::Indexed(232), &colors), [8, 8, 8]);
        assert_eq!(p.resolve(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), &colors), [1, 2, 3]);
    }
}
