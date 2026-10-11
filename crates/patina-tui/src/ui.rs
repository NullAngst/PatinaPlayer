//! Drawing. Sidebar on the left, the open view in the middle, the queue
//! panel on the right when it's toggled on, and the two-line bottom bar.

use patina_core::{PlayState, Repeat};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table};

use crate::app::{App, Focus, InputPurpose, Modal, SideEntry};
use crate::views::{Item, Section, Source, fmt_duration, fmt_length};

const SIDEBAR_W: u16 = 24;
const QUEUE_W: u16 = 42;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(app.theme.bg).fg(app.theme.text)),
        area,
    );

    let toast_h = u16::from(app.toast.is_some());
    let [main, toast, bar] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(toast_h),
        Constraint::Length(2),
    ])
    .areas(area);

    let side_w = if area.width >= 70 { SIDEBAR_W } else { 0 };
    let queue_w = if app.queue_panel && area.width >= 100 {
        QUEUE_W
    } else {
        0
    };
    let [side, content, panel] = Layout::horizontal([
        Constraint::Length(side_w),
        Constraint::Min(20),
        Constraint::Length(queue_w),
    ])
    .areas(main);

    draw_sidebar(f, app, side);
    if app.section == Section::Queue {
        draw_queue(f, app, content);
    } else {
        draw_level(f, app, content);
    }
    if queue_w > 0 {
        draw_queue_panel(f, app, panel);
    }
    draw_toast(f, app, toast);
    draw_bar(f, app, bar);

    match &app.modal {
        Modal::Help => draw_help(f, app, area),
        Modal::Input { purpose, buf } if !matches!(purpose, InputPurpose::Search) => {
            let title = match purpose {
                InputPurpose::NewPlaylist { .. } => "New playlist",
                InputPurpose::RenamePlaylist { .. } => "Rename playlist",
                InputPurpose::SaveQueue => "Save queue as playlist",
                InputPurpose::Search => "",
            };
            draw_input_box(f, app, area, title, buf);
        }
        Modal::PickPlaylist { rows, selected, .. } => {
            let names: Vec<String> = std::iter::once("New playlist…".to_string())
                .chain(rows.iter().map(|r| format!("{}  ({})", r.name, r.items)))
                .collect();
            draw_pick(f, app, area, "Add to playlist", &names, *selected);
        }
        Modal::Confirm { msg, .. } => draw_confirm(f, app, area, msg),
        _ => {}
    }
}

fn draw_toast(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let Some((msg, _, err)) = &app.toast else {
        return;
    };
    let (tag, color) = if *err {
        (" ! ", t.copper)
    } else {
        (" · ", t.verdigris)
    };
    let p = Paragraph::new(Line::from(vec![
        Span::styled(tag, Style::default().fg(t.bg).bg(color)),
        Span::styled(format!(" {msg}"), Style::default().fg(color)),
    ]))
    .style(Style::default().bg(t.surface));
    f.render_widget(p, area);
}

// ----- Sidebar -----

