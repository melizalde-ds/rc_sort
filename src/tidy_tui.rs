//! Inline front end for `bashrc-sort tidy`: a box drawn below the prompt (fits the content,
//! at most 60% of the terminal), blocks grouped by source file with a diff preview, and a
//! short summary left in scrollback when it closes. Same plan and same apply as the plain
//! prompts (`tidy::run_tidy`); only how you choose differs.

use crate::classify::Category;
use crate::state::Paths;
use crate::tidy::{apply, plan, Act, Item, Plan};
use anyhow::Result;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::terminal::ScrollUp;
use ratatui::crossterm::{cursor, execute, terminal};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use std::cell::{Cell, RefCell};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

/// Lines of the target file shown above the block in the preview.
const TAIL: usize = 3;
/// Columns a target badge takes (` FUNCTIONS `).
const BADGE_W: usize = 11;
/// Below this width the preview starts hidden (`p` shows it).
const WIDE: u16 = 90;
/// First words that get bold in the preview.
const SHELL_WORDS: [&str; 18] = [
    "export", "alias", "function", "eval", "source", ".", "if", "then", "elif", "else", "fi", "for", "while",
    "do", "done", "case", "esac", "unset",
];

/// Every symbol the box and the summary print, so a non-UTF-8 terminal gets ASCII.
#[derive(Clone, Copy)]
pub(crate) struct Glyphs {
    sure: &'static str,
    unsure: &'static str,
    blocked: &'static str,
    changed: &'static str,
    arrow: &'static str,
    dot: &'static str,
    select: &'static str,
    updown: &'static str,
    more: char,
    border: border::Set<'static>,
    title: &'static str,
    bar: &'static str,
    end: &'static str,
}

pub(crate) const UNICODE: Glyphs = Glyphs {
    sure: "✓",
    unsure: "?",
    blocked: "⊘",
    changed: "•",
    arrow: "→",
    dot: "·",
    select: "▸ ",
    updown: "↑↓",
    more: '…',
    border: border::ROUNDED,
    title: "◇",
    bar: "│",
    end: "└",
};

pub(crate) const ASCII: Glyphs = Glyphs {
    sure: "v",
    unsure: "?",
    blocked: "x",
    changed: "*",
    arrow: "->",
    dot: "-",
    select: "> ",
    updown: "up/down",
    more: '~',
    border: border::Set {
        top_left: "+",
        top_right: "+",
        bottom_left: "+",
        bottom_right: "+",
        vertical_left: "|",
        vertical_right: "|",
        horizontal_top: "-",
        horizontal_bottom: "-",
    },
    title: "*",
    bar: "|",
    end: "`",
};

/// The first of LC_ALL / LC_CTYPE / LANG that is set decides; none set means ASCII.
fn utf8_locale() -> bool {
    ["LC_ALL", "LC_CTYPE", "LANG"]
        .iter()
        .find_map(|v| std::env::var(v).ok().filter(|s| !s.is_empty()))
        .is_some_and(|v| {
            let v = v.to_ascii_lowercase();
            v.contains("utf-8") || v.contains("utf8")
        })
}


/// The `?` key list, with this terminal's symbols.
fn keys_help(g: &Glyphs) -> Vec<String> {
    vec![
        format!(" {}  PgUp PgDn  Home End  mouse   select     Tab / Shift-Tab   next / previous unsure", g.updown),
        " t e a f   move to tools / exports / aliases / functions".into(),
        " k  keep: stays here, never asked again   l  later: decide next time   r  back to the suggestion".into(),
        " p  show/hide preview     y  apply     q Esc  quit".into(),
        format!(" {} sure   {} unsure   {} can't move   {} changed by you", g.sure, g.unsure, g.blocked, g.changed),
    ]
}

/// True when both ends are a real terminal big enough to draw in; otherwise tidy stays plain.
pub fn usable() -> bool {
    std::io::stdin().is_terminal()
        && std::io::stdout().is_terminal()
        && std::env::var("TERM").map_or(true, |t| t != "dumb")
        && terminal::size().is_ok_and(|(w, h)| w >= 60 && h >= 12)
}

