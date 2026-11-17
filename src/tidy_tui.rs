//! Inline front end for `rc-sort tidy`: a box drawn below the prompt (fits the content,
//! at most 60% of the terminal), blocks grouped by source file with a diff preview, and a
//! short summary left in scrollback when it closes. Same plan and same apply as the plain
//! prompts (`tidy::run_tidy`); only how you choose differs.

use crate::classify::Category;
use crate::state::Paths;
use crate::tidy::{apply, plan, Act, Item, Plan};
use anyhow::Result;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::crossterm::terminal::ScrollUp;
use ratatui::crossterm::{cursor, execute, terminal};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, Terminal, TerminalOptions, Viewport};
use std::cell::{Cell, RefCell};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

/// Lines of the target file shown above the block in the preview.
const TAIL: usize = 3;
/// Columns a target badge takes (` ● functions `).
const BADGE_W: usize = 13;
/// Cells in the header's progress bar.
const BAR: usize = 8;
/// Below this width the preview starts hidden (`p` shows it).
const WIDE: u16 = 90;
/// First words that get bold in the preview.
const SHELL_WORDS: [&str; 18] = [
    "export", "alias", "function", "eval", "source", ".", "if", "then", "elif", "else", "fi", "for", "while",
    "do", "done", "case", "esac", "unset",
];

/// How much color the terminal gets.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Tier {
    /// The 16 named colors: the terminal's theme picks the shades (also under NO_COLOR).
    Ansi,
    /// 24-bit colors for a dark background.
    Dark,
    /// 24-bit colors for a light background.
    Light,
}

/// The palette, by meaning: "usable at 16 colors, nicer at true color". On 16 colors only
/// named colors and dim, no backgrounds; on true color a soft violet accent plus a surface
/// tint for the selected row, badges and key caps. No black/white text in any tier, and
/// every color comes with an icon or a word.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Theme {
    pub(crate) tier: Tier,
}