fn draw_sidebar(f: &mut Frame, app: &mut App, area: Rect) {
    app.sidebar_area = area;
    if area.width == 0 {
        return;
    }
    let t = &app.theme;
    f.render_widget(Block::default().style(Style::default().bg(t.surface)), area);
    let status = app.service.as_ref().map(|s| s.status());
    let focused = app.focus == Focus::Sidebar;
    let mut lines = Vec::new();
    for (i, e) in app.sidebar.iter().enumerate() {
        let sel = i == app.sidebar_sel;
        let base = if sel && focused {
            Style::default().bg(t.raised)
        } else {
            Style::default()
        };
        let line = match e {
            SideEntry::Section(s) => {
                let num = Section::NUMBERED
                    .iter()
                    .position(|x| x == s)
                    .map(|n| format!("{}", n + 1))
                    .unwrap_or_else(|| " ".into());
                let active = *s == app.section;
                let style = if active {
                    base.fg(t.verdigris).add_modifier(Modifier::BOLD)
                } else {
                    base.fg(t.text)
                };
                Line::from(vec![
                    Span::styled(format!(" {num} "), base.fg(t.wave_raw)),
                    Span::styled(pad(s.label(), area.width as usize - 3), style),
                ])
            }
            SideEntry::Source { path, tracks } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let scanning = status
                    .as_ref()
                    .and_then(|s| s.scanning.as_ref())
                    .filter(|sc| sc.root == *path && sc.total > 0);
                let (dot, dot_color) = if !path.is_dir() {
                    ("●", t.copper)
                } else if scanning.is_some() {
                    ("◌", t.verdigris)
                } else if status.as_ref().is_some_and(|s| s.watching) {
                    ("●", t.verdigris)
                } else {
                    ("●", t.muted)
                };
                let right = match scanning {
                    Some(sc) => format!("{}/{}", sc.done, sc.total),
                    None => tracks.to_string(),
                };
                let w = area.width as usize;
                let name_w = w.saturating_sub(right.len() + 5);
                Line::from(vec![
                    Span::styled(format!(" {dot} "), base.fg(dot_color)),
                    Span::styled(pad(&truncate(&name, name_w), name_w), base.fg(t.text)),
                    Span::styled(format!(" {right} "), base.fg(t.muted)),
                ])
            }
            SideEntry::Label(l) => {
                Line::from(Span::styled(format!(" {l}"), Style::default().fg(t.muted)))
            }
            SideEntry::Gap => Line::from(""),
        };
        lines.push(line);
    }
    f.render_widget(Paragraph::new(lines), area);
}

// ----- Library levels -----

/// What columns a level uses, from the kind of rows in it.
#[derive(PartialEq)]
enum Kind {
    Tracks,
    Albums,
    Names,
    Info,
}

fn level_kind(items: &[Item]) -> Kind {
    if items
        .iter()
        .any(|i| matches!(i, Item::Track(_) | Item::Missing { .. }))
        && !items
            .iter()
            .any(|i| matches!(i, Item::Album(_) | Item::Artist { .. }))
    {
        Kind::Tracks
    } else if items.iter().all(|i| matches!(i, Item::Info { .. })) {
        Kind::Info
    } else if items.iter().all(|i| matches!(i, Item::Album(_))) && !items.is_empty() {
        Kind::Albums
    } else {
        Kind::Names
    }
}

fn hints(app: &App) -> &'static str {
    let src = app.level().map(|l| &l.source);
    match (app.section, src) {
        (_, Some(Source::Search(_))) => "Enter open · a add · A play next · Esc back",
        (Section::Albums, Some(Source::Section(_))) => "o sort · a add · p playlist",
        (Section::Songs, _) => "o sort · O reverse · a add · p playlist",
        (Section::Artists, Some(Source::Section(_))) => "t album/track artist · a add",
        (_, Some(Source::Genre { .. })) => "t albums/songs · a add · Esc back",
        (_, Some(Source::Playlist { .. })) => "d remove · J/K move · Esc back",
        (Section::Playlists, _) => "N new · R rename · D delete",
        (Section::Settings | Section::Downloads, _) => "",
        _ => "Enter open · a add · A play next · p playlist",
    }
}

fn title_line<'a>(app: &'a App, width: u16, right: &'a str) -> Line<'a> {
    let t = &app.theme;
    let crumbs = app.crumbs();
    let mut spans = vec![Span::raw(" ")];
    let n = crumbs.len();
    for (i, c) in crumbs.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" › ", Style::default().fg(t.wave_raw)));
        }
        let style = if i + 1 == n {
            Style::default().fg(t.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.muted)
        };
        spans.push(Span::styled(c.clone(), style));
    }
    let used: usize = spans.iter().map(|s| s.width()).sum();
    let room = (width as usize).saturating_sub(used + 1);
    if right.len() < room {
        spans.push(Span::raw(" ".repeat(room - right.len())));
        spans.push(Span::styled(right, Style::default().fg(t.wave_raw)));
    }
    Line::from(spans)
}

