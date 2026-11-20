//! Vim mode: modal editing (Normal / Insert / Visual) and the `:` command
//! line, driving a [`Buffer`].
//!
//! This module has no UI: the editor turns key presses into [`VimKey`]s,
//! feeds them to [`Vim::key`], and carries out the [`Action`]s that need the
//! rest of the app (saving, closing tabs…). In Insert mode the editor types
//! as usual; only Escape comes here.

use qell_core::{Buffer, Position};
use regex::Regex;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    #[default]
    Normal,
    Insert,
    Visual,
    VisualLine,
}

/// A key press, as Vim sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VimKey {
    Char(char),
    Esc,
    Enter,
    Backspace,
    Left,
    Right,
    Up,
    Down,
    Ctrl(char),
    /// Pasted (or composed) text: goes into the command line.
    Paste(String),
}

/// What the app must do for a command the buffer alone can't carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// `:w [name]`
    Save(Option<String>),
    /// `:q`, `:q!`, `:bd`
    Close {
        force: bool,
    },
    /// `:wq`, `:x`, `ZZ`
    SaveAndClose,
    /// `:wa`
    SaveAll,
    /// `:qa`, `:qa!`
    QuitAll {
        force: bool,
    },
    /// `:wqa`
    SaveAllAndQuit,
    /// `:e path`
    Edit(String),
    NextTab,
    PrevTab,
    GoToDefinition,
    GoBack,
    /// Yanked text, for the system clipboard.
    Copy(String),
    /// `:set wrap` / `:set nowrap`
    SetWrap(bool),
    /// `:term`
    Terminal,
    /// `:Ex`
    Explorer,
}

/// What the editor knows that commands need.
pub struct Ctx {
    pub tab_size: usize,
    /// Rows visible in the editor (for Ctrl-D / Ctrl-U).
    pub page: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    Delete,
    Change,
    Yank,
    Indent,
    Dedent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Exclusive,
    Inclusive,
    Linewise,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Motion {
    Left,
    Right,
    Down,
    Up,
    WordFwd(bool),
    WordBack(bool),
    WordEnd(bool),
    LineStart,
    FirstNonBlank,
    LineEnd,
    FileStart,
    FileEnd,
    ParaFwd,
    ParaBack,
    MatchBracket,
    Find { ch: char, forward: bool, till: bool },
    RepeatFind { reverse: bool },
    Search { reverse: bool },
    HalfPage { down: bool },
    NextLine,
    PrevLine,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    /// The command needs more keys.
    Wait,
    Done,
}

/// Ctrl-<c> as a private-use char, so a command is a plain string of chars.
fn ctrl(c: char) -> char {
    char::from_u32(0xE000 + c as u32).unwrap_or('\u{E000}')
}

struct Keys<'a> {
    k: &'a [char],
    i: usize,
}

impl Keys<'_> {
    fn next(&mut self) -> Option<char> {
        let c = self.k.get(self.i).copied();
        self.i += 1;
        c
    }

    /// A count: digits not starting with 0 (`0` is a motion).
    fn count(&mut self) -> Option<usize> {
        let start = self.i;
        while self.k.get(self.i).is_some_and(|c| c.is_ascii_digit() && (self.i > start || *c != '0')) {
            self.i += 1;
        }
        (self.i > start).then(|| self.k[start..self.i].iter().collect::<String>().parse().unwrap_or(1).min(100_000))
    }
}

#[derive(Default)]
pub struct Vim {
    pub mode: Mode,
    /// Keys of the command being typed (`2d`, `ci`…), shown in the bar.
    pending: Vec<char>,
    register: String,
    register_linewise: bool,
    /// The command line being typed, with its prefix (`:`, `/` or `?`).
    pub cmdline: Option<String>,
    history: Vec<String>,
    history_at: Option<usize>,
    /// Last message (`E486: Pattern not found`…) and whether it's an error.
    pub message: String,
    pub error: bool,
    search: Option<Regex>,
    search_forward: bool,
    highlight: bool,
    /// Visual mode: where it started and where the cursor is (inclusive).
    anchor: Position,
    vcur: Position,
    /// The buffer selection Visual mode last set, to notice outside changes.
    visual_sel: Option<(Position, Position)>,
    /// Lines of the last visual selection (`'<,'>`).
    visual_lines: (usize, usize),
    last_find: Option<(char, bool, bool)>,
    goal: Option<usize>,
    kept_goal: bool,
    /// `.`: keys of the last change and the text typed in Insert mode.
    last_change: Option<(Vec<char>, Option<String>)>,
    recording: Vec<char>,
    changed: bool,
    pending_change: Option<Vec<char>>,
    insert_start: Option<Position>,
    replaying: bool,
}

// ---- text helpers ---------------------------------------------------------------

fn first_non_blank(buf: &Buffer, line: usize) -> usize {
    buf.line(line).chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

/// Last line with text: a file ending in a newline has an empty line after it.
fn last_line(buf: &Buffer) -> usize {
    let n = buf.line_count();
    if n > 1 && buf.line_len(n - 1) == 0 {
        n - 2
    } else {
        n - 1
    }
}

fn char_at(buf: &Buffer, i: usize) -> Option<char> {
    (i < buf.rope().len_chars()).then(|| buf.rope().char(i))
}

fn range_text(buf: &Buffer, a: Position, b: Position) -> String {
    let (s, e) = (buf.pos_to_char(a), buf.pos_to_char(b));
    buf.rope().slice(s..e.max(s)).to_string()
}

#[derive(PartialEq, Clone, Copy)]
enum Class {
    Space,
    Word,
    Punct,
}

fn class(c: char, big: bool) -> Class {
    if c.is_whitespace() {
        Class::Space
    } else if big || c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punct
    }
}

fn word_fwd(buf: &Buffer, mut i: usize, big: bool) -> usize {
    let n = buf.rope().len_chars();
    let ch = |i: usize| buf.rope().char(i);
    if i >= n {
        return n;
    }
    let c0 = class(ch(i), big);
    if c0 != Class::Space {
        while i < n && class(ch(i), big) == c0 {
            i += 1;
        }
    }
    while i < n && class(ch(i), big) == Class::Space {
        // An empty line is a word of its own.
        if ch(i) == '\n' && i + 1 < n && ch(i + 1) == '\n' {
            return i + 1;
        }
        i += 1;
    }
    i
}

fn word_back(buf: &Buffer, mut i: usize, big: bool) -> usize {
    let ch = |i: usize| buf.rope().char(i);
    if i == 0 {
        return 0;
    }
    i -= 1;
    while i > 0 && class(ch(i), big) == Class::Space && !(ch(i) == '\n' && ch(i - 1) == '\n') {
        i -= 1;
    }
    let c = class(ch(i), big);
    if c == Class::Space {
        return i;
    }
    while i > 0 && class(ch(i - 1), big) == c {
        i -= 1;
    }
    i
}

fn word_end(buf: &Buffer, mut i: usize, big: bool) -> usize {
    let n = buf.rope().len_chars();
    let ch = |i: usize| buf.rope().char(i);
    i += 1;
    while i < n && class(ch(i), big) == Class::Space {
        i += 1;
    }
    if i >= n {
        return n.saturating_sub(1);
    }
    let c = class(ch(i), big);
    while i + 1 < n && class(ch(i + 1), big) == c {
        i += 1;
    }
    i
}

/// The bracket matching the one at (or after, on the line) the cursor.
fn match_bracket(buf: &Buffer, from: Position) -> Option<Position> {
    let line: Vec<char> = buf.line(from.line).chars().collect();
    let col = (from.col..line.len()).find(|&c| "()[]{}".contains(line[c]))?;
    let open = line[col];
    let (close, forward) = match open {
        '(' => (')', true),
        '[' => (']', true),
        '{' => ('}', true),
        ')' => ('(', false),
        ']' => ('[', false),
        _ => ('{', false),
    };
    let mut i = buf.pos_to_char(Position::new(from.line, col));
    let n = buf.rope().len_chars();
    let mut depth = 0i32;
    loop {
        let c = buf.rope().char(i);
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(buf.char_to_pos(i));
            }
        }
        if forward {
            i += 1;
            if i >= n {
                return None;
            }
        } else {
            if i == 0 {
                return None;
            }
            i -= 1;
        }
    }
}

