//! Drawing. Sidebar on the left, the open view in the middle, the queue
//! panel on the right when it's toggled on, and the two-line bottom bar.

use patina_core::{PlayState, Repeat};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table};

use crate::app::{App, Focus, InputPurpose, Modal};
use patina_core::browse::SideEntry;
use patina_core::browse::{Control, Item, Section, Source, fmt_duration, fmt_length};
use patina_core::remote::ServerState;

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
    // The lyrics, while they're up, take the panel's place.
    let lyrics = app.b.lyrics().is_some();
    let queue_w = if lyrics && area.width >= 90 {
        LYRICS_W.min(area.width / 3)
    } else if app.queue_panel && area.width >= 100 {
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
    if app.b.section == Section::Queue {
        draw_queue(f, app, content);
    } else {
        draw_level(f, app, content);
    }
    if queue_w > 0 && lyrics {
        draw_lyrics_panel(f, app, panel);
    } else if queue_w > 0 {
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
                InputPurpose::RenameMix { .. } => "Rename mix",
                InputPurpose::Search => "",
                InputPurpose::StationName { edit: None } => "New station: name",
                InputPurpose::StationName { .. } => "Edit station: name",
                InputPurpose::StationUrl { .. } => "Station URL (stream, .pls, .m3u or .m3u8)",
                InputPurpose::FeedUrl => "Podcast feed URL",
                InputPurpose::AddFolder => "Add a music folder",
                InputPurpose::SetValue { label, .. } => label.as_str(),
                InputPurpose::RenameServer(_) => "DLNA server: new name",
                InputPurpose::ServerName(f) if f.old.is_some() => "Server: name",
                InputPurpose::ServerName(_) => "New server: a name for it",
                InputPurpose::ServerUrl(_) => "Server address (https://music.example.com)",
                InputPurpose::ServerUser(_) => "User name (empty if you'll give an API key)",
                InputPurpose::ServerSecret(f) if f.old.is_some() => {
                    "Password or API key (empty keeps the saved one)"
                }
                InputPurpose::ServerSecret(_) => "Password or API key",
                InputPurpose::ServerPlaylist { .. } => "New playlist on the server",
                InputPurpose::EqPresetName => "Save the EQ as a preset called",
                InputPurpose::AmpPresetName => "Save the amp stack as a preset called",
                InputPurpose::EqImport => "AutoEq or APO file (ParametricEQ.txt, GraphicEQ.txt)",
            };
            // Passwords show as dots.
            let shown = if matches!(purpose, InputPurpose::ServerSecret(_)) {
                "•".repeat(buf.chars().count())
            } else {
                buf.clone()
            };
            draw_input_box(f, app, area, title, &shown);
        }
        Modal::PickPlaylist {
            title,
            servers,
            rows,
            selected,
            ..
        } => {
            let names: Vec<String> = std::iter::once("New playlist…".to_string())
                .chain(servers.iter().map(|s| format!("New playlist on {s}…")))
                .chain(rows.iter().map(|r| match &r.server {
                    Some(s) => format!("{}  ({}, on {s})", r.name, r.items),
                    None => format!("{}  ({})", r.name, r.items),
                }))
                .collect();
            draw_pick(f, app, area, title, &names, *selected, true);
        }
        Modal::PickDlna { found, selected } => {
            let names: Vec<String> = found.iter().map(|f| f.label()).collect();
            let title = if found.len() == 1 {
                "Found a DLNA server. Enter adds it"
            } else {
                "Found DLNA servers. Enter adds one"
            };
            draw_pick(f, app, area, title, &names, *selected, false);
        }
        Modal::PickChoice {
            title,
            options,
            selected,
            ..
        } => draw_pick(f, app, area, title, options, *selected, false),
        Modal::PickRate {
            title, selected, ..
        } => {
            let opts: Vec<String> = (0..=5u8)
                .map(|n| {
                    if n == 0 {
                        "☆☆☆☆☆  no rating".to_string()
                    } else {
                        patina_core::browse::marks::stars_text(n)
                    }
                })
                .collect();
            draw_pick(f, app, area, title, &opts, *selected, false)
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
    let status = app.b.service.as_ref().map(|s| s.status());
    let focused = app.focus == Focus::Sidebar;
    let mut lines = Vec::new();
    for (i, e) in app.b.sidebar.iter().enumerate() {
        let sel = i == app.b.sidebar_sel;
        let base = if sel && focused {
            Style::default().bg(t.raised)
        } else {
            Style::default()
        };
        let line = match e {
            SideEntry::Section(s) => {
                let num = s.key().map(|c| c.to_string()).unwrap_or_else(|| " ".into());
                let active = *s == app.b.section;
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
            SideEntry::Source {
                path,
                tracks,
                server: Some(state),
                ..
            } => {
                let name = patina_core::remote::server_of(path).unwrap_or("server");
                let (dot, dot_color) = match state {
                    ServerState::Ready => ("●", t.verdigris),
                    ServerState::Syncing { .. } | ServerState::Connecting => ("◌", t.verdigris),
                    _ => ("●", t.copper),
                };
                let right = match state {
                    ServerState::Syncing { done, total } if *total > 0 => format!("{done}/{total}"),
                    ServerState::NoPassword | ServerState::BadLogin => "login".into(),
                    ServerState::Offline(_) => "offline".into(),
                    _ => tracks.to_string(),
                };
                let w = area.width as usize;
                let name_w = w.saturating_sub(right.len() + 5);
                Line::from(vec![
                    Span::styled(format!(" {dot} "), base.fg(dot_color)),
                    Span::styled(pad(&truncate(name, name_w), name_w), base.fg(t.text)),
                    Span::styled(format!(" {right} "), base.fg(t.muted)),
                ])
            }
            SideEntry::Source {
                path,
                tracks,
                present,
                server: None,
            } => {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string());
                let scanning = status
                    .as_ref()
                    .and_then(|s| s.scanning.as_ref())
                    .filter(|sc| sc.root == *path && sc.total > 0);
                let (dot, dot_color) = if !present {
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
            SideEntry::Mix { name, .. } => {
                let playing = app
                    .b
                    .mix
                    .as_ref()
                    .and_then(|m| m.saved)
                    .is_some_and(|id| matches!(e, SideEntry::Mix { id: x, .. } if *x == id));
                let w = (area.width as usize).saturating_sub(3);
                Line::from(vec![
                    Span::styled(" ∞ ", base.fg(t.verdigris)),
                    Span::styled(
                        pad(&truncate(name, w), w),
                        if playing {
                            base.fg(t.verdigris).add_modifier(Modifier::BOLD)
                        } else {
                            base.fg(t.text)
                        },
                    ),
                ])
            }
            SideEntry::Favorite { label, .. } => {
                let w = (area.width as usize).saturating_sub(3);
                Line::from(vec![
                    Span::styled(" ★ ", base.fg(t.copper)),
                    Span::styled(pad(&truncate(label, w), w), base.fg(t.text)),
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

/// A title with its rating after it, in copper: "Blew  ★★★".
fn titled<'a>(title: &str, style: Style, rating: u8, t: &crate::theme::Theme) -> Line<'a> {
    let mut v = vec![Span::styled(title.to_string(), style)];
    if rating > 0 {
        v.push(Span::styled(
            format!("  {}", "★".repeat(rating as usize)),
            Style::default().fg(t.copper),
        ));
    }
    Line::from(v)
}

/// What a row's columns show, which depends on the list it's in.
#[derive(Clone, Copy, PartialEq)]
enum RowCtx {
    Plain,
    /// Inside an album: track numbers instead of list positions.
    Album,
    /// A listening-stats list: plays in the last column.
    Ranked,
}

fn plays(n: i64) -> String {
    format!("{n} play{}", s(n))
}

/// What columns a level uses, from the kind of rows in it.
#[derive(PartialEq)]
enum Kind {
    Tracks,
    Albums,
    Names,
    Info,
    Stations,
    Episodes,
    Downloads,
}

fn level_kind(items: &[Item]) -> Kind {
    if items
        .iter()
        .any(|i| matches!(i, Item::Job(_) | Item::Pinned(_) | Item::Action { .. }))
    {
        return Kind::Downloads;
    }
    if items
        .iter()
        .any(|i| matches!(i, Item::Track(_) | Item::Missing { .. }))
        && !items
            .iter()
            .any(|i| matches!(i, Item::Album(_) | Item::Artist { .. }))
    {
        Kind::Tracks
    } else if items
        .iter()
        .all(|i| matches!(i, Item::Info { .. } | Item::Setting(_) | Item::Header(_)))
        && items
            .iter()
            .any(|i| matches!(i, Item::Info { .. } | Item::Setting(_)))
    {
        Kind::Info
    } else if items.iter().all(|i| matches!(i, Item::Station { .. })) {
        Kind::Stations
    } else if items.iter().any(|i| matches!(i, Item::Episode(_)))
        && items
            .iter()
            .all(|i| matches!(i, Item::Episode(_) | Item::Header(_) | Item::Info { .. }))
    {
        Kind::Episodes
    } else if items.iter().all(|i| matches!(i, Item::Album(_))) && !items.is_empty() {
        Kind::Albums
    } else {
        Kind::Names
    }
}

fn hints(app: &App) -> &'static str {
    let src = app.b.level().map(|l| &l.source);
    match (app.b.section, src) {
        (_, Some(Source::Search(_))) => "Enter open · a add · A play next · Esc back",
        (Section::Albums, Some(Source::Section(_))) => {
            "o sort · v play all · V shuffle · e queue all · P playlist all"
        }
        (Section::Songs, _) => {
            "o sort · O reverse · v play all · V shuffle · e queue all · P playlist all"
        }
        (Section::Artists, Some(Source::Section(_))) => {
            "t album/track artist · a add · X artist mix"
        }
        (_, Some(Source::Genre { .. })) => "t albums/songs · a add · X genre mix · Esc back",
        (Section::Genres, _) => "Enter open · M mark for a mix · X play the mix of marked genres",
        (_, Some(Source::Playlist { .. })) => {
            "v play all · e queue all · d remove · J/K move · Esc back"
        }
        (Section::Playlists, _) => "N new · R rename · D delete · Enter on a mix plays it",
        (Section::Radio, _) => "Enter play · N new · E edit · T test · D delete · J/K move",
        (_, Some(Source::Podcast { .. })) => {
            "Enter play · w download · x delete file · P played · [ ] speed · z trim"
        }
        (Section::Podcasts, _) => "Enter open · N subscribe · D unsubscribe · F5 refresh",
        (Section::Downloads, _) => {
            "Enter open · w pause · x cancel or unpin · W pause all · C clear cache"
        }
        (Section::Settings, _) => {
            "Enter change (or sync a server) · h/l step · N add folder · E edit server · d remove"
        }
        (Section::Eq, _) => {
            "Enter change · h/l ±0.5 dB · H/L move a band · o on/off · d remove a band · Ctrl+E here"
        }
        (Section::Amp, _) => "Enter change · h/l turn a knob · o rack on/off · Ctrl+A here",
        (Section::MostListened | Section::TopAlbums | Section::TopArtists, _) => {
            "Enter open · v play all · V shuffle · e queue all · P playlist all"
        }
        _ => "v play all · V shuffle · e queue all · a add · p playlist · f keep offline · X mix",
    }
}

fn title_line<'a>(app: &'a App, width: u16, right: &'a str) -> Line<'a> {
    let t = &app.theme;
    let crumbs = app.b.crumbs();
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
    let Some(level) = app.b.stacks.get(&app.b.section).and_then(|s| s.last()) else {
        let msg = app
            .b
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
            Source::Section(Section::Radio) => {
                "No stations yet. Press N to add one: a name, then its stream or playlist URL."
                    .to_string()
            }
            Source::Section(Section::Podcasts) => {
                "No podcasts yet. Press N and paste a feed URL.".to_string()
            }
            Source::Section(Section::MostListened | Section::TopAlbums | Section::TopArtists) => {
                "Nothing played yet. A song counts once you've heard half of it, \
                 or four minutes of it, whichever comes first."
                    .to_string()
            }
            Source::Podcast { .. } => {
                if app.b.pod.as_ref().is_some_and(|p| p.status().refreshing) {
                    "Fetching the feed…".to_string()
                } else {
                    "No episodes with audio in this feed.".to_string()
                }
            }
            _ => {
                let scanning = app.b.service.as_ref().is_some_and(|s| !s.status().ready);
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
    let ctx = match &level.source {
        Source::Album { .. } => RowCtx::Album,
        Source::Section(s) if Section::STATS.contains(s) => RowCtx::Ranked,
        _ => RowCtx::Plain,
    };
    let ranked = ctx == RowCtx::Ranked;
    let header = match kind {
        Kind::Tracks => vec!["", "#", "Title", "Artist", "Album", "Time"],
        Kind::Albums if ranked => vec!["", "", "Album", "Artist", "Year", "Plays"],
        Kind::Albums => vec!["", "", "Album", "Artist", "Year", "Length"],
        Kind::Names if ranked => vec!["", "", "Name", "", "", "Plays"],
        Kind::Names => vec!["", "", "Name", "", "", ""],
        Kind::Info => vec!["", "", "", "", "", ""],
        Kind::Stations => vec!["", "", "Station", "Last test", "", ""],
        Kind::Episodes => vec!["", "", "Episode", "Published", "Progress", "Length"],
        Kind::Downloads => vec!["", "", "", "", "", ""],
    };
    // The EQ view: its response curve above the rows.
    let body = if matches!(level.source, Source::Section(Section::Eq)) && body.height > 16 {
        let [curve, rest] =
            Layout::vertical([Constraint::Length(10), Constraint::Min(0)]).areas(body);
        draw_eq_curve(f, app, curve);
        rest
    } else {
        body
    };
    // In Settings, the selected option's help sits under the list.
    let help = match level.items.get(level.selected) {
        Some(Item::Setting(st)) if st.disabled.is_some() => st.disabled,
        Some(Item::Setting(st)) if !st.help.is_empty() => Some(st.help),
        _ => None,
    };
    let body = if let Some(h) = help {
        let [list, line] =
            Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(body);
        f.render_widget(
            Paragraph::new(Span::styled(format!(" {h}"), Style::default().fg(t.muted))),
            line,
        );
        list
    } else {
        body
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
    let focused = app.focus == Focus::Content;
    let rows: Vec<Row> = level
        .items
        .iter()
        .enumerate()
        .skip(offset)
        .take(rows_h)
        .map(|(i, item)| {
            let mut row = item_row(app, item, i, ctx, playing.as_deref());
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
            Constraint::Length(24),
            Constraint::Min(10),
            Constraint::Length(0),
            Constraint::Length(0),
        ],
        Kind::Stations => [
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Percentage(40),
            Constraint::Percentage(60),
            Constraint::Length(0),
            Constraint::Length(0),
        ],
        Kind::Episodes => [
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Min(20),
            Constraint::Length(11),
            Constraint::Length(16),
            Constraint::Length(8),
        ],
        Kind::Downloads => [
            Constraint::Length(1),
            Constraint::Length(0),
            Constraint::Percentage(50),
            Constraint::Percentage(50),
            Constraint::Length(0),
            Constraint::Length(0),
        ],
    };
    let table = Table::new(rows, widths)
        .header(Row::new(header).style(Style::default().fg(t.muted).bg(t.surface)))
        .column_spacing(1);
    f.render_widget(table, body);

    if let Some(l) = app
        .b
        .stacks
        .get_mut(&app.b.section)
        .and_then(|s| s.last_mut())
    {
        l.offset = offset;
    }
}

fn item_row<'a>(
    app: &App,
    item: &'a Item,
    i: usize,
    ctx: RowCtx,
    playing: Option<&std::path::Path>,
) -> Row<'a> {
    let t = &app.theme;
    let dim = Style::default().fg(t.wave_raw);
    // Its server can't be reached and none of it is kept: greyed, not gone.
    let off = !app.b.plays(item);
    let (text, muted) = if off {
        (dim, dim)
    } else {
        (Style::default().fg(t.text), Style::default().fg(t.muted))
    };
    // Pinned for offline: a mark where the play arrow goes.
    let mark = |c: &'static str| -> Cell<'static> {
        if app.b.pinned(item).is_some() {
            Cell::from(Span::styled("●", Style::default().fg(t.verdigris)))
        } else {
            Cell::from(Span::styled(c, dim))
        }
    };
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
            let num = if ctx == RowCtx::Album {
                tr.track_no.map(|n| n.to_string()).unwrap_or_default()
            } else {
                (i + 1).to_string()
            };
            vec![
                if now {
                    Cell::from(Span::styled("▶", Style::default().fg(t.verdigris)))
                } else {
                    mark(" ")
                },
                Cell::from(Span::styled(num, dim)),
                Cell::from(titled(&tr.title, ts, tr.rating, t)),
                Cell::from(Span::styled(tr.artist.clone().unwrap_or_default(), ss)),
                Cell::from(Span::styled(tr.album.clone().unwrap_or_default(), ss)),
                Cell::from(Span::styled(
                    if ctx == RowCtx::Ranked {
                        plays(tr.play_count)
                    } else {
                        tr.duration.map(fmt_duration).unwrap_or_default()
                    },
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
            mark(" "),
            Cell::from(""),
            Cell::from(titled(&a.name, text, a.rating, t)),
            Cell::from(Span::styled(a.artist_line(), muted)),
            Cell::from(Span::styled(
                a.year.map(|y| y.to_string()).unwrap_or_default(),
                muted,
            )),
            Cell::from(Span::styled(
                if ctx == RowCtx::Ranked {
                    plays(a.play_count)
                } else {
                    format!("{} · {}", a.track_count, fmt_length(a.duration))
                },
                muted,
            )),
        ],
        Item::Artist(a) => vec![
            mark(" "),
            Cell::from(""),
            Cell::from(Span::styled(a.name.clone(), text)),
            Cell::from(Span::styled(
                format!("{} album{}", a.albums, s(a.albums)),
                muted,
            )),
            Cell::from(""),
            Cell::from(Span::styled(
                if ctx == RowCtx::Ranked {
                    plays(a.plays)
                } else {
                    format!("{} track{}", a.tracks, s(a.tracks))
                },
                muted,
            )),
        ],
        Item::Dir { name, .. } => vec![
            mark("▸"),
            Cell::from(""),
            Cell::from(Span::styled(name.clone(), text)),
            Cell::from(""),
            Cell::from(""),
            Cell::from(""),
        ],
        Item::Root { path, tracks } => vec![
            mark("▸"),
            Cell::from(""),
            Cell::from(Span::styled(
                match patina_core::remote::server_of(path) {
                    Some(name) => format!("{name} (server)"),
                    None => path.display().to_string(),
                },
                text,
            )),
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(format!("{tracks} track{}", s(*tracks)), muted)),
        ],
        Item::Genre(g) => vec![
            if app.b.genre_marked(&g.norm) {
                Cell::from(Span::styled("✓", Style::default().fg(t.verdigris)))
            } else {
                Cell::from(" ")
            },
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
            mark(" "),
            Cell::from(""),
            Cell::from(Span::styled(p.name.clone(), text)),
            Cell::from(Span::styled(
                match (&p.server, p.readonly) {
                    (Some(s), true) => format!("on {s}, read-only"),
                    (Some(s), false) => format!("on {s}"),
                    _ => String::new(),
                },
                muted,
            )),
            Cell::from(""),
            Cell::from(Span::styled(
                format!("{} track{}", p.items, s(p.items)),
                muted,
            )),
        ],
        Item::Station { station, check, .. } => {
            let now = app
                .snap
                .track
                .as_ref()
                .is_some_and(|t| t.is_live() && t.url.as_deref() == Some(station.url.as_str()));
            let style = if now {
                Style::default()
                    .fg(t.verdigris)
                    .add_modifier(Modifier::BOLD)
            } else {
                text
            };
            let (status, st) = match check {
                Some(c) if c.starts_with("Not working") || c.contains("no stream answered") => {
                    (c.clone(), Style::default().fg(t.copper))
                }
                Some(c) => (c.clone(), muted),
                None => (station.url.clone(), dim),
            };
            vec![
                Cell::from(Span::styled(
                    if now { "▶" } else { " " },
                    Style::default().fg(t.verdigris),
                )),
                Cell::from(""),
                Cell::from(Span::styled(station.name.clone(), style)),
                Cell::from(Span::styled(status, st)),
                Cell::from(""),
                Cell::from(""),
            ]
        }
        Item::Podcast(p) => {
            let detail = match (&p.last_error, p.episodes) {
                (Some(e), 0) => {
                    Span::styled(format!("feed error: {e}"), Style::default().fg(t.copper))
                }
                (_, 0) => Span::styled("not fetched yet", dim),
                (_, n) => Span::styled(format!("{} new of {n}", p.unplayed), muted),
            };
            vec![
                Cell::from(" "),
                Cell::from(""),
                Cell::from(Span::styled(p.title.clone(), text)),
                Cell::from(Span::styled(p.author.clone().unwrap_or_default(), muted)),
                Cell::from(""),
                Cell::from(detail),
            ]
        }
        Item::Episode(e) => {
            let now = app.snap.track.as_ref().and_then(|t| t.episode) == Some(e.id);
            use patina_core::cache::EpisodeState;
            let dl = app.b.episode_download(e.id);
            let downloading = matches!(dl, Some(EpisodeState::Downloading { .. }));
            let date = e.published.and_then(chrono_date).unwrap_or_default();
            let progress = if let Some(EpisodeState::Downloading { done, total }) = dl {
                match total {
                    Some(t) if t > 0 => format!("↓ {}%", done * 100 / t),
                    _ => format!("↓ {} MB", done / 1_000_000),
                }
            } else if dl == Some(EpisodeState::Paused) {
                "↓ paused".into()
            } else if dl.is_some() {
                "↓ waiting".into()
            } else if e.played {
                "played".into()
            } else if e.position > std::time::Duration::from_secs(10) {
                match e.duration {
                    Some(d) if d > e.position => format!("{} left", fmt_duration(d - e.position)),
                    _ => format!("at {}", fmt_duration(e.position)),
                }
            } else {
                "new".into()
            };
            let style = if now {
                Style::default()
                    .fg(t.verdigris)
                    .add_modifier(Modifier::BOLD)
            } else if e.played {
                muted
            } else {
                text
            };
            vec![
                Cell::from(Span::styled(
                    if now {
                        "▶"
                    } else if e.download.is_some() {
                        "●"
                    } else {
                        " "
                    },
                    Style::default().fg(t.verdigris),
                )),
                Cell::from(""),
                Cell::from(Span::styled(e.title.clone(), style)),
                Cell::from(Span::styled(date, muted)),
                Cell::from(Span::styled(
                    progress,
                    if downloading {
                        Style::default().fg(t.verdigris)
                    } else {
                        muted
                    },
                )),
                Cell::from(Span::styled(
                    e.duration.map(fmt_duration).unwrap_or_default(),
                    muted,
                )),
            ]
        }
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
        Item::Mix(m) => vec![
            mark("∞"),
            Cell::from(""),
            Cell::from(Span::styled(m.name.clone(), text)),
            Cell::from(Span::styled(format!("{} mix", capital(&m.kind)), muted)),
            Cell::from(""),
            Cell::from(Span::styled(
                if m.kind == "genre" { "" } else { "endless" },
                muted,
            )),
        ],
        Item::Pinned(p) => {
            let all = p.stored >= p.songs;
            vec![
                Cell::from(Span::styled(
                    if all { "●" } else { "◐" },
                    Style::default().fg(t.verdigris),
                )),
                Cell::from(""),
                Cell::from(Span::styled(p.pin.name.clone(), text)),
                Cell::from(Span::styled(
                    format!(
                        "{} · {} of {} song{} · {}",
                        p.pin.kind.label(),
                        p.stored,
                        p.songs,
                        s(p.songs as i64),
                        patina_core::browse::fmt_bytes(p.bytes)
                    ),
                    muted,
                )),
                Cell::from(""),
                Cell::from(""),
            ]
        }
        Item::Job(j) => {
            let st = if j.error.is_some() {
                Style::default().fg(t.copper)
            } else if j.active {
                Style::default().fg(t.verdigris)
            } else {
                muted
            };
            vec![
                Cell::from(Span::styled(
                    if j.active { "↓" } else { " " },
                    Style::default().fg(t.verdigris),
                )),
                Cell::from(""),
                Cell::from(Span::styled(j.label.clone(), text)),
                Cell::from(Span::styled(
                    format!("{} · {}", j.kind.label(), j.state()),
                    st,
                )),
                Cell::from(""),
                Cell::from(""),
            ]
        }
        Item::Action { label, .. } => vec![
            Cell::from(""),
            Cell::from(""),
            Cell::from(Span::styled(
                label.clone(),
                Style::default().fg(t.verdigris),
            )),
            Cell::from(Span::styled("Enter", dim)),
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
        Item::Setting(st) => {
            let accent = Style::default().fg(t.verdigris);
            let (label, value) = match &st.control {
                Control::Folder => (
                    Span::styled("Folder", muted),
                    Span::styled(st.label.clone(), text),
                ),
                Control::Toggle(on) => (
                    Span::styled(st.label.clone(), text),
                    Span::styled(
                        if *on { "[x] on" } else { "[ ] off" },
                        if *on { accent } else { muted },
                    ),
                ),
                // The amp's knobs as sliders.
                Control::Number {
                    value, min, max, ..
                } if matches!(
                    st.key,
                    patina_core::browse::Key::AmpParam(..)
                        | patina_core::browse::Key::AmpTrimIn(_)
                        | patina_core::browse::Key::AmpTrimOut(_)
                ) =>
                {
                    let w = 14usize;
                    let pos = if max > min {
                        ((value - min) as f64 / (max - min) as f64).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let fill = (pos * w as f64).round() as usize;
                    (
                        Span::styled(format!("  {}", st.label), text),
                        Span::styled(
                            format!(
                                "{}{} {}",
                                "━".repeat(fill),
                                "─".repeat(w - fill),
                                st.control.display()
                            ),
                            accent,
                        ),
                    )
                }
                Control::Choice { .. } | Control::Number { .. } => (
                    Span::styled(st.label.clone(), text),
                    Span::styled(format!("‹ {} ›", st.control.display()), text),
                ),
                Control::Action(a) => (
                    Span::styled(st.label.clone(), text),
                    Span::styled(
                        if st.key == patina_core::browse::Key::AddFolder {
                            format!("Enter or N: {}", lower_first(a))
                        } else {
                            format!("Enter: {}", lower_first(a))
                        },
                        accent,
                    ),
                ),
                Control::Text { value, .. } => (
                    Span::styled(st.label.clone(), text),
                    Span::styled(
                        st.control.display(),
                        if value.is_empty() { muted } else { text },
                    ),
                ),
                Control::Info(v) => (
                    Span::styled(st.label.clone(), muted),
                    Span::styled(v.clone(), muted),
                ),
                Control::Server(v) => (
                    Span::styled(st.label.clone(), accent),
                    Span::styled(v.clone(), text),
                ),
            };
            // Greyed out (leveling under bit-perfect): same row, all muted.
            let (label, value) = if st.disabled.is_some() {
                (
                    Span::styled(label.content, muted),
                    Span::styled(value.content, muted),
                )
            } else {
                (label, value)
            };
            vec![
                Cell::from(""),
                Cell::from(""),
                Cell::from(label),
                Cell::from(value),
                Cell::from(""),
                Cell::from(""),
            ]
        }
    };
    Row::new(cells)
}

/// The EQ's response, 20 Hz to 20 kHz on a log axis, with a dot at each
/// band. Braille dots, so it's smooth enough to read in a terminal.
fn draw_eq_curve(f: &mut Frame, app: &App, area: Rect) {
    use ratatui::widgets::canvas::{Canvas, Line as CLine, Points};
    let t = &app.theme;
    let eq = app.b.eq_settings();
    let pts = eq.curve(160);
    let bands = eq.bands();
    let top = pts
        .iter()
        .map(|p| p.1.abs())
        .chain(
            bands
                .iter()
                .filter(|b| b.kind.has_gain())
                .map(|b| b.gain.abs()),
        )
        .fold(12.0f64, f64::max)
        .ceil();
    let lx = |hz: f64| hz.max(20.0).log10();
    let on = eq.on;
    let curve_color = if on { t.verdigris } else { t.wave_raw };
    let title = format!(
        " {} · {} · preamp {:+.1} dB ",
        if on { "on" } else { "bypassed" },
        app.b.eq.preset_label(),
        eq.total_preamp()
    );
    let canvas = Canvas::default()
        .block(
            Block::default()
                .borders(Borders::BOTTOM)
                .border_style(Style::default().fg(t.border))
                .title(Span::styled(title, Style::default().fg(t.muted))),
        )
        .x_bounds([lx(20.0), lx(20000.0)])
        .y_bounds([-top, top])
        .marker(ratatui::symbols::Marker::Braille)
        .paint(move |ctx| {
            for hz in [100.0, 1000.0, 10000.0] {
                ctx.draw(&CLine {
                    x1: lx(hz),
                    y1: -top,
                    x2: lx(hz),
                    y2: top,
                    color: t.border,
                });
            }
            ctx.draw(&CLine {
                x1: lx(20.0),
                y1: 0.0,
                x2: lx(20000.0),
                y2: 0.0,
                color: t.wave_raw,
            });
            for w in pts.windows(2) {
                ctx.draw(&CLine {
                    x1: lx(w[0].0),
                    y1: w[0].1,
                    x2: lx(w[1].0),
                    y2: w[1].1,
                    color: curve_color,
                });
            }
            let dots: Vec<(f64, f64)> = bands
                .iter()
                .filter(|b| b.freq >= 20.0 && b.freq <= 20000.0)
                .map(|b| (lx(b.freq), if b.kind.has_gain() { b.gain } else { 0.0 }))
                .collect();
            ctx.draw(&Points {
                coords: &dots,
                color: t.copper,
            });
            ctx.print(
                lx(100.0),
                -top,
                Span::styled("100", Style::default().fg(t.muted)),
            );
            ctx.print(
                lx(1000.0),
                -top,
                Span::styled("1k", Style::default().fg(t.muted)),
            );
            ctx.print(
                lx(10000.0),
                -top,
                Span::styled("10k", Style::default().fg(t.muted)),
            );
            ctx.print(
                lx(20.0),
                top,
                Span::styled(format!("+{top}"), Style::default().fg(t.muted)),
            );
        });
    f.render_widget(canvas, area);
}

/// "Use my usual EQ here" as "use my usual EQ here": only the first
/// letter, so names and abbreviations keep their capitals.
fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => String::new(),
    }
}

fn s(n: i64) -> &'static str {
    if n == 1 { "" } else { "s" }
}

// ----- Queue -----

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = area;
    let [title_a, body] = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(area);
    let t = &app.theme;
    let q = app.b.player.queue();
    let n = q.len();
    let total: std::time::Duration = q
        .play_order()
        .iter()
        .filter_map(|&id| q.get(id).and_then(|e| e.info.duration))
        .sum();
    let title = match (n, app.b.mix_title()) {
        (0, _) => "Queue".to_string(),
        (_, Some(mix)) => format!("Queue · {mix} · {n} tracks so far"),
        _ => format!(
            "Queue · {n} track{} · {}",
            if n == 1 { "" } else { "s" },
            fmt_length(total)
        ),
    };
    let hint = if app.b.mix.as_ref().is_some_and(|m| m.saved.is_none()) {
        "Enter play · d remove · J/K move · S add to playlist · M save the mix · C clear"
    } else {
        "Enter play · d remove · J/K move · S add to playlist · C clear"
    };
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
            } else if !app.b.avail.info_plays(info) {
                // Its server is away and it isn't kept: it'll be skipped.
                style = style.fg(t.wave_raw);
                sub = sub.fg(t.wave_raw);
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
const LYRICS_W: u16 = 48;

fn draw_lyrics_panel(f: &mut Frame, app: &mut App, area: Rect) {
    use patina_core::browse::marks::LyricsStatus;
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let stale = app.b.lyrics_stale();
    let Some(v) = app.b.lyrics() else { return };
    let w = inner.width.saturating_sub(2) as usize;
    let mut lines = vec![Line::from(Span::styled(
        format!(" {}", truncate(&v.title, w)),
        Style::default().fg(t.text).add_modifier(Modifier::BOLD),
    ))];
    let note = |s: String, c| Line::from(Span::styled(format!(" {s}"), Style::default().fg(c)));
    if stale {
        lines.push(note(
            "Another song is playing. Ctrl+L looks it up.".into(),
            t.copper,
        ));
    }
    match &v.status {
        LyricsStatus::Looking => lines.push(note("Looking…".into(), t.muted)),
        LyricsStatus::Nothing(why) => lines.push(note(why.clone(), t.muted)),
        LyricsStatus::Failed(e) => lines.push(note(format!("Couldn't look: {e}"), t.copper)),
        LyricsStatus::Found(l) => {
            lines.push(note(l.origin.label().to_string(), t.wave_raw));
            lines.push(Line::from(""));
            let cur = if stale {
                None
            } else {
                l.line_at(app.snap.position)
            };
            let room = inner.height.saturating_sub(lines.len() as u16) as usize;
            // Keep the playing line a third of the way down.
            let first = cur.map(|c| c.saturating_sub(room / 3)).unwrap_or(0);
            for (i, (_, text)) in l.lines.iter().enumerate().skip(first).take(room) {
                let style = if Some(i) == cur {
                    Style::default()
                        .fg(t.verdigris)
                        .add_modifier(Modifier::BOLD)
                } else if l.synced && cur.is_some_and(|c| i < c) {
                    Style::default().fg(t.muted)
                } else {
                    Style::default().fg(t.text)
                };
                lines.push(Line::from(Span::styled(
                    format!(" {}", truncate(text, w)),
                    style,
                )));
            }
        }
    }
    // Notes can run long; lyric lines are cut to fit already.
    f.render_widget(
        Paragraph::new(lines).wrap(ratatui::widgets::Wrap { trim: false }),
        inner,
    );
}

fn draw_queue_panel(f: &mut Frame, app: &mut App, area: Rect) {
    let t = &app.theme;
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface));
    let mut inner = block.inner(area);
    f.render_widget(block, area);
    // Cover art on top, about square: terminal cells are roughly twice as
    // tall as they are wide.
    if app.art.enabled() && inner.height > 12 {
        let art_h = ((inner.width.saturating_sub(2)) / 2).min(inner.height / 2);
        let art_area = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), art_h);
        if let Some(img) = app.art.image.as_mut() {
            f.render_stateful_widget(crate::art::ArtView::widget(), art_area, img);
        }
        inner = Rect::new(
            inner.x,
            inner.y + art_h + 1,
            inner.width,
            inner.height - art_h - 1,
        );
    }
    let t = &app.theme;
    let q = app.b.player.queue();
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
    match app.b.loved_now {
        Some(true) => left.push(Span::styled("♥ ", Style::default().fg(t.copper))),
        Some(false) => left.push(Span::styled("♡ ", Style::default().fg(t.wave_raw))),
        None => {}
    }
    if let Some(r) = app.b.playing_rating() {
        left.push(Span::styled(
            format!("{} ", patina_core::browse::marks::stars_text(r)),
            Style::default().fg(if r > 0 { t.copper } else { t.wave_raw }),
        ));
    }
    match &s.track {
        Some(info) => {
            left.push(Span::styled(
                info.display_title(),
                Style::default().fg(t.text).add_modifier(Modifier::BOLD),
            ));
            if info.is_live() {
                // Station name, then what's on.
                if let Some(now) = &s.live_title {
                    left.push(Span::styled(
                        format!(" · {now}"),
                        Style::default().fg(t.text),
                    ));
                }
            } else {
                for part in [&info.artist, &info.album].into_iter().flatten() {
                    left.push(Span::styled(
                        format!(" · {part}"),
                        Style::default().fg(t.muted),
                    ));
                }
            }
        }
        None => left.push(Span::styled(
            "Nothing playing",
            Style::default().fg(t.muted),
        )),
    }
    if let Some(r) = &s.reconnect {
        let secs =
            r.at.saturating_duration_since(std::time::Instant::now())
                .as_secs();
        left.push(Span::styled(
            format!(
                "  connection lost, retrying in {secs} s (attempt {})",
                r.attempt
            ),
            Style::default().fg(t.copper),
        ));
    } else if s.buffering {
        left.push(Span::styled("  buffering", Style::default().fg(t.copper)));
    }

    let mut right = Vec::new();
    if let Some(info) = s
        .track
        .as_ref()
        .filter(|i| i.kind == patina_core::track::MediaKind::Podcast)
        && ((s.podcast_tempo - 1.0).abs() > 0.01 || s.podcast_trim)
    {
        let mut tag = format!(" {:.1}x", s.podcast_tempo);
        if s.podcast_trim {
            tag.push_str(" trim");
        }
        tag.push(' ');
        let _ = info;
        right.push(Span::styled(tag, Style::default().fg(t.copper)));
        right.push(Span::raw(" "));
    }
    match (&s.exclusive, &s.bit_perfect_refused) {
        (Some(e), _) => {
            let (label, fg) = if e.exact {
                (" BIT-PERFECT ", t.verdigris)
            } else {
                (" EXCLUSIVE ", t.muted)
            };
            right.push(Span::styled(label, Style::default().fg(fg).bg(t.raised)));
            right.push(Span::raw(" "));
        }
        (None, Some(_)) => {
            right.push(Span::styled(
                " NOT BIT-PERFECT ",
                Style::default().fg(t.copper).bg(t.raised),
            ));
            right.push(Span::raw(" "));
        }
        _ => {}
    }
    // The limiter at work, in copper: how much it's taking off.
    if s.limiting_db >= 0.1 {
        right.push(Span::styled(
            format!(" LIM -{:.1} dB ", s.limiting_db),
            Style::default().fg(t.copper).bg(t.raised),
        ));
        right.push(Span::raw(" "));
    }
    // The EQ, lit when it's on and doing something.
    {
        let eq = app.b.eq_settings();
        let lit = eq.on && !eq.is_flat();
        right.push(Span::styled(
            " EQ ",
            Style::default()
                .fg(if lit { t.verdigris } else { t.wave_raw })
                .bg(t.raised),
        ));
        right.push(Span::raw(" "));
    }
    // The amp stack, lit when it's on and a device in it is.
    {
        let lit = app.b.amp_settings().changes_sound();
        right.push(Span::styled(
            " AMP ",
            Style::default()
                .fg(if lit { t.copper } else { t.wave_raw })
                .bg(t.raised),
        ));
        right.push(Span::raw(" "));
    }
    // The leveling gain on what's playing: "-4.2 dB", "-6.0 dB?" while it's
    // a guess, "~-9.1 dB" under the live normalizer.
    if let Some(l) = s.level {
        let txt = l.short();
        if !txt.is_empty() {
            right.push(Span::styled(
                format!(" {txt} "),
                Style::default().fg(t.muted).bg(t.raised),
            ));
            right.push(Span::raw(" "));
        }
    }
    if let Some(info) = &s.track {
        let badge = info.badge();
        if !badge.is_empty() {
            right.push(Span::styled(
                format!(" {badge} "),
                Style::default().fg(t.verdigris).bg(t.raised),
            ));
        }
    }
    // Resampled: from and to, in copper, since the source rate isn't
    // what's playing.
    if let Some(r) = s.resample {
        right.push(Span::styled(
            format!(" {} ", patina_core::browse::settings::resample_badge(r)),
            Style::default().fg(t.copper).bg(t.raised),
        ));
        right.push(Span::raw(" "));
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
    let remaining = match s.duration {
        Some(d) => format!("-{}", fmt_duration(d.saturating_sub(s.position))),
        None if s.live => String::new(),
        None => "--:--".into(),
    };
    let vol_label = if s.muted {
        " mute".to_string()
    } else if s.volume_locked {
        // Bit-perfect on a DAC with no volume control.
        " 100% fixed".to_string()
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
    if s.live {
        // No length and no seeking: a plain label instead of a bar.
        f.render_widget(
            Paragraph::new(Span::styled(" live", Style::default().fg(t.verdigris))),
            seek,
        );
    } else {
        f.render_widget(
            Paragraph::new(bar_line(seek.width, frac, t.verdigris, t.wave_raw)),
            seek,
        );
    }
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

/// A list to pick from. `first_dim` greys the first row, for "New…".
fn draw_pick(
    f: &mut Frame,
    app: &App,
    area: Rect,
    title: &str,
    names: &[String],
    selected: usize,
    first_dim: bool,
) {
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
            } else if i == 0 && first_dim {
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
            "1 to 9, 0",
            "Folders, Artists, Albums, Songs, Genres, Playlists, Radio, Podcasts, Queue, Downloads",
        ),
        ("Tab", "Switch between sidebar and list"),
        ("/", "Search the library"),
        ("Enter", "Open, or play from here"),
        ("v / V", "Play the whole view in order / shuffled"),
        ("e", "Add the whole view to the queue"),
        ("P", "Add the whole view to a playlist"),
        ("Esc / Backspace", "Back up a level"),
        ("a / A", "Add to queue / play next"),
        ("p", "Add to a playlist"),
        (
            "f / F",
            "Keep the selection / the whole view offline, or stop",
        ),
        (
            "X",
            "Mix from the selection: an artist, album, song, or the marked genres",
        ),
        (
            "M",
            "Genres: mark one for a genre mix. Queue: save the mix playing",
        ),
        ("o / O", "Sort Albums or Songs / reverse Songs"),
        (
            "t",
            "Artists: album or track artist. Genre: albums or songs",
        ),
        ("[ ]   z", "Podcast speed, silence trimming"),
        ("q", "Queue panel"),
        (".", "Go to the playing track in Queue"),
        (
            "Queue",
            "d remove, J/K move, S add the queue to a playlist, L load the queue saved on your server, C clear",
        ),
        ("Playlists", "N new, R rename, D delete; inside: d, J/K"),
        ("Radio", "N new, E edit, T test, D delete, J/K move"),
        (
            "Settings",
            "Enter change, h/l step, N add folder, d remove folder or server; on a server Enter syncs, E edits",
        ),
        (
            "Podcasts",
            "N subscribe, D unsubscribe; episodes: w download, x delete, P played",
        ),
        (
            "Downloads",
            "w pause or resume, x cancel or unpin, W pause all, C clear the cache",
        ),
        (
            "EQ (Ctrl+E)",
            "Enter change, h/l ±0.5 dB, H/L move a band, o on/off, d remove a band",
        ),
        (
            "Amp (Ctrl+A)",
            "Enter change, h/l turn a knob, o rack on/off; add, move and remove devices from their rows",
        ),
        ("Ctrl+O", "Pick the output device"),
        (
            "Ctrl+L",
            "Lyrics for the song playing (looked up only then)",
        ),
        (
            "* / y / Y",
            "Star a folder, album or artist; rate the selection; rate the song playing",
        ),
        ("F5", "Check folders and podcast feeds for changes"),
        (
            "Mouse",
            "Click, double-click to play, right-click back, drag bars",
        ),
        (
            "l",
            "Love the song playing (Last.fm, and a star on its server if set)",
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

/// "artist" to "Artist".
fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
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

fn chrono_date(ts: i64) -> Option<String> {
    let days = ts.div_euclid(86_400);
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm),
    // to avoid pulling a date crate into the TUI for one column.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    Some(format!("{y:04}-{m:02}-{d:02}"))
}

#[cfg(test)]
mod tests {
    #[test]
    fn dates() {
        assert_eq!(super::chrono_date(0).unwrap(), "1970-01-01");
        assert_eq!(super::chrono_date(1_791_021_600).unwrap(), "2026-10-03");
    }
}