fn hex(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

impl Theme {
    /// `RC_SORT_THEME=ansi|dark|light` wins; otherwise `COLORTERM=truecolor|24bit` gets
    /// the dark palette (most terminals; asking the terminal for its background is
    /// unreliable over tmux and ssh) and anything else the 16 colors.
    pub(crate) fn detect() -> Theme {
        let tier = match std::env::var("RC_SORT_THEME").as_deref() {
            Ok("ansi") => Tier::Ansi,
            Ok("dark") => Tier::Dark,
            Ok("light") => Tier::Light,
            _ if std::env::var("COLORTERM").is_ok_and(|v| v == "truecolor" || v == "24bit") => Tier::Dark,
            _ => Tier::Ansi,
        };
        Theme { tier }
    }

    fn pick(&self, dark: u32, light: u32, ansi: Color) -> Color {
        match self.tier {
            Tier::Ansi => ansi,
            Tier::Dark => hex(dark),
            Tier::Light => hex(light),
        }
    }

    /// Title, selection bar, category badges, key caps.
    pub(crate) fn accent(&self) -> Style {
        Style::new().fg(self.pick(0xa78bfa, 0x7c3aed, Color::Cyan))
    }
    /// ✓ sure, `+` lines, a finished progress bar.
    pub(crate) fn good(&self) -> Style {
        Style::new().fg(self.pick(0x86efac, 0x15803d, Color::Green))
    }
    /// ? unsure, • changed, messages, `Apply?`.
    pub(crate) fn attention(&self) -> Style {
        Style::new().fg(self.pick(0xfcd34d, 0xb45309, Color::Yellow))
    }
    /// ⊘ can't move, remove, a file that gets deleted.
    pub(crate) fn danger(&self) -> Style {
        Style::new().fg(self.pick(0xfca5a5, 0xb91c1c, Color::Red))
    }
    /// Hints, outcomes, rules, the target file's tail, later.
    pub(crate) fn muted(&self) -> Style {
        match self.tier {
            Tier::Ansi => Style::new().dim(),
            _ => Style::new().fg(self.pick(0x8b8fa3, 0x6b7280, Color::Reset)),
        }
    }
    /// The tint behind the selected row, badges and key caps; none on 16 colors.
    pub(crate) fn surface(&self) -> Option<Color> {
        match self.tier {
            Tier::Ansi => None,
            Tier::Dark => Some(hex(0x2a2d3e)),
            Tier::Light => Some(hex(0xe6e6f0)),
        }
    }
    fn on_surface(&self, style: Style) -> Style {
        match self.surface() {
            Some(bg) => style.bg(bg),
            None => style,
        }
    }
}

/// Every symbol the box and the summary print, so a non-UTF-8 terminal gets ASCII.
#[derive(Clone, Copy)]
pub(crate) struct Glyphs {
    sure: &'static str,
    unsure: &'static str,
    blocked: &'static str,
    changed: &'static str,
    arrow: &'static str,
    dot: &'static str,
    /// Left of the selected row.
    bar: &'static str,
    updown: &'static str,
    more: char,
    border: border::Set<'static>,
    hline: &'static str,
    vline: &'static str,
    tee_top: &'static str,
    tee_bottom: &'static str,
    logo: &'static str,
    full: &'static str,
    empty: &'static str,
    /// Badge icons: category, keep, later, remove.
    cat: &'static str,
    keep: &'static str,
    later: &'static str,
    remove: &'static str,
    title: &'static str,
    pipe: &'static str,
    end: &'static str,
}

pub(crate) const UNICODE: Glyphs = Glyphs {
    sure: "✓",
    unsure: "?",
    blocked: "⊘",
    changed: "•",
    arrow: "→",
    dot: "·",
    bar: "▌",
    updown: "↑↓",
    more: '…',
    border: border::ROUNDED,
    hline: "─",
    vline: "│",
    tee_top: "┬",
    tee_bottom: "┴",
    logo: "◆",
    full: "▰",
    empty: "▱",
    cat: "●",
    keep: "○",
    later: "◐",
    remove: "✕",
    title: "◇",
    pipe: "│",
    end: "└",
};

pub(crate) const ASCII: Glyphs = Glyphs {
    sure: "v",
    unsure: "?",
    blocked: "x",
    changed: "*",
    arrow: "->",
    dot: "-",
    bar: ">",
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
    hline: "-",
    vline: "|",
    tee_top: "+",
    tee_bottom: "+",
    logo: "*",
    full: "#",
    empty: "-",
    cat: "*",
    keep: "o",
    later: "~",
    remove: "x",
    title: "*",
    pipe: "|",
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
        writeln!(out, "rc-sort: nothing to tidy")?;
        return Ok(());
    }
    out.flush()?;
    let color = std::env::var_os("NO_COLOR").is_none();
    let mut app = App {
        g: if utf8_locale() { UNICODE } else { ASCII },
        theme: if color { Theme::detect() } else { Theme { tier: Tier::Ansi } },
        color,
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

/// Box height: header, two footer lines, borders and one row per file, block and gap
/// between files; at most 60% of the terminal (but never under 10 lines), the list
/// scrolls past that.
pub(crate) fn height(plan: &Plan, term_rows: u16) -> u16 {
    let rows = rows(plan).len() as u16;
    let want = rows.max(4) + 5;
    let cap = (term_rows * 6 / 10).max(10);
    want.min(cap).min(term_rows)
}

/// A list row: a blank line between files, a file heading, or one of its blocks.
#[derive(Debug, PartialEq)]
pub(crate) enum Row {
    Gap,
    File(usize),
    Item(usize),
}

/// Items come out of `plan` grouped by source already; add a heading before each group.
pub(crate) fn rows(plan: &Plan) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut last = None;
    for (i, item) in plan.items.iter().enumerate() {
        if last != Some(item.src) {
            if last.is_some() {
                rows.push(Row::Gap);
            }
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
    pub(crate) theme: Theme,
    /// False under NO_COLOR: the box keeps bold/dim but drops colors.
    pub(crate) color: bool,
    /// Whether the last frame was wide enough for the preview by default.
    wide: Cell<bool>,
    /// The list's scroll position, kept across frames.
    list: RefCell<ListState>,
    /// Where the list rows were drawn, for mouse clicks.
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
            theme: Theme { tier: Tier::Ansi },
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
                    Some(Row::Gap) | None => {}
                }
            }
            _ => {}
        }
        Step::Stay
    }
}