fn draw_level(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = area;
    let [title_a, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);

    // Search box replaces the title while typing.
    if let Modal::Input {
        purpose: InputPurpose::Search,
        buf,
    } = &app.modal
    {
        let t = &app.theme;
        let line = Line::from(vec![
            Span::styled(" / ", Style::default().fg(t.verdigris)),
            Span::styled(buf.clone(), Style::default().fg(t.text)),
            Span::styled("▏", Style::default().fg(t.verdigris)),
        ]);
        f.render_widget(
            Paragraph::new(line).style(Style::default().bg(t.raised)),
            title_a,
        );
    } else {
        let line = title_line(app, title_a.width, hints(app));
        f.render_widget(Paragraph::new(line), title_a);
    }

    let t = &app.theme;
    let Some(level) = app.stacks.get(&app.section).and_then(|s| s.last()) else {
        let msg = app
            .view_error
            .clone()
            .unwrap_or_else(|| "Nothing here".into());
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {msg}"),
                Style::default().fg(t.muted),
            )),
            body,
        );
        return;
    };
    if level.items.is_empty() {
        let msg = match &level.source {
            Source::Search(q) if q.is_empty() => "Type to search the library".to_string(),
            Source::Search(_) => "No matches".to_string(),
            Source::Section(Section::Playlists) => {
                "No playlists yet. Press N to make one.".to_string()
            }
            Source::Playlist { .. } => "Empty. Add tracks with p from any view.".to_string(),
            _ => {
                let scanning = app.service.as_ref().is_some_and(|s| !s.status().ready);
                if scanning {
                    "Scanning your folders…".into()
                } else {
                    "Nothing here".into()
                }
            }
        };
        f.render_widget(
            Paragraph::new(Span::styled(
                format!(" {msg}"),
                Style::default().fg(t.muted),
            )),
            body,
        );
        return;
    }

    let kind = level_kind(&level.items);
    let header = match kind {
        Kind::Tracks => vec!["", "#", "Title", "Artist", "Album", "Time"],
        Kind::Albums => vec!["", "", "Album", "Artist", "Year", "Length"],
        Kind::Names => vec!["", "", "Name", "", "", ""],
        Kind::Info => vec!["", "", "", "", "", ""],
    };
    let rows_h = body.height.saturating_sub(1) as usize;
    let n = level.items.len();
    let (sel, mut offset) = (level.selected.min(n - 1), level.offset);
    let margin = 2.min(rows_h / 4);
    if sel < offset + margin {
        offset = sel.saturating_sub(margin);
    } else if rows_h > 0 && sel + margin >= offset + rows_h {
        offset = (sel + margin + 1).saturating_sub(rows_h);
    }
    offset = offset.min(n.saturating_sub(rows_h));

    let playing = app.snap.track.as_ref().map(|t| t.path.clone());
    let in_album = matches!(level.source, Source::Album { .. });
    let focused = app.focus == Focus::Content;
    let rows: Vec<Row> = level
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows_h)
        .map(|(i, item)| {
            let mut row = item_row(app, item, i, in_album, playing.as_deref());
            if i == sel && item.selectable() {
                row = row.style(Style::default().bg(if focused { t.raised } else { t.surface }));
            }
            row
        })
        .collect();

    let widths = match kind {
        Kind::Tracks => [
            Constraint::Length(1),
            Constraint::Length((n.to_string().len() as u16).max(2)),
            Constraint::Percentage(42),
            Constraint::Percentage(26),
            Constraint::Percentage(32),
            Constraint::Length(7),
        ],
        Kind::Albums => [
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Percentage(48),
            Constraint::Percentage(40),
            Constraint::Length(5),
            Constraint::Length(12),
        ],
        Kind::Names => [
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Percentage(55),
            Constraint::Percentage(25),
            Constraint::Percentage(20),
            Constraint::Length(12),
        ],
        Kind::Info => [
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Length(22),
            Constraint::Min(10),
            Constraint::Length(0),
            Constraint::Length(0),
        ],
    };
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::default().fg(t.muted).bg(t.surface)))
        .column_spacing(1);
    f.render_widget(table, body);

    if let Some(l) = app.stacks.get_mut(&app.section).and_then(|s| s.last_mut()) {
        l.offset = offset;
    }
}