pub fn run(p: &Paths, target: Option<PathBuf>, out: &mut impl Write) -> Result<()> {
    // Notes from planning (files left alone, skipped files) print above the box and stay.
    let mut plan = plan(p, target, out)?;
    if plan.items.is_empty() {
        writeln!(out, "bashrc-sort: nothing to tidy")?;
        return Ok(());
    }
    out.flush()?;
    let mut app = App {
        g: if utf8_locale() { UNICODE } else { ASCII },
        color: std::env::var_os("NO_COLOR").is_none(),
        ..App::default()
    };
    let (_, rows) = terminal::size()?;
    let viewport = Viewport::Inline(height(&plan, rows));
    let mut term = ratatui::init_with_options(TerminalOptions { viewport });
    let _ = execute!(std::io::stdout(), EnableMouseCapture);
    let step: Result<Step> = loop {
        if let Err(e) = term.draw(|f| draw(f, &app, &plan)) {
            break Err(e.into());
        }
        let step = match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => app.handle(&mut plan, k),
            Ok(Event::Mouse(m)) => app.mouse(&plan, m),
            Ok(Event::Resize(_, rows)) => {
                // Terminals re-wrap the old box differently when the width changes (and may
                // push part of it into history), so there is no reliable spot to redraw it.
                // Scroll everything visible into history (nothing is erased) and start over
                // at the top of the window.
                let _ = execute!(std::io::stdout(), ScrollUp(rows), cursor::MoveTo(0, 0));
                let viewport = Viewport::Inline(height(&plan, rows));
                match Terminal::with_options(CrosstermBackend::new(std::io::stdout()), TerminalOptions { viewport }) {
                    Ok(t) => term = t,
                    Err(e) => break Err(e.into()),
                }
                Step::Stay
            }
            Ok(_) => Step::Stay,
            Err(e) => break Err(e.into()),
        };
        if step != Step::Stay {
            break Ok(step);
        }
    };
    // Erase the box; the summary is printed where it was. `draw` results only carry a
    // frame-relative area, so the box's row on screen comes from `get_frame`.
    let top = term.get_frame().area().y;
    let _ = execute!(std::io::stdout(), DisableMouseCapture);
    let _ = term.clear();
    let _ = term.set_cursor_position((0, top));
    let _ = term.show_cursor();
    let _ = term.backend_mut().flush();
    terminal::disable_raw_mode()?;
    drop(term);

    let backups = p.state_dir().join("backups");
    let backups = match backups.strip_prefix(&p.home) {
        Ok(rel) => format!("~/{}", rel.display()),
        Err(_) => backups.display().to_string(),
    };
    let skipped = match step? {
        Step::Apply => {
            let mut log = Vec::new();
            match apply(p, &plan, &mut log) {
                Ok(skipped) => Some(skipped),
                Err(e) => {
                    out.write_all(&log)?;
                    return Err(e);
                }
            }
        }
        _ => None,
    };
    let width = terminal::size().map_or(80, |(w, _)| w as usize);
    for line in summary(&plan, skipped.as_deref(), &backups, width, &app.g) {
        writeln!(out, "{}", paint(&line, &app.g, app.color))?;
    }
    Ok(())
}

/// Box height: header, two footer lines, borders and one row per file and block; at most 60% of the
/// terminal (but never under 10 lines), the list scrolls past that.
pub(crate) fn height(plan: &Plan, term_rows: u16) -> u16 {
    let rows = rows(plan).len() as u16;
    let want = rows.max(4) + 5;
    let cap = (term_rows * 6 / 10).max(10);
    want.min(cap).min(term_rows)
}

/// A list row: a source file heading, or one of its blocks (index into `plan.items`).
#[derive(Debug, PartialEq)]
pub(crate) enum Row {
    File(usize),
    Item(usize),
}

/// Items come out of `plan` grouped by source already; add a heading before each group.
pub(crate) fn rows(plan: &Plan) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut last = None;
    for (i, item) in plan.items.iter().enumerate() {
        if last != Some(item.src) {
            rows.push(Row::File(item.src));
            last = Some(item.src);
        }
        rows.push(Row::Item(i));
    }
    rows
}

#[derive(Debug, PartialEq)]
pub(crate) enum Step {
    Stay,
    Quit,
    Apply,
}

/// Screen state. Choices live in the plan itself (`items[..].act`).
pub(crate) struct App {
    /// Selected block (index into `plan.items`).
    pub(crate) sel: usize,
    /// The footer asks `Apply? … y/N`.
    pub(crate) confirm: bool,
    /// The key list is open; any key closes it.
    pub(crate) help: bool,
    /// One-off message shown in the footer.
    pub(crate) status: String,
    /// `p` pressed: show or hide the preview regardless of width.
    pub(crate) preview: Option<bool>,
    pub(crate) g: Glyphs,
    /// False under NO_COLOR: the box keeps bold/dim but drops colors.
    pub(crate) color: bool,
    /// Whether the last frame was wide enough for the preview by default.
    wide: Cell<bool>,
    /// The list's scroll position, kept across frames.
    list: RefCell<ListState>,
    /// Where the list rows were drawn (inside the border), for mouse clicks.
    list_area: Cell<Rect>,
}

impl Default for App {
    fn default() -> Self {
        App {
            sel: 0,
            confirm: false,
            help: false,
            status: String::new(),
            preview: None,
            g: UNICODE,
            color: true,
            wide: Cell::new(true),
            list: RefCell::new(ListState::default()),
            list_area: Cell::new(Rect::default()),
        }
    }
}

/// Still needs an answer: unsure and set to leave.
fn undecided(i: &Item) -> bool {
    i.note.is_some() && i.act == Act::Leave
}

impl App {
    fn show_preview(&self, wide: bool) -> bool {
        self.preview.unwrap_or(wide)
    }

    /// Selects the next (or previous) undecided block, wrapping around.
    fn jump(&mut self, plan: &Plan, forward: bool) {
        let n = plan.items.len();
        let found = (1..=n)
            .map(|d| if forward { (self.sel + d) % n } else { (self.sel + n - d % n) % n })
            .find(|&i| undecided(&plan.items[i]));
        match found {
            Some(i) => self.sel = i,
            None => self.status = "no unsure blocks left to decide".into(),
        }
    }