/// Badge icon, word and style: categories share the accent; color only marks status.
fn badge(act: &Act, g: &Glyphs, t: &Theme) -> (&'static str, String, Style) {
    match act {
        Act::Move(_) => (g.cat, word(act), t.accent().bold()),
        Act::Keep => (g.keep, word(act), Style::new().bold()),
        Act::Leave => (g.later, word(act), t.muted()),
        Act::Dup(_) => (g.remove, word(act), t.danger().bold()),
    }
}

/// A fixed-width badge (`BADGE_W` columns, ` ● functions `) on the surface tint, so the
/// targets line up.
fn badge_span(act: &Act, g: &Glyphs, t: &Theme) -> Span<'static> {
    let (icon, word, style) = badge(act, g, t);
    Span::styled(format!(" {icon} {word:<9} "), t.on_surface(style))
}

/// The choice in words: `tools`, `exports`, `aliases`, `functions`, `keep`, `later`, `remove`.
fn word(act: &Act) -> String {
    match act {
        Act::Move(cat) => cat.file_name().trim_end_matches(".sh").to_string(),
        Act::Keep => "keep".into(),
        Act::Leave => "later".into(),
        Act::Dup(_) => "remove".into(),
    }
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

/// `▌ ✓• name ……  ● tools     `: selection bar, status icon, changed mark, name, badge.
fn item_line(item: &Item, selected: bool, width: usize, g: &Glyphs, t: &Theme) -> Line<'static> {
    let (icon, icon_style) = if !item.block.balanced {
        (g.blocked, t.danger())
    } else if item.note.is_some() {
        (g.unsure, t.attention())
    } else {
        (g.sure, t.good())
    };
    let changed = if item.act != item.suggested { g.changed } else { " " };
    let name_w = width.saturating_sub(BADGE_W + 6).max(4);
    let name_style = if selected { Style::new().bold() } else { Style::new() };
    Line::from(vec![
        Span::styled(if selected { g.bar } else { " " }, t.accent()),
        Span::raw(" "),
        Span::styled(icon, icon_style),
        Span::styled(changed, t.attention()),
        Span::raw(" "),
        Span::styled(truncate(&item.name, name_w, g.more), name_style),
        Span::raw(" "),
        badge_span(&item.act, g, t),
    ])
}

/// `10-mixed.sh ───────── keeps 1 block`: the file, a rule, and what the file becomes.
fn heading(plan: &Plan, si: usize, width: usize, g: &Glyphs, t: &Theme) -> Line<'static> {
    let src = &plan.srcs[si];
    let (_, delete) = plan.leftover(si);
    let left = plan.remaining(si);
    let mut outcome = vec![if delete {
        Span::styled(format!("{} deleted (empty)", g.arrow), t.danger())
    } else {
        let s = if left == 1 { "" } else { "s" };
        Span::styled(format!("keeps {left} block{s}"), t.muted())
    }];
    let moving = plan.items.iter().any(|i| i.src == si && matches!(i.act, Act::Move(_)));
    if src.guarded() && moving {
        outcome.push(Span::styled(format!(" {} interactive only", g.dot), t.muted()));
    }
    let name = file_name(&src.shown).to_string();
    let used = name.chars().count() + 2 + outcome.iter().map(|s| s.content.chars().count()).sum::<usize>();
    let rule = g.hline.repeat(width.saturating_sub(used).max(1));
    let mut spans = vec![Span::styled(name, Style::new().bold()), Span::raw(" "), Span::styled(rule, t.muted()), Span::raw(" ")];
    spans.extend(outcome);
    Line::from(spans)
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