fn item_row<'a>(
    app: &App,
    item: &'a Item,
    i: usize,
    in_album: bool,
    playing: Option<&std::path::Path>,
) -> Row<'a> {
    let t = &app.theme;
    let text = Style::default().fg(t.text);
    let muted = Style::default().fg(t.muted);
    let dim = Style::default().fg(t.wave_raw);
    let cells: Vec<Cell> = match item {
        Item::Track(tr) => {
            let now = playing == Some(tr.path.as_path());
            let (ts, ss) = if now {
                (
                    Style::default()
                        .fg(t.verdigris)
                        .add_modifier(Modifier::BOLD),
                    Style::default().fg(t.verdigris),
                )
            } else {
                (text, muted)
            };
            let num = if in_album {
                tr.track_no.map(|n| n.to_string()).unwrap_or_default()
            } else {
                (i + 1).to_string()
            };
            vec![
                Cell::from(Span::styled(
                    if now { "▶" } else { " " },
                    Style::default().fg(t.verdigris),
                )),
                Cell::from(Span::styled(num, dim)),
                Cell::from(Span::styled(tr.title.clone(), ts)),
                Cell::from(Span::styled(tr.artist.clone().unwrap_or_default(), ss)),
                Cell::from(Span::styled(tr.album.clone().unwrap_or_default(), ss)),
                Cell::from(Span::styled(
                    tr.duration.map(fmt_duration).unwrap_or_default(),
                    muted,
                )),
            ]
        }
        Item::Missing { uri } => {
            let name =
                String::from_utf8_lossy(uri.strip_prefix(b"file:").unwrap_or(uri)).into_owned();
            let name = std::path::Path::new(&name)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or(name);
            vec![
                Cell::from(" "),
                Cell::from(Span::styled((i + 1).to_string(), dim)),
                Cell::from(Span::styled(
                    format!("missing: {name}"),
                    Style::default().fg(t.copper),
                )),
                Cell::from(""),
                Cell::from(""),
                Cell::from(""),
            ]
        }
        Item::Album(a) => vec![
            Cell::from(" "),
            Cell::from(""),
            Cell::from(Span::styled(a.name.clone(), text)),
            Cell::from(Span::styled(a.artist.clone(), muted)),
            Cell::from(Span::styled(
                a.year.map(|y| y.to_string()).unwrap_or_default(),
                muted,
            )),
            Cell::from(Span::styled(
                format!("{} · {}", a.track_count, fmt_length(a.duration)),
                muted,
            )),
        ],
        Item::Artist {
            name,
            albums,
            tracks,
        } => vec![
            Cell::from(" "),
            Cell::from(""),
            Cell::from(Span::styled(name.clone(), text)),
            Cell::from(Span::styled(format!("{albums} album{}", s(*albums)), muted)),
            Cell::from(""),
            Cell::from(Span::styled(format!("{tracks} track{}", s(*tracks)), muted)),
        ],
        Item::Dir { name, .. } => vec![
            Cell::from(Span::styled("▸", dim)),
            Cell::from(""),
            Cell::from(Span::styled(name.clone(), text)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ],
        Item::Root { path, tracks } => vec![
            Cell::from(Span::styled("▸", dim)),
            Cell::from(""),
            Cell::from(Span::styled(path.display().to_string(), text)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(format!("{tracks} track{}", s(*tracks)), muted)),
        ],
        Item::Genre(g) => vec![
            Cell::from(" "),
            Cell::from(""),
            Cell::from(Span::styled(g.name.clone(), text)),
            Cell::from(Span::styled(
                format!("{} album{}", g.albums, s(g.albums)),
                muted,
            )),
            Cell::from(""),
            Cell::from(Span::styled(
                format!("{} track{}", g.tracks, s(g.tracks)),
                muted,
            )),
        ],
        Item::Playlist(p) => vec![
            Cell::from(" "),
            Cell::from(""),
            Cell::from(Span::styled(p.name.clone(), text)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(
                format!("{} track{}", p.items, s(p.items)),
                muted,
            )),
        ],
        Item::Header(h) => vec![
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(
                *h,
                Style::default()
                    .fg(t.verdigris)
                    .add_modifier(Modifier::BOLD),
            )),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ],
        Item::Info { label, value } => vec![
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(label.clone(), muted)),
            Cell::from(Span::styled(value.clone(), text)),
            Cell::from(""),
            Cell::from(""),
        ],
    };
    Row::new(cells)
}

