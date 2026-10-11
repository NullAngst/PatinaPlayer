//! Drawing. Queue in the middle, two-line bottom bar, help overlay.

use std::time::Duration;

use patina_core::{PlayState, Repeat};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Cell, Clear, Paragraph, Row, Table};

use crate::app::App;

pub fn draw(f: &mut Frame, app: &mut App) {
    let area = f.area();
    f.render_widget(
        Block::default().style(Style::default().bg(app.theme.bg).fg(app.theme.text)),
        area,
    );

    let toast_h = if app.toast.is_some() { 1 } else { 0 };
    let [list, toast, bar] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(toast_h),
        Constraint::Length(2),
    ])
    .areas(area);

    draw_queue(f, app, list);
    let t = &app.theme;
    if let Some((msg, _)) = &app.toast {
        let p = Paragraph::new(Line::from(vec![
            Span::styled(" ! ", Style::default().fg(t.bg).bg(t.copper)),
            Span::styled(format!(" {msg}"), Style::default().fg(t.copper)),
        ]))
        .style(Style::default().bg(t.surface));
        f.render_widget(p, toast);
    }
    draw_bar(f, app, bar);
    if app.show_help {
        draw_help(f, app, area);
    }
}

fn fmt_time(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

fn draw_queue(f: &mut Frame, app: &mut App, area: Rect) {
    app.list_area = area;
    let t = &app.theme;
    let q = app.player.queue();
    let order = q.play_order();
    let n = order.len();
    let rows_h = area.height.saturating_sub(1) as usize;

    if n == 0 {
        f.render_widget(
            Paragraph::new(Span::styled(
                "  Queue is empty",
                Style::default().fg(t.muted),
            )),
            area,
        );
        return;
    }

    // Keep the selection on screen with a little margin.
    app.selected = app.selected.min(n - 1);
    let margin = 2.min(rows_h / 4);
    if app.selected < app.offset + margin {
        app.offset = app.selected.saturating_sub(margin);
    } else if rows_h > 0 && app.selected + margin >= app.offset + rows_h {
        app.offset = (app.selected + margin + 1).saturating_sub(rows_h);
    }
    app.offset = app.offset.min(n.saturating_sub(rows_h));

    let current = q.current();
    let rows: Vec<Row> = order
        .iter()
        .enumerate()
        .skip(app.offset)
        .take(rows_h)
        .map(|(pos, &idx)| {
            let e = &q.entries()[idx];
            let info = &e.info;
            let is_cur = current == Some(idx);
            let marker = match (is_cur, app.snap.state) {
                (true, PlayState::Playing) => "▶",
                (true, PlayState::Paused) => "‖",
                (true, PlayState::Stopped) => "■",
                _ => " ",
            };
            let mut style = Style::default().fg(t.text);
            let mut sub = Style::default().fg(t.muted);
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
                    info.duration.map(fmt_time).unwrap_or_default(),
                    Style::default().fg(t.muted),
                )),
            ]);
            if pos == app.selected {
                row = row.style(Style::default().bg(t.raised));
            }
            row
        })
        .collect();

    let header = Row::new(vec!["", "#", "Title", "Artist", "Album", "Time"])
        .style(Style::default().fg(t.muted).bg(t.surface));
    let num_w = (n.to_string().len() as u16).max(2);
    let table = Table::new(
        rows,
        [
            Constraint::Length(1),
            Constraint::Length(num_w),
            Constraint::Percentage(45),
            Constraint::Percentage(25),
            Constraint::Percentage(30),
            Constraint::Length(7),
        ],
    )
    .header(header)
    .column_spacing(1);
    f.render_widget(table, area);
}

fn draw_bar(f: &mut Frame, app: &mut App, area: Rect) {
    let t = &app.theme;
    let s = &app.snap;
    f.render_widget(Block::default().style(Style::default().bg(t.surface)), area);
    let [top, bottom] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);

    // Line 1: what's playing, then the format badge and device.
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

    // Line 2: shuffle, repeat, seek bar, volume.
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
    let elapsed = fmt_time(s.position);
    let remaining = s
        .duration
        .map(|d| format!("-{}", fmt_time(d.saturating_sub(s.position))))
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

fn bar_line(
    width: u16,
    frac: f64,
    fill: ratatui::style::Color,
    rest: ratatui::style::Color,
) -> Line<'static> {
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

fn draw_help(f: &mut Frame, app: &App, area: Rect) {
    let t = &app.theme;
    const KEYS: &[(&str, &str)] = &[
        ("Space", "Play / pause"),
        ("n / b", "Next / back"),
        ("Left / Right", "Seek 5 s"),
        ("Shift+Left / Right", "Seek 30 s"),
        ("+ / -", "Volume"),
        ("m", "Mute"),
        ("s", "Shuffle"),
        ("r", "Cycle repeat"),
        ("Up / Down, j / k", "Move selection"),
        ("PgUp / PgDn, g / G", "Page, top, bottom"),
        ("Enter", "Play selection"),
        (".", "Jump to playing track"),
        (
            "Mouse",
            "Click rows, double-click plays, click or drag the bars",
        ),
        ("?", "This help"),
        ("Ctrl+Q", "Quit"),
    ];
    let w = 64.min(area.width);
    let h = (KEYS.len() as u16 + 4).min(area.height);
    let r = Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    );
    f.render_widget(Clear, r);
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(k, v)| {
            Line::from(vec![
                Span::styled(format!(" {k:<20}"), Style::default().fg(t.verdigris)),
                Span::styled(*v, Style::default().fg(t.text)),
            ])
        })
        .collect();
    let block = Block::default()
        .title(Span::styled(" Keys ", Style::default().fg(t.text)))
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(t.border))
        .style(Style::default().bg(t.surface));
    f.render_widget(Paragraph::new(lines).block(block), r);
}