/// A key drawn like a key cap: on the surface tint with true color, bold accent otherwise.
fn keycap(k: &str, t: &Theme) -> Span<'static> {
    let style = t.accent().bold();
    match t.surface() {
        Some(_) => Span::styled(format!(" {k} "), t.on_surface(style)),
        None => Span::styled(k.to_string(), style),
    }
}


/// Rows `lines` take once wrapped at `width` (one at least per line).
fn wrapped_height(lines: &[Line], width: u16) -> u16 {
    let w = width.max(1) as usize;
    lines.iter().map(|l| l.width().max(1).div_ceil(w) as u16).sum()
}

pub(crate) fn draw(f: &mut Frame, app: &App, plan: &Plan) {
    let (g, t) = (&app.g, &app.theme);
    let d = g.dot;
    let area = f.area();
    let [head, body, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(2)]).areas(area);
    app.wide.set(area.width >= WIDE);
    let rows = rows(plan);

    // Header: logo, title, progress through the unsure blocks, counts.
    let count = |pred: fn(&Item) -> bool| plan.items.iter().filter(|i| pred(i)).count();
    let moves = count(|i| matches!(i.act, Act::Move(_)));
    let drops = count(|i| matches!(i.act, Act::Dup(_)));
    let keeps = count(|i| i.act == Act::Keep);
    let total = plan.items.len();
    let decided = total - count(undecided);
    let filled = (decided * BAR).checked_div(total).unwrap_or(BAR);
    let done = decided == total;
    let mut title = vec![
        Span::raw(" "),
        Span::styled(g.logo, t.accent().bold()),
        Span::styled(" rc-sort tidy   ", Style::new().bold()),
        Span::styled(g.full.repeat(filled), if done { t.good() } else { t.accent() }),
        Span::styled(g.empty.repeat(BAR - filled), t.muted()),
        Span::raw(" "),
    ];
    if done {
        title.push(Span::styled(format!("all decided {d} y to apply"), t.good()));
    } else {
        title.push(Span::styled(format!("{decided}/{total} decided"), Style::new()));
    }
    title.push(Span::styled(format!("   {moves} move {d} {keeps} keep {d} {drops} remove"), t.muted()));
    f.render_widget(Paragraph::new(Line::from(title)), head);
    f.render_widget(Line::styled("? keys ", t.muted()).right_aligned(), head);

    // One rounded box; a thin divider splits list and preview.
    let outer = Block::bordered().border_set(g.border).border_style(t.muted());
    let inner = outer.inner(body);
    f.render_widget(outer, body);
    let (left, right) = if app.show_preview(area.width >= WIDE) {
        let [l, div, r] =
            Layout::horizontal([Constraint::Percentage(52), Constraint::Length(1), Constraint::Min(10)]).areas(inner);
        let buf = f.buffer_mut();
        for y in div.y..div.y + div.height {
            if let Some(c) = buf.cell_mut((div.x, y)) {
                c.set_symbol(g.vline).set_style(t.muted());
            }
        }
        for (y, sym) in [(body.y, g.tee_top), (body.y + body.height.saturating_sub(1), g.tee_bottom)] {
            if let Some(c) = buf.cell_mut((div.x, y)) {
                c.set_symbol(sym);
            }
        }
        (l, Some(r.inner(Margin::new(1, 0))))
    } else {
        (inner, None)
    };
    let list_area = left.inner(Margin::new(1, 0));
    app.list_area.set(list_area);

    let width = list_area.width as usize;
    let items: Vec<ListItem> = rows
        .iter()
        .map(|r| match r {
            Row::Gap => ListItem::new(Line::default()),
            Row::File(si) => ListItem::new(heading(plan, *si, width, g, t)),
            Row::Item(i) => ListItem::new(item_line(&plan.items[*i], *i == app.sel, width, g, t)),
        })
        .collect();
    let list = List::new(items).highlight_style(t.on_surface(Style::new()));
    {
        let mut state = app.list.borrow_mut();
        state.select(rows.iter().position(|r| *r == Row::Item(app.sel)));
        f.render_stateful_widget(list, list_area, &mut state);
    }

    if let (Some(right), Some(item)) = (right, plan.items.get(app.sel)) {
        let (title, lines) = preview(plan, app.sel, g, t);
        let mut text = vec![title, Line::default()];
        text.extend(lines);
        let (top, bottom) = match &item.clash {
            Some(_) => {
                // The block gets the room it needs; the compare part takes the rest.
                let need = wrapped_height(&text, right.width).min(right.height.saturating_sub(3));
                let [a, b] = Layout::vertical([Constraint::Length(need), Constraint::Min(3)]).areas(right);
                (a, Some(b))
            }
            None => (right, None),
        };
        f.render_widget(Paragraph::new(text).wrap(Wrap { trim: false }), top);
        if let (Some(b), Some((file, existing))) = (bottom, &item.clash) {
            let label = format!(" already in {file} {d} compare ");
            let fill = (b.width as usize).saturating_sub(label.chars().count() + 2);
            let mut lines = vec![Line::styled(format!("{}{}{}", g.hline.repeat(2), label, g.hline.repeat(fill)), t.muted())];
            lines.extend(existing.lines().map(|l| shell_line("  ", l, Style::new())));
            f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), b);
        }
    }

    // Footer line 1: each choice key and the word it sets; line 2: confirm, message or moving around.
    let mut keys = vec![Span::raw("  ")];
    let choices = [
        ("t", Act::Move(Category::Tools)),
        ("e", Act::Move(Category::Exports)),
        ("a", Act::Move(Category::Aliases)),
        ("f", Act::Move(Category::Functions)),
        ("k", Act::Keep),
        ("l", Act::Leave),
    ];
    for (k, act) in &choices {
        keys.extend([keycap(k, t), Span::styled(format!(" {}  ", word(act)), t.muted())]);
    }
    keys.extend([keycap("r", t), Span::styled(" reset", t.muted())]);
    let second = if app.confirm {
        Line::from(vec![
            Span::styled("  Apply? ", t.attention().bold()),
            Span::raw(format!("{moves} move {d} {keeps} keep {d} {drops} remove   ")),
            keycap("y", t),
            Span::styled(" yes  any other key goes back", t.muted()),
        ])
    } else if !app.status.is_empty() {
        Line::styled(format!("  {}", app.status), t.attention())
    } else {
        let mut nav = vec![Span::raw("  ")];
        let hints = [("tab", "next unsure"), (g.updown, "select"), ("y", "apply"), ("q", "quit"), ("?", "all keys")];
        for (i, (k, what)) in hints.iter().enumerate() {
            if i > 0 {
                nav.push(Span::styled(format!(" {d} "), t.muted()));
            }
            nav.extend([keycap(k, t), Span::styled(format!(" {what}"), t.muted())]);
        }
        Line::from(nav)
    };
    f.render_widget(Paragraph::new(vec![Line::from(keys), second]), foot);

    if app.help {
        let help = keys_help(g);
        let h = (help.len() as u16 + 2).min(inner.height);
        let area = Rect { height: h, ..inner };
        let keys: Vec<Line> = help.into_iter().map(Line::from).collect();
        let block = Block::bordered().border_set(g.border).border_style(t.accent()).title(" keys · any key closes ");
        f.render_widget(Clear, area);
        f.render_widget(Paragraph::new(keys).block(block), area);
    }

    if !app.color {
        for cell in f.buffer_mut().content.iter_mut() {
            cell.set_fg(Color::Reset);
            cell.set_bg(Color::Reset);
        }
    }
}