/// A text object (`iw`, `a"`, `i(`…) around `at`: a char range `[start, end)`.
fn text_object(buf: &Buffer, at: Position, obj: char, around: bool) -> Option<(Position, Position)> {
    let line: Vec<char> = buf.line(at.line).chars().collect();
    match obj {
        'w' | 'W' => {
            if line.is_empty() {
                return None;
            }
            let big = obj == 'W';
            let col = at.col.min(line.len() - 1);
            let c = class(line[col], big);
            let mut s = col;
            while s > 0 && class(line[s - 1], big) == c {
                s -= 1;
            }
            let mut e = col + 1;
            while e < line.len() && class(line[e], big) == c {
                e += 1;
            }
            if around {
                // The spaces after it, else the ones before.
                let after = e;
                while e < line.len() && line[e].is_whitespace() {
                    e += 1;
                }
                if e == after {
                    while s > 0 && line[s - 1].is_whitespace() {
                        s -= 1;
                    }
                }
            }
            Some((Position::new(at.line, s), Position::new(at.line, e)))
        }
        '"' | '\'' | '`' => {
            let quotes: Vec<usize> = (0..line.len()).filter(|&i| line[i] == obj).collect();
            // The pair around the cursor, else the next one on the line.
            let pair = quotes.chunks(2).filter(|p| p.len() == 2).find(|p| at.col <= p[1])?;
            let (s, e) = if around { (pair[0], pair[1] + 1) } else { (pair[0] + 1, pair[1]) };
            Some((Position::new(at.line, s), Position::new(at.line, e)))
        }
        '(' | ')' | 'b' | '[' | ']' | '{' | '}' | 'B' | '<' | '>' => {
            let (open, close) = match obj {
                '(' | ')' | 'b' => ('(', ')'),
                '[' | ']' => ('[', ']'),
                '{' | '}' | 'B' => ('{', '}'),
                _ => ('<', '>'),
            };
            let n = buf.rope().len_chars();
            let here = buf.pos_to_char(at);
            // Back to the unmatched opener (the one under the cursor counts).
            let mut depth = 0i32;
            let mut i = here.min(n.checked_sub(1)?);
            let start = loop {
                let c = buf.rope().char(i);
                if c == close && i != here {
                    depth += 1;
                } else if c == open {
                    if depth == 0 {
                        break i;
                    }
                    depth -= 1;
                }
                if i == 0 {
                    return None;
                }
                i -= 1;
            };
            let mut depth = 0i32;
            let mut j = start;
            let end = loop {
                let c = buf.rope().char(j);
                if c == open {
                    depth += 1;
                } else if c == close {
                    depth -= 1;
                    if depth == 0 {
                        break j;
                    }
                }
                j += 1;
                if j >= n {
                    return None;
                }
            };
            let (s, e) = if around { (start, end + 1) } else { (start + 1, end) };
            Some((buf.char_to_pos(s), buf.char_to_pos(e)))
        }
        _ => None,
    }
}

/// Vim's replacement syntax (`\1`, `&`) as the regex crate's (`${1}`, `${0}`).
fn replacement(rep: &str) -> String {
    let mut out = String::new();
    let mut it = rep.chars();
    while let Some(c) = it.next() {
        match c {
            '\\' => match it.next() {
                Some(d) if d.is_ascii_digit() => out.push_str(&format!("${{{d}}}")),
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => out.push('\\'),
            },
            '&' => out.push_str("${0}"),
            '$' => out.push_str("$$"),
            c => out.push(c),
        }
    }
    out
}

/// A pattern as typed: a regex when it is one, else the literal text.
fn compile(pattern: &str, ignore_case: bool) -> Regex {
    let build = |p: &str| regex::RegexBuilder::new(p).case_insensitive(ignore_case).build();
    build(pattern).or_else(|_| build(&regex::escape(pattern))).expect("an escaped pattern is valid")
}

impl Vim {
    pub fn new() -> Self {
        Self { search_forward: true, ..Default::default() }
    }

    /// Whether Vim handles this key (otherwise the editor does).
    pub fn wants(&self, key: &VimKey) -> bool {
        if self.cmdline.is_some() {
            return true;
        }
        match self.mode {
            Mode::Insert => matches!(key, VimKey::Esc | VimKey::Ctrl('[') | VimKey::Ctrl('c')),
            _ => !matches!(key, VimKey::Paste(_)),
        }
    }

    /// Whether typing belongs to Vim (not Insert mode): the editor must not
    /// insert the keys it doesn't hand over.
    pub fn captures(&self) -> bool {
        self.cmdline.is_some() || self.mode != Mode::Insert
    }

    /// Where to draw the block cursor, outside Insert mode.
    pub fn block_cursor(&self, buf: &Buffer) -> Option<Position> {
        match self.mode {
            Mode::Insert => None,
            Mode::Normal => Some(buf.cursor.head()),
            _ => Some(self.vcur),
        }
    }