fn s(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ----- Queue -----

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = area;
    let [title_a, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let t = &app.theme;
    let q = app.player.queue();
    let n = q.len();
    let total: std::time::Duration = q
        .play_order()
        .iter()
        .filter_map(|&id| q.get(id).and_then(|e| e.info.duration))
        .sum();
    let title = if n == 0 {
        "Queue".to_string()
    } else {
        format!(
            "Queue · {n} track{} · {}",
            if n == 1 { "" } else { "s" },
            fmt_length(total)
        )
    };
    let hint = "Enter play · d remove · J/K move · S save · C clear";
    let mut spans = vec![Span::styled(
        format!(" {title}"),
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    )];
    let room = (title_a.width as usize).saturating_sub(title.len() + 2);
    if hint.len() < room {
        spans.push(Span::raw(" ".repeat(room - hint.len())));
        spans.push(Span::styled(hint, Style::default().fg(t.wave_raw)));
    }
    f.render_widget(Paragraph::new(Line::from(spans)), title_a);

    if n == 0 {
        f.render_widget(
            Paragraph::new(Span::styled(
                " Queue is empty. Press a on anything in the library to add it.",
                Style::default().fg(t.muted),
            )),
            body,
        );
        return;
    }

    let rows_h = body.height.saturating_sub(1) as usize;
    let sel = app.queue_sel.min(n - 1);
    let margin = 2.min(rows_h / 4);
    let mut offset = app.queue_offset;
    if sel < offset + margin {
        offset = sel.saturating_sub(margin);
    } else if rows_h > 0 && sel + margin >= offset + rows_h {
        offset = (sel + margin + 1).saturating_sub(rows_h);
    }
    offset = offset.min(n.saturating_sub(rows_h));

    let current = q.current();
    let state = app.snap.state;
    let focused = app.focus == Focus::Content;
    let rows: Vec<Row> = q
        .play_order()
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows_h)
        .filter_map(|(pos, &id)| q.get(id).map(|e| (pos, e)))
        .map(|(pos, e)| {
            let info = &e.info;
            let is_cur = current == Some(e.id);
            let marker = match (is_cur, state) {
                (true, PlayState::Playing) => "▶",
                (true, PlayState::Paused) => "‖",
                (true, PlayState::Stopped) => "■",
                _ => " ",
            };
            let (mut style, mut sub) = (Style::default().fg(t.text), Style::default().fg(t.muted));
            if e.failed {
                style = style.fg(t.copper);
                sub = sub.fg(t.copper_dim);
            } else if is_cur {
                style = style.fg(t.verdigris).add_modifier(Modifier::BOLD);
                sub = sub.fg(t.verdigris);
            }
            let mut row = Row::new(vec![
                Cell::from(Span::styled(marker, Style::default().fg(t.verdigris))),
                Cell::from(Span::styled(
                    format!("{}", pos + 1),
                    Style::default().fg(t.wave_raw),
                )),
                Cell::from(Span::styled(info.display_title(), style)),
                Cell::from(Span::styled(info.artist.clone().unwrap_or_default(), sub)),
                Cell::from(Span::styled(info.album.clone().unwrap_or_default(), sub)),
                Cell::from(Span::styled(
                    info.duration.map(fmt_duration).unwrap_or_default(),
                    Style::default().fg(t.muted),
                )),
            ]);
            if pos == sel {
                row = row.style(Style::default().bg(if focused { t.raised } else { t.surface }));
            }
            row
        })
        .collect();
    let num_w = (n.to_string().len() as u16).max(2);
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(num_w),
            Constraint::Percentage(42),
            Constraint::Percentage(26),
            Constraint::Percentage(32),
            Constraint::Length(7),
        ],
    )
    .header(
        Row::new(vec!["", "#", "Title", "Artist", "Album", "Time"])
            .style(Style::default().fg(t.muted).bg(t.surface)),
    )
    .column_spacing(1);
    f.render_widget(table, body);
    drop(q);
    app.queue_sel = sel;
    app.queue_offset = offset;
}