/// Preview heading and lines: the end of the target file muted, then the block as `+` lines.
fn preview(plan: &Plan, sel: usize, g: &Glyphs, t: &Theme) -> (Line<'static>, Vec<Line<'static>>) {
    let Some(item) = plan.items.get(sel) else { return (Line::default(), Vec::new()) };
    let d = g.dot;
    let mut lines = Vec::new();
    if !item.block.balanced {
        lines.push(Line::styled("half of an if/loop: it can't be moved", t.danger()));
    }
    match item.note.as_deref() {
        Some("") => lines.push(Line::styled("unsure: no rule matched", t.attention())),
        Some(why) => lines.push(Line::styled(format!("unsure: {why}"), t.attention())),
        None => {}
    }
    if item.act != item.suggested {
        let was = word(&item.suggested);
        lines.push(Line::styled(format!("changed by you (suggested: {was}; r resets)"), t.attention()));
    }
    if !lines.is_empty() {
        lines.push(Line::default());
    }
    let src = file_name(&plan.srcs[item.src].shown).to_string();
    let (what, rest) = match &item.act {
        Act::Move(cat) => {
            let name = cat.file_name();
            let target = plan.files.iter().find(|(n, _)| n == name).map(|(_, t)| t.as_str());
            if let Some(text) = target {
                let tail: Vec<&str> = text.lines().collect();
                for l in &tail[tail.len().saturating_sub(TAIL)..] {
                    lines.push(shell_line("  ", l, t.muted()));
                }
            }
            lines.extend(item.text.lines().map(|l| shell_line("+ ", l, t.good())));
            (name.to_string(), if target.is_some() { "end of file".to_string() } else { "new file".to_string() })
        }
        Act::Dup(other) => {
            lines.extend(item.block.text.lines().map(|l| shell_line("= ", l, t.muted())));
            ("remove".to_string(), format!("exact copy already in {other}"))
        }
        Act::Keep => {
            lines.extend(item.block.text.lines().map(|l| shell_line("  ", l, Style::new())));
            ("keep".to_string(), format!("stays in {src} for good"))
        }
        Act::Leave => {
            lines.extend(item.block.text.lines().map(|l| shell_line("  ", l, Style::new())));
            ("later".to_string(), format!("stays in {src} for now"))
        }
    };
    let heading = Line::from(vec![
        Span::styled(what, Style::new().bold()),
        Span::styled(format!(" {d} {rest}"), t.muted()),
    ]);
    (heading, lines)
}