    pub(crate) fn handle(&mut self, plan: &mut Plan, key: KeyEvent) -> Step {
        self.status.clear();
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            return Step::Quit;
        }
        if self.help {
            self.help = false;
            return Step::Stay;
        }
        if self.confirm {
            self.confirm = false;
            return if key.code == KeyCode::Char('y') { Step::Apply } else { Step::Stay };
        }
        let last = plan.items.len().saturating_sub(1);
        match key.code {
            KeyCode::Up => self.sel = self.sel.saturating_sub(1),
            KeyCode::Down => self.sel = (self.sel + 1).min(last),
            KeyCode::PageUp => self.sel = self.sel.saturating_sub(10),
            KeyCode::PageDown => self.sel = (self.sel + 10).min(last),
            KeyCode::Home => self.sel = 0,
            KeyCode::End => self.sel = last,
            KeyCode::Tab => self.jump(plan, true),
            KeyCode::BackTab => self.jump(plan, false),
            KeyCode::Char('q') | KeyCode::Esc => return Step::Quit,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('p') => self.preview = Some(!self.show_preview(self.wide.get())),
            KeyCode::Char('y') => {
                if plan.has_changes() {
                    self.confirm = true;
                } else {
                    self.status = "nothing to apply: every block is set to later".into();
                }
            }
            KeyCode::Char('k') => plan.items[self.sel].act = Act::Keep,
            // `u` was the first key for later; it keeps working.
            KeyCode::Char('l') | KeyCode::Char('u') => plan.items[self.sel].act = Act::Leave,
            KeyCode::Char('r') => {
                let item = &mut plan.items[self.sel];
                item.act = item.suggested.clone();
            }
            KeyCode::Char(c) => {
                if let Some(cat) = Category::from_key(c) {
                    let item = &mut plan.items[self.sel];
                    if item.block.balanced {
                        item.act = Act::Move(cat);
                    } else {
                        self.status = "this block is half of an if/loop, so it can't be moved".into();
                    }
                }
            }
            _ => {}
        }
        Step::Stay
    }

    /// Wheel moves the selection; a click selects a block (or the first block of a file).
    pub(crate) fn mouse(&mut self, plan: &Plan, m: MouseEvent) -> Step {
        let last = plan.items.len().saturating_sub(1);
        match m.kind {
            MouseEventKind::ScrollUp => self.sel = self.sel.saturating_sub(1),
            MouseEventKind::ScrollDown => self.sel = (self.sel + 1).min(last),
            MouseEventKind::Down(MouseButton::Left) => {
                let a = self.list_area.get();
                let inside = (a.x..a.x + a.width).contains(&m.column) && (a.y..a.y + a.height).contains(&m.row);
                if !inside || self.confirm || self.help {
                    return Step::Stay;
                }
                let idx = self.list.borrow().offset() + (m.row - a.y) as usize;
                match rows(plan).get(idx) {
                    Some(Row::Item(i)) => self.sel = *i,
                    Some(Row::File(si)) => {
                        if let Some(i) = plan.items.iter().position(|it| it.src == *si) {
                            self.sel = i;
                        }
                    }
                    None => {}
                }
            }
            _ => {}
        }
        Step::Stay
    }
}

/// Badge text and (text, background) colors for a choice.
fn badge(act: &Act) -> (&'static str, Color, Color) {
    match act {
        Act::Move(Category::Tools) => ("TOOLS", Color::White, Color::Blue),
        Act::Move(Category::Exports) => ("EXPORTS", Color::White, Color::Magenta),
        Act::Move(Category::Aliases) => ("ALIASES", Color::Black, Color::Green),
        Act::Move(Category::Functions) => ("FUNCTIONS", Color::Black, Color::Cyan),
        Act::Keep => ("KEEP", Color::Black, Color::White),
        Act::Leave => ("LATER", Color::White, Color::DarkGray),
        Act::Dup(_) => ("REMOVE", Color::Black, Color::Yellow),
    }
}

/// A fixed-width badge (`BADGE_W` columns) so the targets line up. Without colors the
/// background can't mark it, so it gets brackets instead.
fn badge_span(act: &Act, color: bool) -> Span<'static> {
    let (text, fg, bg) = badge(act);
    if color {
        Span::styled(format!(" {text:<9} "), Style::new().fg(fg).bg(bg).bold())
    } else {
        Span::raw(format!("[{text:<9}]")).bold()
    }
}

/// The choice in words, for messages: `tools`, `later`, `keep`, `remove`.
fn word(act: &Act) -> String {
    badge(act).0.to_ascii_lowercase()
}

fn truncate(s: &str, w: usize, more: char) -> String {
    if s.chars().count() <= w {
        return format!("{s:<w$}");
    }
    let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
    t.push(more);
    t
}

fn file_name(shown: &str) -> &str {
    shown.rsplit('/').next().unwrap_or(shown)
}

