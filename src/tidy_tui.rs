//! Inline front end for `bashrc-sort tidy`: a box drawn below the prompt (fits the content,
//! at most 60% of the terminal), blocks grouped by source file with a diff preview, and a
//! short summary left in scrollback when it closes. Same plan and same apply as the plain
//! prompts (`tidy::run_tidy`); only how you choose differs.

use crate::classify::Category;
use crate::state::Paths;
use crate::tidy::{apply, plan, Act, Item, Plan};
use anyhow::Result;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::crossterm::terminal;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap};
use ratatui::{Frame, TerminalOptions, Viewport};
use std::cell::Cell;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

const FOOTER: &str = " t e a f move · k keep · u leave · y apply · q quit";
const KEYS: [&str; 4] = [
    " ↑ ↓  PgUp PgDn  Home End   select",
    " t e a f   move to tools / exports / aliases / functions",
    " k  keep in place, never asked again     u  leave for now",
    " p  show/hide preview     y  apply     q Esc  quit",
];
/// Lines of the target file shown above the block in the preview.
const TAIL: usize = 3;
/// Below this width the preview starts hidden (`p` shows it).
const WIDE: u16 = 90;

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
    let (width, rows) = terminal::size()?;
    let viewport = Viewport::Inline(height(&plan, rows));
    let mut term = ratatui::init_with_options(TerminalOptions { viewport });
    let mut app = App::default();
    let step: Result<Step> = loop {
        if let Err(e) = term.draw(|f| draw(f, &app, &plan)) {
            break Err(e.into());
        }
        match event::read() {
            Ok(Event::Key(k)) if k.kind == KeyEventKind::Press => match app.handle(&mut plan, k) {
                Step::Stay => {}
                s => break Ok(s),
            },
            Ok(_) => {}
            Err(e) => break Err(e.into()),
        }
    };
    // Erase the box; the summary is printed where it was.
    // Where the box starts on screen (`draw` results only carry a frame-relative area).
    let top = term.get_frame().area().y;
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
    let color = std::env::var_os("NO_COLOR").is_none();
    for line in summary(&plan, skipped.as_deref(), &backups, width as usize) {
        writeln!(out, "{}", paint(&line, color))?;
    }
    Ok(())
}

/// Box height: header, footer, borders and one row per file and block; at most 60% of the
/// terminal (but never under 10 lines), the list scrolls past that.
pub(crate) fn height(plan: &Plan, term_rows: u16) -> u16 {
    let rows = rows(plan).len() as u16;
    let want = rows.max(4) + 4;
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
#[derive(Default)]
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
    /// Whether the last frame was wide enough for the preview by default.
    wide: Cell<bool>,
}

impl App {
    fn show_preview(&self, wide: bool) -> bool {
        self.preview.unwrap_or(wide)
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
            KeyCode::Char('q') | KeyCode::Esc => return Step::Quit,
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('p') => self.preview = Some(!self.show_preview(self.wide.get())),
            KeyCode::Char('y') => {
                if plan.has_changes() {
                    self.confirm = true;
                } else {
                    self.status = "nothing to apply: every block is set to leave".into();
                }
            }
            KeyCode::Char('k') => plan.items[self.sel].act = Act::Keep,
            KeyCode::Char('u') => plan.items[self.sel].act = Act::Leave,
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
}

fn cat_color(cat: Category) -> Color {
    match cat {
        Category::Tools => Color::Blue,
        Category::Exports => Color::Magenta,
        Category::Aliases => Color::Green,
        Category::Functions => Color::Cyan,
    }
}

fn label(act: &Act) -> (String, Style) {
    match act {
        Act::Move(cat) => (format!("→ {}", cat.file_name()), Style::new().fg(cat_color(*cat))),
        Act::Dup(_) => ("✗ drop".into(), Style::new().fg(Color::Yellow)),
        Act::Keep => ("· keep".into(), Style::new().bold()),
        Act::Leave => ("· leave".into(), Style::new().dim()),
    }
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        return format!("{s:<w$}");
    }
    let mut t: String = s.chars().take(w.saturating_sub(1)).collect();
    t.push('…');
    t
}

fn file_name(shown: &str) -> &str {
    shown.rsplit('/').next().unwrap_or(shown)
}

/// `✓ name ……… → target`, padded so the targets line up at the right edge.
fn item_line(item: &Item, width: usize) -> Line<'static> {
    let (icon, icon_style) = if !item.block.balanced {
        ("⊘", Style::new().fg(Color::Red))
    } else if item.note.is_some() {
        ("?", Style::new().fg(Color::Yellow))
    } else {
        ("✓", Style::new().fg(Color::Green))
    };
    let (dest, dest_style) = label(&item.act);
    let name_w = width.saturating_sub(dest.chars().count() + 3).max(4);
    Line::from(vec![
        Span::styled(icon, icon_style),
        Span::raw(format!(" {} ", truncate(&item.name, name_w))),
        Span::styled(dest, dest_style),
    ])
}