/// What stays in scrollback once the box is gone. `skipped` is `None` when nothing was
/// applied, else the sources `apply` skipped because they changed meanwhile.
pub(crate) fn summary(plan: &Plan, skipped: Option<&[usize]>, backups: &str, width: usize, g: &Glyphs) -> Vec<String> {
    let mut out = vec![format!("{} rc-sort tidy", g.title)];
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
            g.pipe, plan.srcs[si].shown
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
    let indent = format!("{} {}", g.pipe, " ".repeat(label.chars().count()));
    let mut line = format!("{} {label}", g.pipe);
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
    for (mark, code) in [(g.title, "32"), (g.pipe, "2"), (g.end, "2")] {
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
        let home = std::env::temp_dir().join(format!("rc-sort-tui-{}-{name}", std::process::id()));
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
        assert_eq!(rows(&plan), vec![Row::File(0), Row::Item(0), Row::Item(1), Row::Gap, Row::File(1), Row::Item(2)]);
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
        for want in ["◆ rc-sort tidy", "▰▰▰▰▱▱▱▱ 1/2 decided", "1 move · 0 keep · 0 remove", "30-x.sh ─",
                     "─ keeps 1 block", "▌ ✓  alias g=git", "● aliases", "?  ssh-agent", "◐ later", "aliases.sh · new file",
                     "+ alias g=git", "t tools  e exports  a aliases  f functions  k keep  l later  r reset",
                     "tab next unsure · ↑↓ select · y apply", "╭", "┬", "┴"] {
            assert!(s.contains(want), "missing {want:?} in {s}");
        }
        assert!(!screen(&app, &plan, 70, 10).contains("new file"), "narrow: preview hidden");
        app.preview = Some(true);
        assert!(screen(&app, &plan, 70, 10).contains("new file"), "p shows it anyway");
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
        assert!(s.contains("─ → deleted (empty)"), "{s}");
        assert!(s.contains("✓  alias") && s.contains("▌ ?• ssh-agent"), "changed rows are marked: {s}");
        assert!(s.contains("changed by you (suggested: later; r resets)"), "{s}");

        fs::write(p.rc_dir().join("30-x.sh"), "[[ $- == *i* ]] || return\nalias g=git\n").unwrap();
        let plan = super::plan(&p, None, &mut Vec::new()).unwrap();
        assert!(screen(&App::default(), &plan, 110, 10).contains("interactive only"));
    }

    #[test]
    fn wrapped_height_counts_wrapped_rows() {
        let lines = vec![Line::from("x".repeat(25)), Line::default(), Line::from("short")];
        assert_eq!(wrapped_height(&lines, 10), 3 + 1 + 1);
    }

    #[test]
    fn same_tool_clash_shows_the_existing_block() {
        let p = setup("clash");
        fs::write(p.rc_dir().join("tools.sh"), "# rust\n. \"$HOME/.cargo/env\"\n").unwrap();
        fs::write(p.rc_dir().join("30-x.sh"), "export PATH=\"$HOME/.cargo/bin:$PATH\"\n").unwrap();
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(plan.items[0].clash, Some(("tools.sh".into(), "# rust\n. \"$HOME/.cargo/env\"".into())));
        let s = screen(&App::default(), &plan, 110, 14);
        assert!(s.contains("── already in tools.sh · compare ─"), "{s}");
        assert!(s.contains(". \"$HOME/.cargo/env\""), "{s}");
        assert!(s.contains("  export PATH=\"$HOME/.cargo/bin:$PATH\""), "the block itself stays visible: {s}");
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
        assert!(s.contains("> v  alias g=git") && s.contains("* aliases") && s.contains("+---") && s.contains("##"), "{s}");
        assert!(s.is_ascii(), "{s}");
    }

    #[test]
    fn palette_rules_hold_on_every_cell() {
        let p = setup("palette");
        fs::write(p.rc_dir().join("60-dup.sh"), "alias g=git\n").unwrap();
        fs::write(p.rc_dir().join("aliases.sh"), "alias g=git\n").unwrap();
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        for tier in [Tier::Ansi, Tier::Dark, Tier::Light] {
            let t = Theme { tier };
            let mut app = App { theme: t, ..App::default() };
            let mut cells = Vec::new();
            for (confirm, help) in [(false, false), (true, false), (false, true)] {
                app.confirm = confirm;
                app.help = help;
                cells.extend(terminal(&app, &plan, 110, 12).backend().buffer().content().to_vec());
            }
            let fg_ok: Vec<Color> = [t.accent(), t.good(), t.attention(), t.danger(), t.muted()]
                .iter()
                .filter_map(|s| s.fg)
                .chain([Color::Reset])
                .collect();
            let bg_ok = [Some(Color::Reset), t.surface()];
            for c in &cells {
                assert!(bg_ok.contains(&Some(c.bg)), "{tier:?}: only the surface tint as background: {c:?}");
                assert!(fg_ok.contains(&c.fg), "{tier:?}: only palette colors, no black/white: {c:?}");
            }
            if tier == Tier::Ansi {
                assert!(cells.iter().all(|c| c.bg == Color::Reset), "no backgrounds on 16 colors");
            }
        }
    }

    #[test]
    fn theme_comes_from_the_environment() {
        // Only checks the mapping; the env vars of the test process are left as they are.
        assert_eq!(Theme { tier: Tier::Ansi }.surface(), None);
        assert!(Theme { tier: Tier::Dark }.surface().is_some());
        assert_ne!(Theme { tier: Tier::Dark }.accent(), Theme { tier: Tier::Light }.accent());
        assert_eq!(Theme { tier: Tier::Ansi }.accent().fg, Some(Color::Cyan));
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
        // Row 0 header line, 1 top border, 2 "30-x.sh", 3 alias, 4 ssh-agent, 5 gap, 6 "40-y.sh", 7 export.
        app.mouse(&plan, click(4));
        assert_eq!(app.sel, 1);
        app.mouse(&plan, click(5));
        assert_eq!(app.sel, 1, "the gap between files does nothing");
        app.mouse(&plan, click(6));
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
        assert_eq!(summary(&plan, None, "~/b", 80, &UNICODE), vec!["◇ rc-sort tidy", "└ nothing changed"]);
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