/// Compact "up next" list beside whatever view is open.
fn draw_queue_panel(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let q = app.player.queue();
    let start = q.current().and_then(|c| q.position(c)).unwrap_or(0);
    let mut lines = vec![Line::from(Span::styled(
        " Up next",
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    ))];
    let w = inner.width as usize;
    for (k, &id) in q
        .play_order()
        .iter()
        .skip(start)
        .take(inner.height.saturating_sub(1) as usize)
        .enumerate()
    {
        let Some(e) = q.get(id) else { continue };
        let cur = k == 0 && q.current() == Some(id);
        let title = truncate(&e.info.display_title(), w.saturating_sub(4));
        let style = if cur {
            Style::default()
                .fg(t.verdigris)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(t.text)
        };
        lines.push(Line::from(vec![
            Span::styled(
                if cur { " ▶ " } else { "   " },
                Style::default().fg(t.verdigris),
            ),
            Span::styled(title, style),
        ]));
    }
    if q.is_empty() {
        lines.push(Line::from(Span::styled(
            " Nothing queued",
            Style::default().fg(t.muted),
        )));
    }
    f.render_widget(Paragraph::new(lines), inner);
}

// ----- Bottom bar -----

fn draw_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let t = &app.theme;
    let s = &app.snap;
    f.render_widget(Block::default().style(Style::default().bg(t.surface)), area);
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    let (glyph, gstyle) = match s.state {
        PlayState::Playing if s.buffering => ("…", Style::default().fg(t.copper)),
        PlayState::Playing => ("▶", Style::default().fg(t.verdigris)),
        PlayState::Paused => ("‖", Style::default().fg(t.muted)),
        PlayState::Stopped => ("■", Style::default().fg(t.wave_raw)),
    };
    let mut left = vec![Span::styled(format!(" {glyph} "), gstyle)];
    match &s.track {
        Some(info) => {
            left.push(Span::styled(
                info.display_title(),
                Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            ));
            for part in [&info.artist, &info.album].into_iter().flatten() {
                left.push(Span::styled(
                    format!(" · {part}"),
                    Style::default().fg(t.muted),
                ));
            }
        }
        None => left.push(Span::styled(
            "Nothing playing",
            Style::default().fg(t.muted),
        )),
    }
    if s.buffering {
        left.push(Span::styled("  buffering", Style::default().fg(t.copper)));
    }

    let mut right = Vec::new();
    if let Some(info) = &s.track {
        let badge = info.badge();
        if !badge.is_empty() {
            right.push(Span::styled(
                format!(" {badge} "),
                Style::default().fg(t.verdigris).bg(t.raised),
            ));
        }
    }
    if let Some(out) = &s.output {
        right.push(Span::styled(
            format!(" {out} "),
            Style::default().fg(t.muted),
        ));
    }
    let right_w: u16 = right.iter().map(|s| s.width() as u16).sum();
    let [l1, r1] = Layout::horizontal([Constraint::Min(1), Constraint::Length(right_w)]).areas(top);
    f.render_widget(Paragraph::new(Line::from(left)), l1);
    f.render_widget(Paragraph::new(Line::from(right)), r1);

    let on = Style::default().fg(t.verdigris);
    let off = Style::default().fg(t.wave_raw);
    let repeat = match s.repeat {
        Repeat::Off => Span::styled("↻ ", off),
        Repeat::All => Span::styled("↻ ", on),
        Repeat::One => Span::styled("↻1", on),
    };
    let modes = vec![
        Span::raw(" "),
        Span::styled("⇄", if s.shuffle { on } else { off }),
        Span::raw(" "),
        repeat,
        Span::raw(" "),
    ];
    let elapsed = fmt_duration(s.position);
    let remaining = s
        .duration
        .map(|d| format!("-{}", fmt_duration(d.saturating_sub(s.position))))
        .unwrap_or_else(|| "--:--".into());
    let vol_label = if s.muted {
        " mute".to_string()
    } else {
        format!(" {:>3}%", (s.volume * 100.0).round() as u32)
    };

    let [m, el, seek, rem, vol_icon, vol, vol_txt] = Layout::horizontal([
        Constraint::Length(7),
        Constraint::Length(elapsed.len() as u16 + 1),
        Constraint::Min(10),
        Constraint::Length(remaining.len() as u16 + 2),
        Constraint::Length(5),
        Constraint::Length(12),
        Constraint::Length(vol_label.len() as u16 + 1),
    ])
    .areas(bottom);
    f.render_widget(Paragraph::new(Line::from(modes)), m);
    f.render_widget(
        Paragraph::new(Span::styled(elapsed, Style::default().fg(t.muted))),
        el,
    );

    app.seek_area = seek;
    let frac = match s.duration {
        Some(d) if !d.is_zero() => (s.position.as_secs_f64() / d.as_secs_f64()).clamp(0.0, 1.0),
        _ => 0.0,
    };
    f.render_widget(
        Paragraph::new(bar_line(seek.width, frac, t.verdigris, t.wave_raw)),
        seek,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            format!(" {remaining}"),
            Style::default().fg(t.muted),
        )),
        rem,
    );

    app.volume_area = vol;
    f.render_widget(
        Paragraph::new(Span::styled(" vol ", Style::default().fg(t.muted))),
        vol_icon,
    );
    let vcolor = if s.muted { t.wave_raw } else { t.verdigris };
    f.render_widget(
        Paragraph::new(bar_line(vol.width, s.volume, vcolor, t.wave_raw)),
        vol,
    );
    f.render_widget(
        Paragraph::new(Span::styled(
            vol_label,
            Style::default().fg(if s.muted { t.copper } else { t.muted }),
        )),
        vol_txt,
    );
}