/// `✓• name ………  TOOLS `, padded so the badges line up at the right edge.
fn item_line(item: &Item, width: usize, g: &Glyphs, color: bool) -> Line<'static> {
    let (icon, icon_style) = if !item.block.balanced {
        (g.blocked, Style::new().fg(Color::Red))
    } else if item.note.is_some() {
        (g.unsure, Style::new().fg(Color::Yellow))
    } else {
        (g.sure, Style::new().fg(Color::Green))
    };
    let changed = if item.act != item.suggested { g.changed } else { " " };
    let name_w = width.saturating_sub(BADGE_W + 4).max(4);
    Line::from(vec![
        Span::styled(icon, icon_style),
        Span::styled(changed, Style::new().fg(Color::Yellow)),
        Span::raw(format!(" {} ", truncate(&item.name, name_w, g.more))),
        badge_span(&item.act, color),
    ])
}

/// `10-mixed.sh  → deleted (empty)` / `· keeps 1 block · interactive only`.
fn heading(plan: &Plan, si: usize, g: &Glyphs) -> Line<'static> {
    let src = &plan.srcs[si];
    let (_, delete) = plan.leftover(si);
    let left = plan.remaining(si);
    let mut outcome = if delete {
        format!("  {} deleted (empty)", g.arrow)
    } else {
        format!("  {} keeps {left} block{}", g.dot, if left == 1 { "" } else { "s" })
    };
    let moving = plan.items.iter().any(|i| i.src == si && matches!(i.act, Act::Move(_)));
    if src.guarded() && moving {
        outcome.push_str(&format!(" {} interactive only", g.dot));
    }
    Line::from(vec![Span::raw(file_name(&src.shown).to_string()).bold(), Span::raw(outcome).dim()])
}

/// One shell line for the preview: comments dimmed, a leading keyword in bold.
fn shell_line(prefix: &str, line: &str, base: Style) -> Line<'static> {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return Line::styled(format!("{prefix}{line}"), base.dim());
    }
    let indent = &line[..line.len() - trimmed.len()];
    let word_end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    let (word, rest) = trimmed.split_at(word_end);
    if SHELL_WORDS.contains(&word) {
        Line::from(vec![
            Span::styled(format!("{prefix}{indent}"), base),
            Span::styled(word.to_string(), base.bold()),
            Span::styled(rest.to_string(), base),
        ])
    } else {
        Line::styled(format!("{prefix}{line}"), base)
    }
}

pub(crate) fn draw(f: &mut Frame, app: &App, plan: &Plan) {
    let g = &app.g;
    let area = f.area();
    let [head, body, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(area);
    app.wide.set(area.width >= WIDE);
    let rows = rows(plan);
    let bordered = || Block::bordered().border_set(g.border);

    let count = |pred: fn(&Item) -> bool| plan.items.iter().filter(|i| pred(i)).count();
    let moves = count(|i| matches!(i.act, Act::Move(_)));
    let drops = count(|i| matches!(i.act, Act::Dup(_)));
    let keeps = count(|i| i.act == Act::Keep);
    let open = count(undecided);
    let d = g.dot;
    let mut title = vec![
        Span::raw(" bashrc-sort tidy").bold(),
        Span::raw(format!(" {d} {moves} move {d} {keeps} keep {d} {drops} remove")).dim(),
    ];
    if open > 0 {
        title.push(Span::raw(format!(" {d} {open} unsure left")).fg(Color::Yellow));
    }
    f.render_widget(Paragraph::new(Line::from(title)), head);
    f.render_widget(Line::from("? keys ").dim().right_aligned(), head);

    let (left, right) = if app.show_preview(area.width >= WIDE) {
        let [l, r] = Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);
        (l, Some(r))
    } else {
        (body, None)
    };

    // Inside the border and after the 2-column highlight symbol.
    let width = left.width.saturating_sub(4) as usize;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| match r {
            Row::File(si) => ListItem::new(heading(plan, *si, g)),
            Row::Item(i) => ListItem::new(item_line(&plan.items[*i], width, g, app.color)),
        })
        .collect();
    let list = List::new(items)
        .block(bordered())
        .highlight_symbol(g.select)
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().bold());
    {
        let mut state = app.list.borrow_mut();
        state.select(rows.iter().position(|r| *r == Row::Item(app.sel)));
        f.render_stateful_widget(list, left, &mut state);
    }
    app.list_area.set(left.inner(Margin::new(1, 1)));

    if let (Some(right), Some(item)) = (right, plan.items.get(app.sel)) {
        let (top, bottom) = match &item.clash {
            Some(_) => {
                let [t, b] = Layout::vertical([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(right);
                (t, Some(b))
            }
            None => (right, None),
        };
        let (title, lines) = preview(plan, app.sel, g);
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(bordered().title(title)), top);
        if let (Some(b), Some((file, text))) = (bottom, &item.clash) {
            let lines: Vec<Line> = text.lines().map(|l| shell_line("  ", l, Style::new())).collect();
            let title = format!(" already in {file} {d} compare ");
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(bordered().title(title)), b);
        }
    }

    // Line 1: every choice key next to the word it sets, in its badge's color.
    let mut keys = vec![Span::raw(" ")];
    let choices = [
        ("t", Act::Move(Category::Tools)),
        ("e", Act::Move(Category::Exports)),
        ("a", Act::Move(Category::Aliases)),
        ("f", Act::Move(Category::Functions)),
        ("k", Act::Keep),
        ("l", Act::Leave),
    ];
    for (i, (k, act)) in choices.iter().enumerate() {
        let (_, _, bg) = badge(act);
        keys.push(Span::raw(*k).bold());
        keys.push(Span::raw(format!(" {}", word(act))).fg(bg));
        keys.push(Span::raw(if i == 3 { "   " } else { "  " }));
    }
    keys.extend([Span::raw("r").bold(), Span::raw(" reset").dim()]);
    // Line 2: the confirm question, a message, or how to move around.
    let second = if app.confirm {
        Line::from(vec![
            Span::raw(" Apply? ").bold().fg(Color::Yellow),
            Span::raw(format!("{moves} move {d} {keeps} keep {d} {drops} remove   ")),
            Span::raw("y/N").bold(),
        ])
    } else if !app.status.is_empty() {
        Line::from(format!(" {}", app.status)).fg(Color::Yellow)
    } else {
        let mut nav = vec![Span::raw(" ")];
        for (i, (k, what)) in
            [("Tab", "next unsure"), (g.updown, "select"), ("y", "apply"), ("q", "quit"), ("?", "all keys")].iter().enumerate()
        {
            if i > 0 {
                nav.push(Span::raw(format!(" {d} ")).dim());
            }
            nav.push(Span::raw(*k).bold());
            nav.push(Span::raw(format!(" {what}")).dim());
        }
        Line::from(nav)
    };
    f.render_widget(Paragraph::new(vec![Line::from(keys), second]), foot);

    if app.help {
        let help = keys_help(g);
        let h = (help.len() as u16 + 2).min(body.height);
        let area = Rect { height: h, ..body };
        let keys: Vec<Line> = help.into_iter().map(Line::from).collect();
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(keys).block(bordered().title(" keys · any key closes ")), area);
    }

    if !app.color {
        for cell in f.buffer_mut().content.iter_mut() {
            cell.set_fg(Color::Reset);
            cell.set_bg(Color::Reset);
        }
    }
}