pub(crate) fn draw(f: &mut Frame, app: &App, plan: &Plan) {
    let area = f.area();
    let [head, body, foot] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(3), Constraint::Length(1)]).areas(area);
    app.wide.set(area.width >= WIDE);
    let rows = rows(plan);

    let files = rows.iter().filter(|r| matches!(r, Row::File(_))).count();
    let title = Line::from(vec![
        Span::raw(" bashrc-sort tidy").bold(),
        Span::raw(format!(" · {} block(s) in {files} file(s)", plan.items.len())).dim(),
    ]);
    f.render_widget(Paragraph::new(title), head);
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
            Row::File(si) => ListItem::new(Line::from(file_name(&plan.srcs[*si].shown).to_string()).bold()),
            Row::Item(i) => ListItem::new(item_line(&plan.items[*i], width)),
        })
        .collect();
    let selected = rows.iter().position(|r| *r == Row::Item(app.sel));
    let list = List::new(items)
        .block(Block::bordered().border_type(BorderType::Rounded))
        .highlight_symbol("▸ ")
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().bold());
    f.render_stateful_widget(list, left, &mut ListState::default().with_selected(selected));

    if let Some(right) = right {
        let (title, lines) = preview(plan, app.sel);
        let block = Block::bordered().border_type(BorderType::Rounded).title(title);
        f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }).block(block), right);
    }

    let footer = if app.confirm {
        let count = |pred: fn(&Act) -> bool| plan.items.iter().filter(|i| pred(&i.act)).count();
        let moves = count(|a| matches!(a, Act::Move(_)));
        let drops = count(|a| matches!(a, Act::Dup(_)));
        let keeps = count(|a| *a == Act::Keep);
        Line::from(vec![
            Span::raw(" Apply? ").bold().fg(Color::Yellow),
            Span::raw(format!("{moves} move · {keeps} keep · {drops} drop   ")),
            Span::raw("y/N").bold(),
        ])
    } else if !app.status.is_empty() {
        Line::from(format!(" {}", app.status)).fg(Color::Yellow)
    } else {
        Line::from(FOOTER).dim()
    };
    f.render_widget(footer, foot);

    if app.help {
        let h = (KEYS.len() as u16 + 2).min(body.height);
        let area = Rect { height: h, ..body };
        let keys: Vec<Line> = KEYS.iter().map(|k| Line::from(*k)).collect();
        f.render_widget(Clear, area);
        let block = Block::bordered().border_type(BorderType::Rounded).title(" keys · any key closes ");
        f.render_widget(Paragraph::new(keys).block(block), area);
    }
}

/// Preview title and lines: the end of the target file dimmed, then the block as `+` lines.
fn preview(plan: &Plan, sel: usize) -> (String, Vec<Line<'static>>) {
    let Some(item) = plan.items.get(sel) else { return (String::new(), Vec::new()) };
    let mut lines = Vec::new();
    if !item.block.balanced {
        lines.push(Line::from("half of an if/loop: it can't be moved").fg(Color::Red));
    }
    match item.note.as_deref() {
        Some("") => lines.push(Line::from("unsure: no rule matched").fg(Color::Yellow)),
        Some(why) => lines.push(Line::from(format!("unsure: {why}")).fg(Color::Yellow)),
        None => {}
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
                    lines.push(Line::from(format!("  {l}")).dim());
                }
            }
            let style = Style::new().fg(Color::Green);
            lines.extend(item.text.lines().map(|l| Line::styled(format!("+ {l}"), style)));
            format!(" {name} ({}) ", if target.is_some() { "end" } else { "new" })
        }
        Act::Dup(other) => {
            lines.extend(item.block.text.lines().map(|l| Line::from(format!("= {l}")).dim()));
            format!(" already in {other} · dropped on apply ")
        }
        Act::Keep | Act::Leave => {
            lines.extend(item.block.text.lines().map(|l| Line::from(format!("  {l}"))));
            let src = file_name(&plan.srcs[item.src].shown);
            if item.act == Act::Keep { format!(" stays in {src} · not asked again ") } else { format!(" stays in {src} for now ") }
        }
    };
    (title, lines)
}