    /// The keys typed so far of an unfinished command (`2d`…).
    pub fn pending(&self) -> String {
        self.pending.iter().map(|c| if (*c as u32) >= 0xE000 && (*c as u32) < 0xE080 { '^' } else { *c }).collect()
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            Mode::Normal => "-- NORMAL --",
            Mode::Insert => "-- INSERT --",
            Mode::Visual => "-- VISUAL --",
            Mode::VisualLine => "-- VISUAL LINE --",
        }
    }

    pub fn set_message(&mut self, text: impl Into<String>, error: bool) {
        self.message = text.into();
        self.error = error;
    }

    /// Char ranges of the search matches on a line, while highlighted.
    pub fn matches_in(&self, line: &str) -> Vec<(usize, usize)> {
        let Some(re) = self.search.as_ref().filter(|_| self.highlight) else { return Vec::new() };
        re.find_iter(line)
            .filter(|m| !m.is_empty())
            .take(200)
            .map(|m| (line[..m.start()].chars().count(), line[..m.end()].chars().count()))
            .collect()
    }

    /// Back to Normal mode, whatever was going on.
    pub fn reset(&mut self, buf: &mut Buffer) {
        buf.end_undo_group();
        self.pending.clear();
        self.cmdline = None;
        self.pending_change = None;
        self.normal_mode(buf);
    }

    /// Follow what happened outside Vim (mouse clicks and drags, tab
    /// switches): a selection means Visual mode, none means Normal.
    pub fn sync(&mut self, buf: &mut Buffer) {
        let sel = (buf.cursor.selection.anchor, buf.cursor.selection.head);
        let empty = buf.cursor.selection.is_empty();
        match self.mode {
            Mode::Insert => {}
            Mode::Normal if !empty => {
                self.mode = Mode::Visual;
                self.anchor = sel.0;
                self.vcur = sel.1;
                if sel.1 > sel.0 {
                    self.vcur.col = self.vcur.col.saturating_sub(1);
                } else {
                    self.anchor.col = self.anchor.col.saturating_sub(1);
                }
                self.visual_sel = Some(sel);
            }
            Mode::Normal => self.clamp(buf),
            Mode::Visual | Mode::VisualLine => {
                if self.visual_sel != Some(sel) {
                    if empty {
                        self.mode = Mode::Normal;
                        self.clamp(buf);
                    } else {
                        self.mode = Mode::Normal;
                        self.sync(buf);
                    }
                }
            }
        }
    }

    /// In Normal mode the cursor sits on a character, never after the last.
    fn clamp(&self, buf: &mut Buffer) {
        let h = buf.cursor.head();
        let max = buf.line_len(h.line).saturating_sub(1);
        if h.col > max {
            buf.set_cursor(Position::new(h.line, max), false);
        }
    }

    fn normal_mode(&mut self, buf: &mut Buffer) {
        if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
            let (a, b) = (self.anchor.min(self.vcur), self.anchor.max(self.vcur));
            self.visual_lines = (a.line, b.line);
            buf.set_cursor(self.vcur, false);
        }
        self.mode = Mode::Normal;
        self.visual_sel = None;
        self.clamp(buf);
    }

    fn enter_insert(&mut self, buf: &mut Buffer) {
        buf.begin_undo_group();
        self.mode = Mode::Insert;
        self.visual_sel = None;
        self.insert_start = Some(buf.cursor.head());
    }

    fn exit_insert(&mut self, buf: &mut Buffer) {
        buf.end_undo_group();
        let head = buf.cursor.head();
        let typed = match self.insert_start.take() {
            Some(start) if head >= start => range_text(buf, start, head),
            _ => String::new(),
        };
        if let Some(keys) = self.pending_change.take() {
            if !self.replaying {
                self.last_change = Some((keys, Some(typed)));
            }
        }
        buf.set_cursor(Position::new(head.line, head.col.saturating_sub(1)), false);
        self.mode = Mode::Normal;
        self.clamp(buf);
    }

    // ---- keys ---------------------------------------------------------------------

    pub fn key(&mut self, buf: &mut Buffer, key: VimKey, ctx: &Ctx) -> Vec<Action> {
        let mut out = Vec::new();
        if self.cmdline.is_some() {
            self.cmdline_key(buf, key, ctx, &mut out);
            return out;
        }
        if self.mode == Mode::Insert {
            if matches!(key, VimKey::Esc | VimKey::Ctrl('[') | VimKey::Ctrl('c')) {
                self.exit_insert(buf);
            }
            return out;
        }
        self.message.clear();
        let c = match key {
            VimKey::Char(c) => c,
            VimKey::Esc | VimKey::Ctrl('[') | VimKey::Ctrl('c') => {
                self.pending.clear();
                self.recording.clear();
                self.normal_mode(buf);
                return out;
            }
            VimKey::Enter => '+',
            VimKey::Backspace | VimKey::Left => 'h',
            VimKey::Right => 'l',
            VimKey::Up => 'k',
            VimKey::Down => 'j',
            VimKey::Ctrl(c) => ctrl(c),
            VimKey::Paste(_) => return out,
        };
        self.pending.push(c);
        self.recording.push(c);
        let visual = self.mode != Mode::Normal;
        self.changed = false;
        self.kept_goal = false;
        let step = if visual { self.visual(buf, ctx, &mut out) } else { self.normal(buf, ctx, &mut out) };
        if step == Step::Done {
            self.pending.clear();
            let keys = std::mem::take(&mut self.recording);
            if self.changed && !visual {
                if self.mode == Mode::Insert {
                    self.pending_change = Some(keys);
                } else {
                    self.last_change = Some((keys, None));
                }
            }
            if !self.kept_goal {
                self.goal = None;
            }
            if matches!(self.mode, Mode::Visual | Mode::VisualLine) {
                self.show_visual(buf);
            }
        }
        out
    }

    fn parse_motion(&mut self, first: char, keys: &mut Keys) -> Option<Option<Motion>> {
        use Motion::*;
        Some(Some(match first {
            'h' => Left,
            'l' | ' ' => Right,
            'j' => Down,
            'k' => Up,
            'w' => WordFwd(false),
            'W' => WordFwd(true),
            'b' => WordBack(false),
            'B' => WordBack(true),
            'e' => WordEnd(false),
            'E' => WordEnd(true),
            '0' => LineStart,
            '^' => FirstNonBlank,
            '$' => LineEnd,
            'G' => FileEnd,
            '{' => ParaBack,
            '}' => ParaFwd,
            '%' => MatchBracket,
            '+' => NextLine,
            '-' => PrevLine,
            ';' => RepeatFind { reverse: false },
            ',' => RepeatFind { reverse: true },
            'n' => Search { reverse: false },
            'N' => Search { reverse: true },
            'f' | 't' | 'F' | 'T' => {
                let ch = keys.next()?;
                Find { ch, forward: first == 'f' || first == 't', till: first == 't' || first == 'T' }
            }
            'g' => match keys.next()? {
                'g' => FileStart,
                _ => return Some(None),
            },
            c if c == ctrl('d') => HalfPage { down: true },
            c if c == ctrl('u') => HalfPage { down: false },
            _ => return Some(None),
        }))
    }

    /// Where a motion leads from `from`.
    fn target(&mut self, buf: &Buffer, ctx: &Ctx, m: Motion, count: Option<usize>, from: Position) -> Option<(Position, Kind)> {
        use Kind::*;
        let n = count.unwrap_or(1);
        let lines = buf.line_count();
        let len = |l: usize| buf.line_len(l);
        let by_chars = |f: &dyn Fn(usize) -> usize| {
            let mut i = buf.pos_to_char(from);
            for _ in 0..n {
                i = f(i);
            }
            buf.char_to_pos(i)
        };
        let vertical = |this: &mut Self, line: usize| {
            let goal = this.goal.unwrap_or(from.col);
            this.goal = Some(goal);
            this.kept_goal = true;
            Position::new(line, goal.min(len(line)))
        };
        Some(match m {
            Motion::Left => (Position::new(from.line, from.col.saturating_sub(n)), Exclusive),
            Motion::Right => (Position::new(from.line, (from.col + n).min(len(from.line))), Exclusive),
            Motion::Down => (vertical(self, (from.line + n).min(last_line(buf).max(from.line))), Linewise),
            Motion::Up => (vertical(self, from.line.saturating_sub(n)), Linewise),
            Motion::WordFwd(big) => (by_chars(&|i| word_fwd(buf, i, big)), Exclusive),
            Motion::WordBack(big) => (by_chars(&|i| word_back(buf, i, big)), Exclusive),
            Motion::WordEnd(big) => (by_chars(&|i| word_end(buf, i, big)), Inclusive),
            Motion::LineStart => (Position::new(from.line, 0), Exclusive),
            Motion::FirstNonBlank => (Position::new(from.line, first_non_blank(buf, from.line)), Exclusive),
            Motion::LineEnd => {
                let l = (from.line + n - 1).min(lines - 1);
                (Position::new(l, len(l)), Exclusive)
            }
            Motion::FileStart | Motion::FileEnd => {
                let l = match (count, m) {
                    (Some(c), _) => c.saturating_sub(1).min(last_line(buf)),
                    (None, Motion::FileStart) => 0,
                    _ => last_line(buf),
                };
                (Position::new(l, first_non_blank(buf, l)), Linewise)
            }
            Motion::NextLine | Motion::PrevLine => {
                let l = if m == Motion::NextLine { (from.line + n).min(last_line(buf)) } else { from.line.saturating_sub(n) };
                (Position::new(l, first_non_blank(buf, l)), Linewise)
            }
            Motion::HalfPage { down } => {
                let d = (ctx.page / 2).max(1) * n;
                let l = if down { (from.line + d).min(last_line(buf)) } else { from.line.saturating_sub(d) };
                (vertical(self, l), Linewise)
            }
            Motion::ParaFwd | Motion::ParaBack => {
                let blank = |l: usize| buf.line(l).trim().is_empty();
                let mut l = from.line;
                for _ in 0..n {
                    if m == Motion::ParaFwd {
                        while l + 1 < lines && blank(l) {
                            l += 1;
                        }
                        while l + 1 < lines && !blank(l) {
                            l += 1;
                        }
                    } else {
                        while l > 0 && blank(l) {
                            l -= 1;
                        }
                        while l > 0 && !blank(l) {
                            l -= 1;
                        }
                    }
                }
                (Position::new(l, if blank(l) { 0 } else { len(l) }), Exclusive)
            }
            Motion::MatchBracket => (match_bracket(buf, from)?, Inclusive),
            Motion::Find { .. } | Motion::RepeatFind { .. } => {
                let (ch, forward, till) = match m {
                    Motion::Find { ch, forward, till } => {
                        self.last_find = Some((ch, forward, till));
                        (ch, forward, till)
                    }
                    Motion::RepeatFind { reverse } => {
                        let (ch, f, t) = self.last_find?;
                        (ch, f != reverse, t)
                    }
                    _ => unreachable!(),
                };
                let line: Vec<char> = buf.line(from.line).chars().collect();
                let mut col = from.col;
                for i in 0..n {
                    // `;` after `t` must not stay stuck before the same char.
                    let skip = usize::from(till && i == 0 && matches!(m, Motion::RepeatFind { .. }));
                    col = if forward {
                        (col + 1 + skip..line.len()).find(|&c| line[c] == ch)?
                    } else {
                        (0..col.saturating_sub(skip)).rev().find(|&c| line[c] == ch)?
                    };
                }
                if forward {
                    (Position::new(from.line, if till { col - 1 } else { col }), Inclusive)
                } else {
                    (Position::new(from.line, if till { col + 1 } else { col }), Exclusive)
                }
            }
            Motion::Search { reverse } => {
                let mut p = from;
                for _ in 0..n {
                    p = self.find_next(buf, p, self.search_forward != reverse)?;
                }
                (p, Exclusive)
            }
        })
    }

    /// The next match of the last search from `from`, wrapping around.
    fn find_next(&mut self, buf: &Buffer, from: Position, forward: bool) -> Option<Position> {
        let Some(re) = self.search.clone() else {
            self.set_message("E35: No previous regular expression", true);
            return None;
        };
        self.highlight = true;
        let text = buf.text();
        let rope = buf.rope();
        let here = rope.char_to_byte(buf.pos_to_char(from));
        let found = if forward {
            let after = rope.char_to_byte((buf.pos_to_char(from) + 1).min(rope.len_chars()));
            match re.find_at(&text, after) {
                Some(m) => Some(m.start()),
                None => {
                    self.set_message("search hit BOTTOM, continuing at TOP", false);
                    re.find(&text).map(|m| m.start())
                }
            }
        } else {
            match re.find_iter(&text).take_while(|m| m.start() < here).last() {
                Some(m) => Some(m.start()),
                None => {
                    self.set_message("search hit TOP, continuing at BOTTOM", false);
                    re.find_iter(&text).last().map(|m| m.start())
                }
            }
        };
        match found {
            Some(b) => Some(buf.char_to_pos(rope.byte_to_char(b))),
            None => {
                self.set_message(format!("E486: Pattern not found: {}", re.as_str()), true);
                None
            }
        }
    }

    // ---- operators ------------------------------------------------------------------

    fn yank(&mut self, text: String, linewise: bool, out: &mut Vec<Action>) {
        out.push(Action::Copy(text.clone()));
        self.register = text;
        self.register_linewise = linewise;
    }

    /// Apply an operator to whole lines `l1..=l2`.
    fn op_lines(&mut self, buf: &mut Buffer, ctx: &Ctx, op: Op, l1: usize, l2: usize, out: &mut Vec<Action>) {
        let l2 = l2.min(buf.line_count() - 1);
        let origin = buf.cursor.head();
        let text = (l1..=l2).map(|l| buf.line(l) + "\n").collect::<String>();
        match op {
            Op::Yank => {
                self.yank(text, true, out);
                let p = if origin.line == l1 { origin } else { Position::new(l1, first_non_blank(buf, l1)) };
                buf.set_cursor(p, false);
            }
            Op::Delete => {
                self.yank(text, true, out);
                let (s, e) = if l2 + 1 < buf.line_count() {
                    (Position::new(l1, 0), Position::new(l2 + 1, 0))
                } else if l1 > 0 {
                    (Position::new(l1 - 1, buf.line_len(l1 - 1)), Position::new(l2, buf.line_len(l2)))
                } else {
                    (Position::new(0, 0), Position::new(l2, buf.line_len(l2)))
                };
                buf.replace_range(s, e, "");
                let l = l1.min(last_line(buf));
                buf.set_cursor(Position::new(l, first_non_blank(buf, l)), false);
                self.changed = true;
            }
            Op::Change => {
                self.yank(text, true, out);
                let indent: String = buf.line(l1).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                buf.begin_undo_group();
                buf.replace_range(Position::new(l1, 0), Position::new(l2, buf.line_len(l2)), &indent);
                self.enter_insert(buf);
                self.changed = true;
            }
            Op::Indent | Op::Dedent => {
                buf.begin_undo_group();
                let width = ctx.tab_size.max(1);
                for l in l1..=l2 {
                    let line = buf.line(l);
                    if op == Op::Indent {
                        if !line.trim().is_empty() {
                            buf.replace_range(Position::new(l, 0), Position::new(l, 0), &" ".repeat(width));
                        }
                    } else {
                        let cut =
                            if line.starts_with('\t') { 1 } else { line.chars().take(width).take_while(|c| *c == ' ').count() };
                        if cut > 0 {
                            buf.replace_range(Position::new(l, 0), Position::new(l, cut), "");
                        }
                    }
                }
                buf.end_undo_group();
                buf.set_cursor(Position::new(l1, first_non_blank(buf, l1)), false);
                self.changed = true;
            }
        }
    }

    /// Apply an operator to the chars `[a, b)`.
    fn op_chars(&mut self, buf: &mut Buffer, ctx: &Ctx, op: Op, a: Position, b: Position, out: &mut Vec<Action>) {
        let (a, b) = (a.min(b), a.max(b));
        match op {
            Op::Indent | Op::Dedent => self.op_lines(buf, ctx, op, a.line, b.line, out),
            Op::Yank => {
                self.yank(range_text(buf, a, b), false, out);
                buf.set_cursor(a, false);
            }
            Op::Delete => {
                if a != b {
                    self.yank(range_text(buf, a, b), false, out);
                    buf.replace_range(a, b, "");
                }
                buf.set_cursor(a, false);
                self.changed = true;
            }
            Op::Change => {
                self.yank(range_text(buf, a, b), false, out);
                buf.begin_undo_group();
                if a != b {
                    buf.replace_range(a, b, "");
                }
                buf.set_cursor(a, false);
                self.enter_insert(buf);
                self.changed = true;
            }
        }
    }

    fn paste(&mut self, buf: &mut Buffer, before: bool, n: usize) {
        if self.register.is_empty() {
            return;
        }
        let text = self.register.repeat(n);
        let h = buf.cursor.head();
        if self.register_linewise {
            let line = if before { h.line } else { h.line + 1 };
            if line < buf.line_count() {
                let at = Position::new(line, 0);
                buf.replace_range(at, at, &text);
            } else {
                // After the last line, which has no newline of its own.
                let at = Position::new(h.line, buf.line_len(h.line));
                buf.replace_range(at, at, &format!("\n{}", text.trim_end_matches('\n')));
            }
            buf.set_cursor(Position::new(line, first_non_blank(buf, line)), false);
        } else {
            let col = if before || buf.line_len(h.line) == 0 { h.col } else { h.col + 1 };
            let at = Position::new(h.line, col);
            buf.replace_range(at, at, &text);
            let end = buf.cursor.head();
            buf.set_cursor(Position::new(end.line, end.col.saturating_sub(1)), false);
        }
        self.changed = true;
    }

    fn join(&mut self, buf: &mut Buffer, l1: usize, times: usize) {
        buf.begin_undo_group();
        for _ in 0..times {
            if l1 + 1 >= buf.line_count() {
                break;
            }
            let len = buf.line_len(l1);
            let next = buf.line(l1 + 1);
            let lead = next.chars().take_while(|c| c.is_whitespace()).count();
            let cur = buf.line(l1);
            let glue = if cur.is_empty() || cur.ends_with(' ') || next.trim().is_empty() || next.trim_start().starts_with(')') {
                ""
            } else {
                " "
            };
            buf.replace_range(Position::new(l1, len), Position::new(l1 + 1, lead), glue);
            buf.set_cursor(Position::new(l1, len), false);
        }
        buf.end_undo_group();
        self.changed = true;
    }

    fn open_cmdline(&mut self, prefix: &str) {
        self.cmdline = Some(prefix.to_string());
        self.history_at = None;
    }

    // ---- Normal mode --------------------------------------------------------------------

    fn normal(&mut self, buf: &mut Buffer, ctx: &Ctx, out: &mut Vec<Action>) -> Step {
        let pending = self.pending.clone();
        let mut keys = Keys { k: &pending, i: 0 };
        let count = keys.count();
        let Some(c) = keys.next() else { return Step::Wait };
        let n = count.unwrap_or(1);
        let h = buf.cursor.head();
        let len = buf.line_len(h.line);
        match c {
            'i' => self.insert_at(buf, h),
            'a' => self.insert_at(buf, Position::new(h.line, (h.col + 1).min(len))),
            'I' => self.insert_at(buf, Position::new(h.line, first_non_blank(buf, h.line))),
            'A' => self.insert_at(buf, Position::new(h.line, len)),
            'o' => {
                buf.begin_undo_group();
                buf.set_cursor(Position::new(h.line, len), false);
                buf.insert_newline();
                self.enter_insert(buf);
                self.changed = true;
            }
            'O' => {
                buf.begin_undo_group();
                let indent: String = buf.line(h.line).chars().take_while(|c| *c == ' ' || *c == '\t').collect();
                let at = Position::new(h.line, 0);
                buf.replace_range(at, at, &format!("{indent}\n"));
                buf.set_cursor(Position::new(h.line, indent.chars().count()), false);
                self.enter_insert(buf);
                self.changed = true;
            }
            'x' => self.op_chars(buf, ctx, Op::Delete, h, Position::new(h.line, (h.col + n).min(len)), out),
            'X' => self.op_chars(buf, ctx, Op::Delete, Position::new(h.line, h.col.saturating_sub(n)), h, out),
            'D' => self.op_chars(buf, ctx, Op::Delete, h, Position::new(h.line, len), out),
            'C' => self.op_chars(buf, ctx, Op::Change, h, Position::new(h.line, len), out),
            's' => self.op_chars(buf, ctx, Op::Change, h, Position::new(h.line, (h.col + n).min(len)), out),
            'S' => self.op_lines(buf, ctx, Op::Change, h.line, h.line + n - 1, out),
            'Y' => self.op_lines(buf, ctx, Op::Yank, h.line, h.line + n - 1, out),
            'J' => self.join(buf, h.line, n.saturating_sub(1).max(1)),
            '~' => {
                let end = (h.col + n).min(len);
                if end > h.col {
                    let text: String = range_text(buf, h, Position::new(h.line, end))
                        .chars()
                        .map(|c| {
                            if c.is_uppercase() {
                                c.to_lowercase().next().unwrap_or(c)
                            } else {
                                c.to_uppercase().next().unwrap_or(c)
                            }
                        })
                        .collect();
                    buf.replace_range(h, Position::new(h.line, end), &text);
                    buf.set_cursor(Position::new(h.line, end), false);
                    self.changed = true;
                }
            }
            'r' => {
                let Some(ch) = keys.next() else { return Step::Wait };
                if h.col + n <= len && ch != ctrl('[') {
                    buf.replace_range(h, Position::new(h.line, h.col + n), &ch.to_string().repeat(n));
                    buf.set_cursor(Position::new(h.line, h.col + n - 1), false);
                    self.changed = true;
                }
            }
            'p' => self.paste(buf, false, n),
            'P' => self.paste(buf, true, n),
            'u' => {
                for _ in 0..n {
                    if !buf.undo() {
                        self.set_message("Already at oldest change", false);
                        break;
                    }
                }
                let p = buf.cursor.head();
                buf.set_cursor(p, false);
            }
            c if c == ctrl('r') => {
                for _ in 0..n {
                    if !buf.redo() {
                        self.set_message("Already at newest change", false);
                        break;
                    }
                }
            }
            c if c == ctrl('o') => out.push(Action::GoBack),
            '.' => {
                if let Some((change, typed)) = self.last_change.clone() {
                    self.replaying = true;
                    for k in change {
                        self.pending.push(k);
                        // Skip the '.' itself, which is still at the front.
                        let all = std::mem::take(&mut self.pending);
                        self.pending = all[1..].to_vec();
                        let step = self.normal(buf, ctx, out);
                        if step == Step::Done {
                            self.pending.clear();
                        }
                        self.pending.insert(0, '.');
                    }
                    if self.mode == Mode::Insert {
                        if let Some(t) = typed.filter(|t| !t.is_empty()) {
                            buf.insert(&t);
                        }
                        self.exit_insert(buf);
                    }
                    self.replaying = false;
                    self.changed = false;
                }
            }
            'v' | 'V' => {
                self.mode = if c == 'v' { Mode::Visual } else { Mode::VisualLine };
                self.anchor = h;
                self.vcur = h;
            }
            ':' => self.open_cmdline(":"),
            '/' => self.open_cmdline("/"),
            '?' => self.open_cmdline("?"),
            '*' | '#' => {
                let line = buf.line(h.line);
                if let Some((s, e)) = text_object(buf, h, 'w', false).filter(|_| !line.trim().is_empty()) {
                    let word = range_text(buf, s, e);
                    let pat = if word.chars().all(|c| c.is_alphanumeric() || c == '_') {
                        format!(r"\b{}\b", regex::escape(&word))
                    } else {
                        regex::escape(&word)
                    };
                    self.search = Some(compile(&pat, false));
                    self.search_forward = c == '*';
                    if let Some(p) = self.find_next(buf, s, c == '*') {
                        buf.set_cursor(p, false);
                    }
                }
            }
            'Z' => match keys.next() {
                None => return Step::Wait,
                Some('Z') => out.push(Action::SaveAndClose),
                Some('Q') => out.push(Action::Close { force: true }),
                _ => {}
            },
            'z' => {
                if keys.next().is_none() {
                    return Step::Wait;
                }
            }
            'g' if pending.get(keys.i).is_some_and(|k| "dtT".contains(*k)) => match keys.next() {
                Some('d') => out.push(Action::GoToDefinition),
                Some('t') => out.push(Action::NextTab),
                _ => out.push(Action::PrevTab),
            },
            'd' | 'c' | 'y' | '>' | '<' => {
                let op = match c {
                    'd' => Op::Delete,
                    'c' => Op::Change,
                    'y' => Op::Yank,
                    '>' => Op::Indent,
                    _ => Op::Dedent,
                };
                let count2 = keys.count();
                let Some(m) = keys.next() else { return Step::Wait };
                let total = (count.is_some() || count2.is_some()).then(|| n * count2.unwrap_or(1));
                if m == c {
                    self.op_lines(buf, ctx, op, h.line, h.line + total.unwrap_or(1) - 1, out);
                } else if m == 'i' || m == 'a' {
                    let Some(obj) = keys.next() else { return Step::Wait };
                    if let Some((s, e)) = text_object(buf, h, obj, m == 'a') {
                        self.op_chars(buf, ctx, op, s, e, out);
                    }
                } else {
                    let motion = match self.parse_motion(m, &mut keys) {
                        None => return Step::Wait,
                        Some(None) => return Step::Done,
                        Some(Some(motion)) => motion,
                    };
                    // `cw` on a word changes to its end, not the spaces after.
                    let on_word = char_at(buf, buf.pos_to_char(h)).is_some_and(|c| !c.is_whitespace());
                    let motion = match motion {
                        Motion::WordFwd(big) if op == Op::Change && on_word => Motion::WordEnd(big),
                        other => other,
                    };
                    let Some((to, kind)) = self.target(buf, ctx, motion, total, h) else { return Step::Done };
                    match kind {
                        Kind::Linewise => self.op_lines(buf, ctx, op, h.line.min(to.line), h.line.max(to.line), out),
                        Kind::Inclusive => {
                            let (a, b) = (h.min(to), h.max(to));
                            let b = Position::new(b.line, (b.col + 1).min(buf.line_len(b.line)));
                            self.op_chars(buf, ctx, op, a, b, out);
                        }
                        Kind::Exclusive => {
                            let mut to = to;
                            // `dw` on a line's last word stops at the line's end.
                            if matches!(motion, Motion::WordFwd(_)) && to.line > h.line && to.col <= first_non_blank(buf, to.line)
                            {
                                to = Position::new(to.line - 1, buf.line_len(to.line - 1));
                                if to < h {
                                    to = h;
                                }
                            }
                            self.op_chars(buf, ctx, op, h, to, out);
                        }
                    }
                }
                if self.mode == Mode::Normal {
                    self.clamp(buf);
                }
            }
            first => {
                let motion = match self.parse_motion(first, &mut keys) {
                    None => return Step::Wait,
                    Some(None) => return Step::Done,
                    Some(Some(motion)) => motion,
                };
                if let Some((to, _)) = self.target(buf, ctx, motion, count, h) {
                    buf.set_cursor(to, false);
                }
                self.clamp(buf);
            }
        }
        Step::Done
    }

    fn insert_at(&mut self, buf: &mut Buffer, at: Position) {
        buf.set_cursor(at, false);
        self.enter_insert(buf);
        self.changed = true;
    }

    // ---- Visual mode ---------------------------------------------------------------------

    /// Put the Visual selection on the buffer (so it is drawn and copied).
    fn show_visual(&mut self, buf: &mut Buffer) {
        let (a, b) = (self.anchor.min(self.vcur), self.anchor.max(self.vcur));
        let (s, e) = if self.mode == Mode::VisualLine {
            (Position::new(a.line, 0), Position::new(b.line, buf.line_len(b.line)))
        } else {
            (a, Position::new(b.line, (b.col + 1).min(buf.line_len(b.line))))
        };
        if self.vcur >= self.anchor {
            buf.set_cursor(s, false);
            buf.set_cursor(e, true);
        } else {
            buf.set_cursor(e, false);
            buf.set_cursor(s, true);
        }
        self.visual_sel = Some((buf.cursor.selection.anchor, buf.cursor.selection.head));
    }

    fn visual(&mut self, buf: &mut Buffer, ctx: &Ctx, out: &mut Vec<Action>) -> Step {
        let pending = self.pending.clone();
        let mut keys = Keys { k: &pending, i: 0 };
        let count = keys.count();
        let Some(c) = keys.next() else { return Step::Wait };
        let linewise = self.mode == Mode::VisualLine;
        let (a, b) = (self.anchor.min(self.vcur), self.anchor.max(self.vcur));
        let op = match c {
            'd' | 'x' | 'X' | 'D' => Some(Op::Delete),
            'y' | 'Y' => Some(Op::Yank),
            'c' | 's' | 'S' => Some(Op::Change),
            '>' => Some(Op::Indent),
            '<' => Some(Op::Dedent),
            _ => None,
        };
        if let Some(op) = op {
            self.normal_mode(buf);
            buf.set_cursor(a, false);
            if linewise || c.is_uppercase() {
                self.op_lines(buf, ctx, op, a.line, b.line, out);
            } else {
                let end = Position::new(b.line, (b.col + 1).min(buf.line_len(b.line)));
                self.op_chars(buf, ctx, op, a, end, out);
            }
            if self.mode == Mode::Normal {
                self.clamp(buf);
            }
            return Step::Done;
        }
        match c {
            'v' | 'V' => {
                let want = if c == 'v' { Mode::Visual } else { Mode::VisualLine };
                if self.mode == want {
                    self.normal_mode(buf);
                } else {
                    self.mode = want;
                }
            }
            'o' => std::mem::swap(&mut self.anchor, &mut self.vcur),
            'p' | 'P' => {
                let (text, was_linewise) = (self.register.clone(), self.register_linewise);
                self.normal_mode(buf);
                buf.begin_undo_group();
                buf.set_cursor(a, false);
                if linewise {
                    buf.replace_range(
                        Position::new(a.line, 0),
                        Position::new(b.line, buf.line_len(b.line)),
                        text.trim_end_matches('\n'),
                    );
                } else {
                    let end = Position::new(b.line, (b.col + 1).min(buf.line_len(b.line)));
                    buf.replace_range(a, end, if was_linewise { text.trim_end_matches('\n') } else { &text });
                }
                buf.end_undo_group();
                self.clamp(buf);
            }
            'J' => {
                self.normal_mode(buf);
                self.join(buf, a.line, (b.line - a.line).max(1));
            }
            ':' => {
                self.normal_mode(buf);
                self.open_cmdline(":'<,'>");
            }
            'i' | 'a' => {
                let Some(obj) = keys.next() else { return Step::Wait };
                if let Some((s, e)) = text_object(buf, self.vcur, obj, c == 'a') {
                    if e > s {
                        self.anchor = s;
                        self.vcur = buf.char_to_pos(buf.pos_to_char(e) - 1);
                    }
                }
            }
            first => {
                let motion = match self.parse_motion(first, &mut keys) {
                    None => return Step::Wait,
                    Some(None) => return Step::Done,
                    Some(Some(motion)) => motion,
                };
                if let Some((to, _)) = self.target(buf, ctx, motion, count, self.vcur) {
                    let max = buf.line_len(to.line).saturating_sub(1);
                    self.vcur = Position::new(to.line, to.col.min(max));
                }
            }
        }
        Step::Done
    }

    // ---- the command line -------------------------------------------------------------------

    fn cmdline_key(&mut self, buf: &mut Buffer, key: VimKey, ctx: &Ctx, out: &mut Vec<Action>) {
        let Some(line) = self.cmdline.as_mut() else { return };
        match key {
            VimKey::Char(c) => line.push(c),
            VimKey::Paste(t) => line.push_str(t.lines().next().unwrap_or("")),
            VimKey::Backspace => {
                line.pop();
                if line.is_empty() {
                    self.cmdline = None;
                }
            }
            VimKey::Esc | VimKey::Ctrl('[') | VimKey::Ctrl('c') => self.cmdline = None,
            VimKey::Up | VimKey::Down => {
                // Earlier / later commands of the same kind.
                let prefix = line.chars().next().unwrap_or(':');
                let same: Vec<&String> = self.history.iter().filter(|h| h.starts_with(prefix)).collect();
                if same.is_empty() {
                    return;
                }
                let at = match (self.history_at, &key) {
                    (None, VimKey::Up) => same.len() - 1,
                    (Some(i), VimKey::Up) => i.saturating_sub(1),
                    (Some(i), _) if i + 1 < same.len() => i + 1,
                    (at, _) => at.unwrap_or(same.len() - 1),
                };
                self.history_at = Some(at);
                *line = same[at].clone();
            }
            VimKey::Enter => {
                let line = self.cmdline.take().unwrap_or_default();
                if line.chars().count() > 1 {
                    self.history.retain(|h| *h != line);
                    self.history.push(line.clone());
                }
                self.message.clear();
                let mut chars = line.chars();
                match chars.next() {
                    Some(':') => self.ex(buf, ctx, chars.as_str().trim(), out),
                    Some(p) => {
                        let pat = chars.as_str();
                        if !pat.is_empty() {
                            self.search = Some(compile(pat, false));
                        }
                        self.search_forward = p == '/';
                        let from = buf.cursor.head();
                        if let Some(to) = self.find_next(buf, from, self.search_forward) {
                            buf.set_cursor(to, false);
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
    }

    /// One line address: `12`, `.`, `$`, `'<`, `'>`.
    fn address(&self, buf: &Buffer, s: &str) -> Option<(usize, usize)> {
        let digits = s.chars().take_while(char::is_ascii_digit).count();
        if digits > 0 {
            let n: usize = s[..digits].parse().ok()?;
            return Some((n.saturating_sub(1).min(buf.line_count() - 1), digits));
        }
        match s.chars().next()? {
            '.' => Some((buf.cursor.head().line, 1)),
            '$' => Some((last_line(buf), 1)),
            '\'' if s.starts_with("'<") => Some((self.visual_lines.0, 2)),
            '\'' if s.starts_with("'>") => Some((self.visual_lines.1.min(buf.line_count() - 1), 2)),
            _ => None,
        }
    }

    /// Run a `:` command.
    fn ex(&mut self, buf: &mut Buffer, ctx: &Ctx, line: &str, out: &mut Vec<Action>) {
        // Range: `%`, `a`, or `a,b`.
        let mut rest = line;
        let mut range: Option<(usize, usize)> = None;
        if let Some(r) = rest.strip_prefix('%') {
            range = Some((0, last_line(buf)));
            rest = r;
        } else if let Some((a, used)) = self.address(buf, rest) {
            rest = &rest[used..];
            range = Some((a, a));
            if let Some(r) = rest.strip_prefix(',') {
                if let Some((b, used)) = self.address(buf, r) {
                    range = Some((a.min(b), a.max(b)));
                    rest = &r[used..];
                }
            }
        }
        let rest = rest.trim_start();
        if rest.is_empty() {
            // `:42`: go to the line.
            if let Some((_, l)) = range {
                buf.set_cursor(Position::new(l, first_non_blank(buf, l)), false);
            }
            return;
        }
        // `:s/a/b/` has no space between the name and its argument.
        let name_len = rest.chars().take_while(|c| c.is_ascii_alphabetic()).count();
        let (name, mut arg) = rest.split_at(name_len);
        let bang = arg.starts_with('!');
        if bang {
            arg = &arg[1..];
        }
        let arg = arg.trim();
        let here = buf.cursor.head().line;
        match name {
            "w" | "write" => out.push(Action::Save((!arg.is_empty()).then(|| arg.to_string()))),
            "q" | "quit" | "bd" | "bdelete" | "clo" | "close" => out.push(Action::Close { force: bang }),
            "wq" | "x" | "xit" => out.push(Action::SaveAndClose),
            "wa" | "wall" => out.push(Action::SaveAll),
            "qa" | "qall" => out.push(Action::QuitAll { force: bang }),
            "wqa" | "wqall" | "xa" | "xall" => out.push(Action::SaveAllAndQuit),
            "e" | "edit" if !arg.is_empty() => out.push(Action::Edit(arg.to_string())),
            "bn" | "bnext" | "tabn" | "tabnext" => out.push(Action::NextTab),
            "bp" | "bprevious" | "bN" | "tabp" | "tabprevious" => out.push(Action::PrevTab),
            "noh" | "nohlsearch" => self.highlight = false,
            "term" | "terminal" => out.push(Action::Terminal),
            "Ex" | "Explore" | "Lex" | "Lexplore" => out.push(Action::Explorer),
            "u" | "undo" => {
                buf.undo();
            }
            "red" | "redo" => {
                buf.redo();
            }
            "d" | "delete" => {
                let (l1, l2) = range.unwrap_or((here, here));
                self.op_lines(buf, ctx, Op::Delete, l1, l2, out);
            }
            "y" | "yank" => {
                let (l1, l2) = range.unwrap_or((here, here));
                self.op_lines(buf, ctx, Op::Yank, l1, l2, out);
            }
            "set" | "se" => match arg {
                "wrap" => out.push(Action::SetWrap(true)),
                "nowrap" => out.push(Action::SetWrap(false)),
                "hls" | "hlsearch" => self.highlight = true,
                "nohls" | "nohlsearch" => self.highlight = false,
                other => self.set_message(format!("E518: Unknown option: {other}"), true),
            },
            "s" | "substitute" => {
                let (l1, l2) = range.unwrap_or((here, here));
                self.substitute(buf, arg, l1, l2);
            }
            _ => self.set_message(format!("E492: Not an editor command: {line}"), true),
        }
    }

    /// `:s/pattern/replacement/flags` over lines `l1..=l2`.
    fn substitute(&mut self, buf: &mut Buffer, arg: &str, l1: usize, l2: usize) {
        let mut chars = arg.chars();
        let Some(delim) = chars.next().filter(|c| !c.is_alphanumeric() && !c.is_whitespace()) else {
            self.set_message("E486: Usage: :s/pattern/replacement/[g][i]", true);
            return;
        };
        // Split on the delimiter, honouring `\<delim>`.
        let mut parts = vec![String::new()];
        let mut escaped = false;
        for c in chars {
            if escaped {
                if c != delim {
                    parts.last_mut().expect("never empty").push('\\');
                }
                parts.last_mut().expect("never empty").push(c);
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == delim && parts.len() < 3 {
                parts.push(String::new());
            } else {
                parts.last_mut().expect("never empty").push(c);
            }
        }
        let pat = &parts[0];
        let rep = replacement(parts.get(1).map_or("", String::as_str));
        let flags = parts.get(2).map_or("", String::as_str);
        let re = if pat.is_empty() {
            match &self.search {
                Some(re) => re.clone(),
                None => return self.set_message("E35: No previous regular expression", true),
            }
        } else {
            compile(pat, flags.contains('i'))
        };
        let limit = if flags.contains('g') { 0 } else { 1 };
        let (mut subs, mut changed_lines, mut last) = (0usize, 0usize, None);
        buf.begin_undo_group();
        for l in l1..=l2.min(buf.line_count() - 1) {
            let line = buf.line(l);
            let hits = if limit == 1 { usize::from(re.is_match(&line)) } else { re.find_iter(&line).count() };
            if hits == 0 {
                continue;
            }
            let new = re.replacen(&line, limit, rep.as_str());
            if new != line {
                buf.replace_range(Position::new(l, 0), Position::new(l, line.chars().count()), &new);
                subs += hits;
                changed_lines += 1;
                last = Some(l);
            }
        }
        buf.end_undo_group();
        self.search = Some(re.clone());
        match last {
            Some(l) => {
                buf.set_cursor(Position::new(l, first_non_blank(buf, l)), false);
                self.highlight = true;
                if changed_lines > 1 {
                    self.set_message(format!("{subs} substitutions on {changed_lines} lines"), false);
                }
            }
            None => self.set_message(format!("E486: Pattern not found: {}", re.as_str()), true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CTX: Ctx = Ctx { tab_size: 4, page: 20 };

    /// Type `keys` (`<Esc>`, `<CR>`, `<C-r>` for special keys). In Insert
    /// mode, characters go to the buffer as the editor would type them.
    fn feed(vim: &mut Vim, buf: &mut Buffer, keys: &str) -> Vec<Action> {
        let mut out = Vec::new();
        let mut rest = keys;
        while let Some(c) = rest.chars().next() {
            let (key, used) = if rest.starts_with("<Esc>") {
                (VimKey::Esc, 5)
            } else if rest.starts_with("<CR>") {
                (VimKey::Enter, 4)
            } else if rest.starts_with("<C-") {
                (VimKey::Ctrl(rest.chars().nth(3).unwrap()), 5)
            } else {
                (VimKey::Char(c), c.len_utf8())
            };
            rest = &rest[used..];
            vim.sync(buf);
            if vim.wants(&key) {
                out.extend(vim.key(buf, key, &CTX));
            } else if let VimKey::Char(c) = key {
                buf.insert(&c.to_string());
            }
        }
        out
    }

    fn run(text: &str, keys: &str) -> (String, (usize, usize)) {
        let mut buf = Buffer::from_text(text, None);
        let mut vim = Vim::new();
        feed(&mut vim, &mut buf, keys);
        let h = vim.block_cursor(&buf).unwrap_or(buf.cursor.head());
        (buf.text(), (h.line, h.col))
    }

    #[test]
    fn motions() {
        let t = "let foo = bar(1, 2);\n    second line\n\nlast";
        assert_eq!(run(t, "w").1, (0, 4));
        assert_eq!(run(t, "3w").1, (0, 10));
        assert_eq!(run(t, "e").1, (0, 2));
        assert_eq!(run(t, "$").1, (0, 19));
        assert_eq!(run(t, "$b").1, (0, 18));
        assert_eq!(run(t, "j^").1, (1, 4));
        assert_eq!(run(t, "G").1, (3, 0));
        assert_eq!(run(t, "Ggg").1, (0, 0));
        assert_eq!(run(t, "2G").1, (1, 4));
        assert_eq!(run(t, "f(").1, (0, 13));
        assert_eq!(run(t, "t(").1, (0, 12));
        assert_eq!(run(t, "f(%").1, (0, 18));
        assert_eq!(run(t, "}").1, (2, 0));
        assert_eq!(run(t, "$j").1, (1, 14)); // keeps the column where it can
        assert_eq!(run(t, "llllllllllllllllllllllllll").1, (0, 19)); // never past the last char
    }

    #[test]
    fn delete_change_yank() {
        assert_eq!(run("one two three", "dw").0, "two three");
        assert_eq!(run("one two three", "d2w").0, "three");
        assert_eq!(run("one two three", "2dw").0, "three");
        assert_eq!(run("one two three", "wD").0, "one ");
        assert_eq!(run("one two\nthree", "wdw").0, "one \nthree"); // dw keeps the newline
        assert_eq!(run("a\nb\nc\nd", "jdd"), ("a\nc\nd".into(), (1, 0)));
        assert_eq!(run("a\nb\nc\nd", "2dd").0, "c\nd");
        assert_eq!(run("a\nb\nc", "Gdd"), ("a\nb".into(), (1, 0)));
        assert_eq!(run("a\nb\nc\nd", "dj").0, "c\nd");
        assert_eq!(run("one two three", "wcwTWO<Esc>"), ("one TWO three".into(), (0, 6)));
        assert_eq!(run("foo(bar, baz)", "fbci(x<Esc>").0, "foo(x)");
        assert_eq!(run("say \"hello world\" now", "ci\"hi<Esc>").0, "say \"hi\" now");
        assert_eq!(run("say \"hello\" now", "fhda\"").0, "say  now");
        assert_eq!(run("one two three", "wdiw").0, "one  three");
        assert_eq!(run("one two three", "wdaw").0, "one three");
        assert_eq!(run("    indented\nx", "ccnew<Esc>").0, "    new\nx");
        assert_eq!(run("abc", "x").0, "bc");
        assert_eq!(run("abcdef", "3x").0, "def");
        assert_eq!(run("abc", "lrZ").0, "aZc");
        assert_eq!(run("abc", "~~").0, "ABc");
    }

    #[test]
    fn put_and_join() {
        assert_eq!(run("a\nb", "yyp"), ("a\na\nb".into(), (1, 0)));
        assert_eq!(run("a\nb", "jyyP").0, "a\nb\nb");
        assert_eq!(run("a\nb", "jyyp").0, "a\nb\nb"); // after the last line
        assert_eq!(run("a\nb", "ddp").0, "b\na");
        assert_eq!(run("ab", "xp").0, "ba");
        assert_eq!(run("one two", "ywwP").0, "one one two");
        assert_eq!(run("one\n  two\nthree", "J"), ("one two\nthree".into(), (0, 3)));
        assert_eq!(run("a\nb\nc", "3J").0, "a b c");
    }

    #[test]
    fn insert_commands_and_undo() {
        assert_eq!(run("ab", "ix<Esc>"), ("xab".into(), (0, 0)));
        assert_eq!(run("ab", "ax<Esc>").0, "axb");
        assert_eq!(run("ab", "Ax<Esc>").0, "abx");
        assert_eq!(run("  ab", "$Ix<Esc>").0, "  xab");
        assert_eq!(run("  a\nb", "onew<Esc>").0, "  a\n  new\nb");
        assert_eq!(run("a\n  b", "jOnew<Esc>").0, "a\n  new\n  b");
        // A whole insert is one undo step, whatever was typed.
        assert_eq!(run("ab", "ihello big world <Esc>u").0, "ab");
        assert_eq!(run("one two", "cwxx yy<Esc>u").0, "one two");
        assert_eq!(run("a\nb", "dd<C-r>").0, "b");
        assert_eq!(run("a\nb", "ddu<C-r>").0, "b");
    }

    #[test]
    fn dot_repeats_the_last_change() {
        assert_eq!(run("a b c d", "dw..").0, "d");
        assert_eq!(run("a\nb\nc\nd", "ddj.").0, "b\nd");
        assert_eq!(run("one two three", "cwX<Esc>w.w.").0, "X X X");
        assert_eq!(run("a\nb", "A;<Esc>j.").0, "a;\nb;");
        assert_eq!(run("abcd", "x..").0, "d");
        assert_eq!(run("a\nb", ">>j.").0, "    a\n    b");
    }

    #[test]
    fn visual_mode() {
        assert_eq!(run("one two three", "wved").0, "one  three");
        assert_eq!(run("one two three", "wviwd").0, "one  three");
        assert_eq!(run("a\nb\nc\nd", "jVjd").0, "a\nd");
        assert_eq!(run("a\nb\nc", "Vjyjp").0, "a\nb\na\nb\nc"); // y leaves the cursor at the start
        assert_eq!(run("a\nb", "Vj>").0, "    a\n    b");
        assert_eq!(run("one two", "vlly$p").0, "one twoone");
        assert_eq!(run("one two three", "wvecX<Esc>").0, "one X three");
        assert_eq!(run("abc def", "vlo").1, (0, 0)); // `o` goes to the other end
                                                     // Escape leaves the cursor where it was.
        assert_eq!(run("abc def", "vll<Esc>"), ("abc def".into(), (0, 2)));
        let mut buf = Buffer::from_text("abc", None);
        let mut vim = Vim::new();
        feed(&mut vim, &mut buf, "vl");
        assert_eq!(vim.mode, Mode::Visual);
        assert_eq!(buf.selected_text(), "ab");
    }

    #[test]
    fn indent() {
        assert_eq!(run("a\nb\nc", ">j").0, "    a\n    b\nc");
        assert_eq!(run("        a", "<<").0, "    a");
        assert_eq!(run("  a", "<<").0, "a");
    }

    #[test]
    fn search() {
        let t = "foo bar\nbaz foo\nfoo end";
        assert_eq!(run(t, "/foo<CR>").1, (1, 4));
        assert_eq!(run(t, "/foo<CR>n").1, (2, 0));
        assert_eq!(run(t, "/foo<CR>nn").1, (0, 0)); // wraps
        assert_eq!(run(t, "/foo<CR>N").1, (0, 0));
        assert_eq!(run(t, "?foo<CR>").1, (2, 0));
        assert_eq!(run(t, "*").1, (1, 4));
        assert_eq!(run(t, "/ba[rz]<CR>n").1, (1, 0)); // a regex
        assert_eq!(run(t, "/a(b<CR>").1, (0, 0)); // not a regex: literal, not found
        assert_eq!(run(t, "/foo<CR>dn").0, "foo bar\nbaz foo end");
        let mut buf = Buffer::from_text(t, None);
        let mut vim = Vim::new();
        feed(&mut vim, &mut buf, "/nope<CR>");
        assert!(vim.error && vim.message.starts_with("E486"));
        feed(&mut vim, &mut buf, "/foo<CR>");
        assert_eq!(vim.matches_in("a foo foo"), vec![(2, 5), (6, 9)]);
        feed(&mut vim, &mut buf, ":noh<CR>");
        assert!(vim.matches_in("a foo foo").is_empty());
    }

    #[test]
    fn ex_commands() {
        let t = "foo foo\nbar foo\nend";
        assert_eq!(run(t, ":s/foo/X/<CR>").0, "X foo\nbar foo\nend");
        assert_eq!(run(t, ":s/foo/X/g<CR>").0, "X X\nbar foo\nend");
        assert_eq!(run(t, ":%s/foo/X/g<CR>").0, "X X\nbar X\nend");
        assert_eq!(run(t, ":%s/foo/X/g<CR>u").0, t); // one undo step
        assert_eq!(run(t, ":2,3s/o+/0/<CR>").0, "foo foo\nbar f0\nend");
        assert_eq!(run(t, r":%s/(\w+) foo/\1-&/<CR>").0, "foo-foo foo\nbar-bar foo\nend");
        assert_eq!(run(t, "Vj:s/foo/X/<CR>").0, "X foo\nbar X\nend"); // '<,'>
        assert_eq!(run(t, ":3<CR>").1, (2, 0));
        assert_eq!(run(t, ":$<CR>").1, (2, 0));
        assert_eq!(run(t, ":2d<CR>").0, "foo foo\nend");
        assert_eq!(run(t, ":2,3d<CR>").0, "foo foo");

        let mut buf = Buffer::from_text(t, None);
        let mut vim = Vim::new();
        assert_eq!(feed(&mut vim, &mut buf, ":w<CR>"), vec![Action::Save(None)]);
        assert_eq!(feed(&mut vim, &mut buf, ":w out.txt<CR>"), vec![Action::Save(Some("out.txt".into()))]);
        assert_eq!(feed(&mut vim, &mut buf, ":q!<CR>"), vec![Action::Close { force: true }]);
        assert_eq!(feed(&mut vim, &mut buf, ":wq<CR>"), vec![Action::SaveAndClose]);
        assert_eq!(feed(&mut vim, &mut buf, ":e src/main.rs<CR>"), vec![Action::Edit("src/main.rs".into())]);
        assert_eq!(feed(&mut vim, &mut buf, ":set nowrap<CR>"), vec![Action::SetWrap(false)]);
        assert_eq!(feed(&mut vim, &mut buf, "gd"), vec![Action::GoToDefinition]);
        assert_eq!(feed(&mut vim, &mut buf, "ZZ"), vec![Action::SaveAndClose]);
        feed(&mut vim, &mut buf, ":frobnicate<CR>");
        assert!(vim.error && vim.message.starts_with("E492"));
        // Escape and Backspace leave the command line.
        feed(&mut vim, &mut buf, ":w<Esc>");
        assert!(vim.cmdline.is_none());
        // Yanks go to the clipboard too.
        assert_eq!(feed(&mut vim, &mut buf, "ggyw"), vec![Action::Copy("foo ".into())]);
    }
}