fn bar_line(width: u16, frac: f64, fill: Color, rest: Color) -> Line<'static> {
    let w = width as usize;
    let filled = ((w as f64) * frac).round() as usize;
    Line::from(vec![
        Span::styled("━".repeat(filled.min(w)), Style::default().fg(fill)),
        Span::styled(
            "─".repeat(w.saturating_sub(filled)),
            Style::default().fg(rest),
        ),
    ])
}

// ----- Overlays -----

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width);
    let h = h.min(area.height);
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

fn frame_block<'a>(app: &App, title: &'a str) -> Block<'a> {
    let t = &app.theme;
    Block::default()
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(t.text),
        ))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface))
}

fn draw_input_box(f: &mut Frame, app: &App, area: Rect, title: &str, buf: &str) {
    let t = &app.theme;
    let r = popup(area, 50, 3);
    f.render_widget(Clear, r);
    let line = Line::from(vec![
        Span::styled(format!(" {buf}"), Style::default().fg(t.text)),
        Span::styled("▏", Style::default().fg(t.verdigris)),
    ]);
    f.render_widget(Paragraph::new(line).block(frame_block(app, title)), r);
}

fn draw_pick(f: &mut Frame, app: &App, area: Rect, title: &str, names: &[String], selected: usize) {
    let t = &app.theme;
    let h = (names.len() as u16 + 2).min(16);
    let r = popup(area, 50, h);
    f.render_widget(Clear, r);
    let rows_h = h.saturating_sub(2) as usize;
    let offset = selected.saturating_sub(rows_h.saturating_sub(1));
    let lines: Vec<Line> = names
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows_h)
        .map(|(i, n)| {
            let style = if i == selected {
                Style::default().fg(t.verdigris).bg(t.raised)
            } else if i == 0 {
                Style::default().fg(t.muted)
            } else {
                Style::default().fg(t.text)
            };
            Line::from(Span::styled(format!(" {}", pad(n, 47)), style))
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(frame_block(app, title)), r);
}