/// What stays in scrollback once the box is gone. `skipped` is `None` when nothing was
/// applied, else the sources `apply` skipped because they changed meanwhile.
pub(crate) fn summary(plan: &Plan, skipped: Option<&[usize]>, backups: &str, width: usize) -> Vec<String> {
    let mut out = vec!["◇ bashrc-sort tidy".to_string()];
    let Some(skipped) = skipped else {
        out.push("└ nothing changed".into());
        return out;
    };
    let applied = |i: &&Item| !skipped.contains(&i.src);
    let moves: Vec<String> = plan
        .items
        .iter()
        .filter(applied)
        .filter_map(|i| match &i.act {
            Act::Move(cat) => Some(format!("{} → {}", i.name, cat.file_name())),
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
    wrap_into(&mut out, "", &moves, width);
    wrap_into(&mut out, "dropped: ", &drops, width);
    wrap_into(&mut out, "kept: ", &keeps, width);
    for &si in skipped {
        out.push(format!("│ skipped {}: it changed while you were choosing; run tidy again", plan.srcs[si].shown));
    }
    out.push(format!("└ done · backups in {backups}"));
    out.push("  open shells keep the old setup until you run: source ~/.bashrc".into());
    out
}

/// `│ label a · b · c`, wrapped at `width` with continuation lines indented under the label.
fn wrap_into(out: &mut Vec<String>, label: &str, parts: &[String], width: usize) {
    if parts.is_empty() {
        return;
    }
    let indent = format!("│ {}", " ".repeat(label.chars().count()));
    let mut line = format!("│ {label}");
    let mut empty = true;
    for part in parts {
        let len = line.chars().count() + 3 + part.chars().count();
        if !empty && len > width {
            out.push(std::mem::replace(&mut line, indent.clone()));
            empty = true;
        }
        if !empty {
            line.push_str(" · ");
        }
        line.push_str(part);
        empty = false;
    }
    out.push(line);
}

/// Colors the leading `◇ │ └` marker (green / dim), unless NO_COLOR is set.
fn paint(line: &str, color: bool) -> String {
    let mut chars = line.chars();
    let (Some(mark), true) = (chars.next(), color) else { return line.to_string() };
    let code = if mark == '◇' { "32" } else { "2" };
    if line == "└ nothing changed" {
        return format!("\x1b[2m{line}\x1b[0m");
    }
    format!("\x1b[{code}m{mark}\x1b[0m{}", chars.as_str())
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

    fn screen(app: &App, plan: &Plan, w: u16, h: u16) -> String {
        let mut term = Terminal::new(TestBackend::new(w, h)).unwrap();
        term.draw(|f| draw(f, app, plan)).unwrap();
        term.backend().buffer().content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn starts_from_the_plan() {
        let p = setup("start");
        let plan = plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(plan.items.len(), 2);
        assert_eq!(plan.items[0].act, Act::Move(Category::Aliases));
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
        assert_eq!(height(&plan, 50), 8, "3 rows → minimum box");
        let many = (0..60).map(|i| format!("alias a{i}=b\n\n")).collect::<String>();
        fs::write(p.rc_dir().join("30-x.sh"), many).unwrap();
        let plan = super::plan(&p, None, &mut Vec::new()).unwrap();
        assert_eq!(height(&plan, 50), 30);
        assert_eq!(height(&plan, 12), 10, "never under 10 lines");
    }

    #[test]
    fn keys_change_any_block() {
        let p = setup("keys");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        let mut app = App::default();
        press(&mut app, &mut plan, &[KeyCode::Char('e')]);
        assert_eq!(plan.items[0].act, Act::Move(Category::Exports), "sure blocks can be overridden");
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
        let s = screen(&app, &plan, 100, 10);
        for want in ["2 block(s) in 1 file(s)", "30-x.sh", "▸ ✓ alias g=git", "→ aliases.sh", "? ssh-agent", "· leave",
                     "aliases.sh (new)", "+ alias g=git", "y apply"] {
            assert!(s.contains(want), "missing {want:?} in {s}");
        }
        assert!(!screen(&app, &plan, 70, 10).contains("(new)"), "narrow: preview hidden");
        app.preview = Some(true);
        assert!(screen(&app, &plan, 70, 10).contains("(new)"), "p shows it anyway");
        app.confirm = true;
        assert!(screen(&app, &plan, 100, 10).contains("Apply? 1 move · 0 keep · 0 drop"));
        app.confirm = false;
        app.help = true;
        assert!(screen(&app, &plan, 100, 10).contains("keep in place, never asked again"));
    }

    #[test]
    fn summary_says_what_happened() {
        let p = setup("summary");
        let mut plan = plan(&p, None, &mut Vec::new()).unwrap();
        plan.items[1].act = Act::Keep;
        assert_eq!(summary(&plan, None, "~/b", 80), vec!["◇ bashrc-sort tidy", "└ nothing changed"]);
        let s = summary(&plan, Some(&[]), "~/b", 80);
        assert_eq!(s[1], "│ alias g=git → aliases.sh");
        assert_eq!(s[2], "│ kept: ssh-agent -s > /dev/null");
        assert_eq!(s[3], "└ done · backups in ~/b");
        let s = summary(&plan, Some(&[0]), "~/b", 80);
        assert!(!s.iter().any(|l| l.contains("→")), "skipped files moved nothing: {s:?}");
        assert!(s.iter().any(|l| l.starts_with("│ skipped ~/.bashrc.d/30-x.sh")));
    }

    #[test]
    fn summary_wraps_long_lists() {
        let mut out = Vec::new();
        let parts: Vec<String> = (0..6).map(|i| format!("block{i} → tools.sh")).collect();
        wrap_into(&mut out, "", &parts, 50);
        assert!(out.len() > 1 && out.iter().all(|l| l.chars().count() <= 50), "{out:?}");
        assert!(out[1].starts_with("│ block"));
    }
}