/// Preview title and lines: the end of the target file dimmed, then the block as `+` lines.
fn preview(plan: &Plan, sel: usize, g: &Glyphs) -> (String, Vec<Line<'static>>) {
    let Some(item) = plan.items.get(sel) else { return (String::new(), Vec::new()) };
    let d = g.dot;
    let mut lines = Vec::new();
    if !item.block.balanced {
        lines.push(Line::from("half of an if/loop: it can't be moved").fg(Color::Red));
    }
    match item.note.as_deref() {
        Some("") => lines.push(Line::from("unsure: no rule matched").fg(Color::Yellow)),
        Some(why) => lines.push(Line::from(format!("unsure: {why}")).fg(Color::Yellow)),
        None => {}
    }
    if item.act != item.suggested {
        let was = word(&item.suggested);
        lines.push(Line::from(format!("changed by you (suggested: {was}; r resets)")).fg(Color::Yellow));
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let title = match &item.act {
        Act::Move(cat) => {
            let name = cat.file_name();
            let target = plan.files.iter().find(|(n, _)| n == name).map(|(_, t)| t.as_str());
            if let Some(t) = target {
                let tail: Vec<&str> = t.lines().collect();
                for l in &tail[tail.len().saturating_sub(TAIL)..] {
                    lines.push(shell_line("  ", l, Style::new().dim()));
                }
            }
            let green = Style::new().fg(Color::Green);
            lines.extend(item.text.lines().map(|l| shell_line("+ ", l, green)));
            format!(" {name} ({}) ", if target.is_some() { "end" } else { "new" })
        }
        Act::Dup(other) => {
            lines.extend(item.block.text.lines().map(|l| shell_line("= ", l, Style::new().dim())));
            format!(" remove {d} exact copy already in {other} ")
        }
        Act::Keep | Act::Leave => {
            lines.extend(item.block.text.lines().map(|l| shell_line("  ", l, Style::new())));
            let src = file_name(&plan.srcs[item.src].shown);
            if item.act == Act::Keep {
                format!(" keep {d} stays in {src}, never asked again ")
            } else {
                format!(" later {d} stays in {src}, asked again next time ")
            }
        }
    };
    (title, lines)
}

/// What stays in scrollback once the box is gone. `skipped` is `None` when nothing was
/// applied, else the sources `apply` skipped because they changed meanwhile.
pub(crate) fn summary(plan: &Plan, skipped: Option<&[usize]>, backups: &str, width: usize, g: &Glyphs) -> Vec<String> {
    let mut out = vec![format!("{} bashrc-sort tidy", g.title)];
    let Some(skipped) = skipped else {
        out.push(format!("{} nothing changed", g.end));
        return out;
    };
    let applied = |i: &&Item| !skipped.contains(&i.src);
    let moves: Vec<String> = plan
        .items
        .iter()
        .filter(applied)
        .filter_map(|i| match &i.act {
            Act::Move(cat) => Some(format!("{} {} {}", i.name, g.arrow, cat.file_name())),
            _ => None,
        })
        .collect();
    let drops: Vec<String> = plan
        .items
        .iter()
        .filter(applied)
        .filter_map(|i| match &i.act {
            Act::Dup(other) => Some(format!("{} (already in {other})", i.name)),
            _ => None,
        })
        .collect();
    // Keeps are saved even when their file was skipped.
    let keeps: Vec<String> = plan.items.iter().filter(|i| i.act == Act::Keep).map(|i| i.name.clone()).collect();
    wrap_into(&mut out, g, "", &moves, width);
    wrap_into(&mut out, g, "removed: ", &drops, width);
    wrap_into(&mut out, g, "kept: ", &keeps, width);
    for &si in skipped {
        out.push(format!(
            "{} skipped {}: it changed while you were choosing; run tidy again",
            g.bar, plan.srcs[si].shown
        ));
    }
    out.push(format!("{} done {} backups in {backups}", g.end, g.dot));
    out.push("  open shells keep the old setup until you run: source ~/.bashrc".into());
    out
}

/// `│ label a · b · c`, wrapped at `width` with continuation lines indented under the label.
fn wrap_into(out: &mut Vec<String>, g: &Glyphs, label: &str, parts: &[String], width: usize) {
    if parts.is_empty() {
        return;
    }
    let sep = format!(" {} ", g.dot);
    let indent = format!("{} {}", g.bar, " ".repeat(label.chars().count()));
    let mut line = format!("{} {label}", g.bar);
    let mut empty = true;
    for part in parts {
        let len = line.chars().count() + sep.chars().count() + part.chars().count();
        if !empty && len > width {
            out.push(std::mem::replace(&mut line, indent.clone()));
            empty = true;
        }
        if !empty {
            line.push_str(&sep);
        }
        line.push_str(part);
        empty = false;
    }
    out.push(line);
}

/// Colors the leading title/bar/end marker (green / dim), unless NO_COLOR is set.
fn paint(line: &str, g: &Glyphs, color: bool) -> String {
    if !color {
        return line.to_string();
    }
    if line == format!("{} nothing changed", g.end) {
        return format!("\x1b[2m{line}\x1b[0m");
    }
    for (mark, code) in [(g.title, "32"), (g.bar, "2"), (g.end, "2")] {
        if let Some(rest) = line.strip_prefix(mark) {
            return format!("\x1b[{code}m{mark}\x1b[0m{rest}");
        }
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;
    use std::fs;

    const FILE: &str = "alias g=git\n\nssh-agent -s > /dev/null\n";

    fn setup(name: &str) -> Paths {
        let home = std::env::temp_dir().join(format!("bashrc-sort-tui-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        fs::create_dir_all(home.join(".bashrc.d")).unwrap();
        fs::write(home.join(".bashrc.d/30-x.sh"), FILE).unwrap();
        Paths { home, skel: PathBuf::from("/nonexistent") }
    }

    fn key(c: KeyCode) -> KeyEvent {
        KeyEvent::new(c, KeyModifiers::NONE)
    }

    fn press(app: &mut App, plan: &mut Plan, keys: &[KeyCode]) -> Step {
        let mut last = Step::Stay;
        for k in keys {
            last = app.handle(plan, key(*k));
        }
        last
    }

    fn terminal(app: &App, plan: &Plan, w: u16, h: u16) -> Terminal<TestBackend> {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app, plan)).unwrap();
        term
    }

    fn screen(app: &App, plan: &Plan, w: u16, h: u16) -> String {
        terminal(app, plan, w, h).backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn starts_from_the_plan() {
        let p = setup("start");
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].act, Act::Move(Category::Aliases));
        assert_eq!(plan.items[0].suggested, plan.items[0].act);
        assert!(plan.items[0].note.is_none());
        assert_eq!(plan.items[1].act, Act::Leave);
        assert_eq!(plan.items[1].note.as_deref(), Some(""));
    }

    #[test]
    fn rows_group_blocks_under_their_file() {
        let p = setup("rows");
        fs::write(p.rc_dir().join("40-y.sh"), "export Y=1\n").unwrap();
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(rows(&plan), vec![Row::File(0), Row::Item(0), Row::Item(1), Row::File(1), Row::Item(2)]);
    }

    #[test]
    fn height_fits_content_up_to_60_percent() {
        let p = setup("height");
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(height(&plan, 50), 9, "3 rows → minimum box");
        let many = (0..60).map(|i| format!("alias a{i}=b\n\n")).collect::<String>();
        fs::write(p.rc_dir().join("30-x.sh"), many).unwrap();
        let plan = super::plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(height(&plan, 50), 30);
        assert_eq!(height(&plan, 12), 10, "never under 10 lines");
    }

    #[test]
    fn keys_change_any_block_and_r_resets() {
        let p = setup("keys");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        let mut app = App::default();
        press(&mut app, &mut plan, &[KeyCode::Char('e')]);
        assert_eq!(plan.items[0].act, Act::Move(Category::Exports), "sure blocks can be overridden");
        press(&mut app, &mut plan, &[KeyCode::Char('r')]);
        assert_eq!(plan.items[0].act, Act::Move(Category::Aliases), "r goes back to the suggestion");
        press(&mut app, &mut plan, &[KeyCode::Down, KeyCode::Char('t')]);
        assert_eq!(plan.items[1].act, Act::Move(Category::Tools));
        press(&mut app, &mut plan, &[KeyCode::Char('k')]);
        assert_eq!(plan.items[1].act, Act::Keep);
        press(&mut app, &mut plan, &[KeyCode::Char('u')]);
        assert_eq!(plan.items[1].act, Act::Leave);
        press(&mut app, &mut plan, &[KeyCode::Down, KeyCode::End]);
        assert_eq!(app.sel, 1, "selection stops at the last block");
        press(&mut app, &mut plan, &[KeyCode::Char('?'), KeyCode::Char('k')]);
        assert_eq!(plan.items[1].act, Act::Leave, "a key that closes the help does nothing else");
    }

    #[test]
    fn tab_jumps_between_undecided_blocks() {
        let p = setup("tab");
        fs::write(p.rc_dir().join("40-y.sh"), "export Y=1\n\nfoo --bar\n").unwrap();
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        // 0 alias (sure), 1 ssh-agent (unsure), 2 export (sure), 3 foo (unsure)
        let mut app = App::default();
        press(&mut app, &mut plan, &[KeyCode::Tab]);
        assert_eq!(app.sel, 1);
        press(&mut app, &mut plan, &[KeyCode::Tab]);
        assert_eq!(app.sel, 3);
        press(&mut app, &mut plan, &[KeyCode::Tab]);
        assert_eq!(app.sel, 1, "wraps around");
        press(&mut app, &mut plan, &[KeyCode::BackTab]);
        assert_eq!(app.sel, 3);
        press(&mut app, &mut plan, &[KeyCode::Char('k'), KeyCode::Tab]);
        assert_eq!(app.sel, 1, "decided blocks are skipped");
        press(&mut app, &mut plan, &[KeyCode::Char('t'), KeyCode::Tab]);
        assert!(app.status.contains("no unsure blocks left"));
    }

    #[test]
    fn half_an_if_is_never_moved() {
        let p = setup("half");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        plan.items[1].block.balanced = false;
        let mut app = App { sel: 1, ..App::default() };
        press(&mut app, &mut plan, &[KeyCode::Char('t')]);
        assert_eq!(plan.items[1].act, Act::Leave);
        assert!(app.status.contains("half of an if/loop"));
    }

    #[test]
    fn apply_needs_confirmation_and_quit_changes_nothing() {
        let p = setup("confirm");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        let mut app = App::default();
        assert_eq!(press(&mut app, &mut plan, &[KeyCode::Char('y'), KeyCode::Char('n')]), Step::Stay);
        assert!(!app.confirm, "any other key goes back");
        assert_eq!(press(&mut app, &mut plan, &[KeyCode::Char('q')]), Step::Quit);
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(app.handle(&mut plan, ctrl_c), Step::Quit);
        assert_eq!(press(&mut app, &mut plan, &[KeyCode::Char('y'), KeyCode::Char('y')]), Step::Apply);

        press(&mut app, &mut plan, &[KeyCode::Char('u')]);
        assert_eq!(press(&mut app, &mut plan, &[KeyCode::Char('y')]), Step::Stay);
        assert!(app.status.contains("nothing to apply"));
    }

    #[test]
    fn choices_apply_like_the_plain_prompts() {
        let p = setup("apply");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        let mut app = App::default();
        press(&mut app, &mut plan, &[KeyCode::Down, KeyCode::Char('k')]);
        assert!(apply(&p, &plan, &mut Vec::new()).unwrap().is_empty());
        let rc = p.rc_dir();
        assert_eq!(fs::read_to_string(rc.join("aliases.sh")).unwrap(), "alias g=git\n");
        // `without` leaves the blank line that followed a moved first block (plain mode too).
        assert_eq!(fs::read_to_string(rc.join("30-x.sh")).unwrap().trim(), "ssh-agent -s > /dev/null");
        assert!(crate::state::load_kept(&p).unwrap().contains(&crate::state::hash("ssh-agent -s > /dev/null")));
    }

    #[test]
    fn draws_grouped_list_preview_and_footer() {
        let p = setup("draw");
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        let mut app = App::default();
        let s = screen(&app, &plan, 110, 10);
        for want in ["1 move · 0 keep · 0 remove · 1 unsure left", "30-x.sh  · keeps 1 block", "▸ ✓  alias g=git",
                     " ALIASES   ", "?  ssh-agent", " LATER     ", "aliases.sh (new)", "+ alias g=git", "t tools  e exports  a aliases  f functions   k keep  l later  r reset", "Tab next unsure · ↑↓ select · y apply"] {
            assert!(s.contains(want), "missing {want:?} in {s}");
        }
        assert!(!screen(&app, &plan, 70, 10).contains("(new)"), "narrow: preview hidden");
        app.preview = Some(true);
        assert!(screen(&app, &plan, 70, 10).contains("(new)"), "p shows it anyway");
        app.confirm = true;
        assert!(screen(&app, &plan, 110, 10).contains("Apply? 1 move · 0 keep · 0 remove"));
        app.confirm = false;
        app.help = true;
        assert!(screen(&app, &plan, 110, 10).contains("keep: stays here, never asked again"));
    }

    #[test]
    fn headings_say_what_happens_to_the_file() {
        let p = setup("heading");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        let app = App { sel: 1, ..App::default() };
        plan.items[1].act = Act::Move(Category::Tools);
        let s = screen(&app, &plan, 110, 10);
        assert!(s.contains("30-x.sh  → deleted (empty)"), "{s}");
        assert!(s.contains("✓  alias") && s.contains("▸ ?• ssh-agent"), "changed rows are marked: {s}");
        assert!(s.contains("changed by you (suggested: later; r resets)"), "{s}");

        fs::write(p.rc_dir().join("30-x.sh"), "[[ $- == *i* ]] || return\nalias g=git\n").unwrap();
        let plan = super::plan(&p, None, &mut Vec::new()).unwrap();
        assert!(screen(&App::default(), &plan, 110, 10).contains("interactive only"));
    }

    #[test]
    fn same_tool_clash_shows_the_existing_block() {
        let p = setup("clash");
        fs::write(p.rc_dir().join("tools.sh"), "# rust\n. \"$HOME/.cargo/env\"\n").unwrap();
        fs::write(p.rc_dir().join("30-x.sh"), "export PATH=\"$HOME/.cargo/bin:$PATH\"\n").unwrap();
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(plan.items[0].clash, Some(("tools.sh".into(), "# rust\n. \"$HOME/.cargo/env\"".into())));
        let s = screen(&App::default(), &plan, 110, 14);
        assert!(s.contains("already in tools.sh · compare"), "{s}");
        assert!(s.contains(". \"$HOME/.cargo/env\""), "{s}");
    }

    #[test]
    fn no_color_and_ascii() {
        let p = setup("plainstyle");
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        let app = App { color: false, g: ASCII, ..App::default() };
        let term = terminal(&app, &plan, 110, 10);
        let buf = term.backend().buffer();
        assert!(buf.content().iter().all(|c| c.fg == Color::Reset && c.bg == Color::Reset));
        let s: String = buf.content().iter().map(|c| c.symbol()).collect();
        assert!(s.contains("> v  alias g=git") && s.contains("[ALIASES  ]") && s.contains("+---"), "{s}");
        assert!(s.is_ascii(), "{s}");
    }

    #[test]
    fn mouse_selects_rows_and_wheel_moves() {
        let p = setup("mouse");
        fs::write(p.rc_dir().join("40-y.sh"), "export Y=1\n").unwrap();
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        let mut app = App::default();
        terminal(&app, &plan, 110, 12);
        let click = |row| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row,
            modifiers: KeyModifiers::NONE,
        };
        // Row 0 header line, 1 top border, 2 "30-x.sh", 3 alias, 4 ssh-agent, 5 "40-y.sh", 6 export.
        app.mouse(&plan, click(4));
        assert_eq!(app.sel, 1);
        app.mouse(&plan, click(5));
        assert_eq!(app.sel, 2, "a file heading selects its first block");
        app.mouse(&plan, click(0));
        assert_eq!(app.sel, 2, "clicks outside the list do nothing");
        let wheel = |kind| MouseEvent { kind, column: 5, row: 3, modifiers: KeyModifiers::NONE };
        app.mouse(&plan, wheel(MouseEventKind::ScrollUp));
        assert_eq!(app.sel, 1);
    }

    #[test]
    fn shell_lines_dim_comments_and_bold_keywords() {
        let base = Style::new();
        assert_eq!(shell_line("  ", "# bun", base).style, base.dim());
        let l = shell_line("+ ", "  export A=1", base);
        assert_eq!(l.spans[1].content, "export");
        assert_eq!(l.spans[1].style, base.bold());
        assert_eq!(shell_line("", "foo export", base).spans.len(), 1);
    }

    #[test]
    fn summary_says_what_happened() {
        let p = setup("summary");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        plan.items[1].act = Act::Keep;
        assert_eq!(summary(&plan, None, "~/b", 80, &UNICODE), vec!["◇ bashrc-sort tidy", "└ nothing changed"]);
        let s = summary(&plan, Some(&[]), "~/b", 80, &UNICODE);
        assert_eq!(s[1], "│ alias g=git → aliases.sh");
        assert_eq!(s[2], "│ kept: ssh-agent -s > /dev/null");
        assert_eq!(s[3], "└ done · backups in ~/b");
        let s = summary(&plan, Some(&[0]), "~/b", 80, &UNICODE);
        assert!(!s.iter().any(|l| l.contains("→")), "skipped files moved nothing: {s:?}");
        assert!(s.iter().any(|l| l.starts_with("│ skipped ~/.bashrc.d/30-x.sh")));
        let s = summary(&plan, Some(&[]), "~/b", 80, &ASCII);
        assert!(s.iter().all(|l| l.is_ascii()), "{s:?}");
        assert_eq!(paint(&s[1], &ASCII, false), s[1]);
    }

    #[test]
    fn summary_wraps_long_lists() {
        let mut out = Vec::new();
        let parts: Vec<String> = (0..6).map(|i| format!("block{i} → tools.sh")).collect();
        wrap_into(&mut out, &UNICODE, "", &parts, 50);
        assert!(out.len() > 1 && out.iter().all(|l| l.chars().count() <= 50), "{out:?}");
        assert!(out[1].starts_with("│ block"));
    }
}