fn draw_confirm(f: &mut Frame, app: &App, area: Rect, msg: &str) {
    let t = &app.theme;
    let r = popup(area, (msg.len() as u16 + 6).max(30), 4);
    f.render_widget(Clear, r);
    let lines = vec![
        Line::from(Span::styled(format!(" {msg}"), Style::default().fg(t.text))),
        Line::from(vec![
            Span::styled(" y ", Style::default().fg(t.bg).bg(t.copper)),
            Span::styled(" yes   ", Style::default().fg(t.muted)),
            Span::styled(" n ", Style::default().fg(t.bg).bg(t.verdigris_dim)),
            Span::styled(" no", Style::default().fg(t.muted)),
        ]),
    ];
    f.render_widget(Paragraph::new(lines).block(frame_block(app, "Confirm")), r);
}

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    const KEYS: &[(&str, &str)] = &[
        ("Space", "Play / pause"),
        ("n / b", "Next / back"),
        ("Left / Right", "Seek 5 s (Shift: 30 s)"),
        ("+ / -   m", "Volume, mute"),
        ("s   r", "Shuffle, cycle repeat"),
        (
            "1 to 8",
            "Folders, Artists, Albums, Songs, Genres, Playlists, Queue, Downloads",
        ),
        ("Tab", "Switch between sidebar and list"),
        ("/", "Search the library"),
        ("Enter", "Open, or play from here"),
        ("Esc / Backspace", "Back up a level"),
        ("a / A", "Add to queue / play next"),
        ("p", "Add to a playlist"),
        ("o / O", "Sort Albums or Songs / reverse Songs"),
        (
            "t",
            "Artists: album or track artist. Genre: albums or songs",
        ),
        ("q", "Queue panel"),
        (".", "Go to the playing track in Queue"),
        ("Queue", "d remove, J/K move, S save as playlist, C clear"),
        ("Playlists", "N new, R rename, D delete; inside: d, J/K"),
        ("F5", "Check folders for changes"),
        (
            "Mouse",
            "Click, double-click to play, right-click back, drag bars",
        ),
        ("Ctrl+Q", "Quit"),
    ];
    let r = popup(area, 84, KEYS.len() as u16 + 2);
    f.render_widget(Clear, r);
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<17}"), Style::default().fg(t.verdigris)),
                Span::styled(*v, Style::default().fg(t.text)),
            ])
        })
        .collect();
    f.render_widget(Paragraph::new(lines).block(frame_block(app, "Keys")), r);
}

fn pad(s: &str, w: usize) -> String {
    let len = s.chars().count();
    if len >= w {
        s.chars().take(w).collect()
    } else {
        format!("{s}{}", " ".repeat(w - len))
    }
}

fn truncate(s: &str, w: usize) -> String {
    if s.chars().count() <= w {
        s.to_string()
    } else if w <= 1 {
        s.chars().take(w).collect()
    } else {
        let mut out: String = s.chars().take(w - 1).collect();
        out.push('…');
        out
    }
}
