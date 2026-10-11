//! Input handling and the frontend's own state (selection, scroll, help).
//! Playback state lives in the core; this only sends commands.

use std::time::{Duration, Instant};

use anyhow::Result;
use patina_core::{Command, Event, Player, Snapshot};
use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{
    self, Event as TermEvent, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton,
    MouseEvent, MouseEventKind,
};
use ratatui::layout::Rect;

use crate::theme::Theme;

pub struct App {
    pub player: Player,
    pub theme: Theme,
    pub snap: Snapshot,
    /// Selected row, as a position in play order.
    pub selected: usize,
    /// First visible row.
    pub offset: usize,
    pub show_help: bool,
    /// Last error and when it arrived, shown for a few seconds.
    pub toast: Option<(String, Instant)>,
    /// Keep the selection on the playing track until the user moves it.
    pub follow: bool,
    // Hit areas from the last draw, for the mouse.
    pub list_area: Rect,
    pub seek_area: Rect,
    pub volume_area: Rect,
    last_click: Option<(Instant, u16)>,
    quit: bool,
}

const TOAST_TIME: Duration = Duration::from_secs(6);

impl App {
    pub fn new(player: Player) -> Self {
        Self {
            player,
            theme: Theme::detect(),
            snap: Snapshot::default(),
            selected: 0,
            offset: 0,
            show_help: false,
            toast: None,
            follow: true,
            list_area: Rect::default(),
            seek_area: Rect::default(),
            volume_area: Rect::default(),
            last_click: None,
            quit: false,
        }
    }

    pub fn run(mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.drain_events();
            self.snap = self.player.snapshot();
            if self
                .toast
                .as_ref()
                .is_some_and(|(_, t)| t.elapsed() > TOAST_TIME)
            {
                self.toast = None;
            }
            terminal.draw(|f| crate::ui::draw(f, &mut self))?;
            if event::poll(Duration::from_millis(100))? {
                match event::read()? {
                    TermEvent::Key(k) if k.kind != KeyEventKind::Release => self.key(k),
                    TermEvent::Mouse(m) => self.mouse(m),
                    _ => {}
                }
            }
        }
        Ok(())
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.player.events().try_recv() {
            match ev {
                Event::Error(e) => self.toast = Some((e, Instant::now())),
                Event::TrackChanged(Some(entry)) if self.follow => {
                    let q = self.player.queue();
                    if let Some(p) = q.play_order().iter().position(|&i| i == entry) {
                        self.selected = p;
                    }
                }
                _ => {}
            }
        }
    }

    fn queue_len(&self) -> usize {
        self.player.queue().len()
    }

    fn entry_at(&self, pos: usize) -> Option<usize> {
        self.player.queue().play_order().get(pos).copied()
    }

    fn move_sel(&mut self, delta: isize) {
        let n = self.queue_len();
        if n == 0 {
            return;
        }
        let s = (self.selected as isize + delta).clamp(0, n as isize - 1);
        self.selected = s as usize;
        self.follow = false;
    }

    fn page(&self) -> isize {
        self.list_area.height.saturating_sub(1).max(1) as isize
    }

    fn key(&mut self, k: KeyEvent) {
        let ctrl = k.modifiers.contains(KeyModifiers::CONTROL);
        let shift = k.modifiers.contains(KeyModifiers::SHIFT);
        if self.show_help {
            // Any key closes help, and quit still quits.
            self.show_help = false;
            if !(ctrl && matches!(k.code, KeyCode::Char('q') | KeyCode::Char('c'))) {
                return;
            }
        }
        let p = &self.player;
        match k.code {
            KeyCode::Char('q') | KeyCode::Char('c') if ctrl => self.quit = true,
            KeyCode::Char(' ') => p.send(Command::TogglePause),
            KeyCode::Char('n') => p.send(Command::Next),
            KeyCode::Char('b') => p.send(Command::Back),
            KeyCode::Left => p.send(Command::SeekBy(if shift { -30.0 } else { -5.0 })),
            KeyCode::Right => p.send(Command::SeekBy(if shift { 30.0 } else { 5.0 })),
            KeyCode::Char('+') | KeyCode::Char('=') => self.nudge_volume(0.05),
            KeyCode::Char('-') | KeyCode::Char('_') => self.nudge_volume(-0.05),
            KeyCode::Char('m') => p.send(Command::ToggleMute),
            KeyCode::Char('s') => p.send(Command::ToggleShuffle),
            KeyCode::Char('r') => p.send(Command::CycleRepeat),
            KeyCode::Char('?') => self.show_help = true,
            KeyCode::Enter => {
                if let Some(e) = self.entry_at(self.selected) {
                    self.player.send(Command::PlayEntry(e));
                    self.follow = true;
                }
            }
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::PageUp => self.move_sel(-self.page()),
            KeyCode::PageDown => self.move_sel(self.page()),
            KeyCode::Home | KeyCode::Char('g') => self.move_sel(isize::MIN / 2),
            KeyCode::End | KeyCode::Char('G') => self.move_sel(isize::MAX / 2),
            KeyCode::Char('.') => {
                // Jump the selection back to what's playing.
                self.follow = true;
                if let Some(cur) = self.snap.current
                    && let Some(p) = self
                        .player
                        .queue()
                        .play_order()
                        .iter()
                        .position(|&i| i == cur)
                {
                    self.selected = p;
                }
            }
            _ => {}
        }
    }

    fn nudge_volume(&self, d: f64) {
        self.player
            .send(Command::SetVolume((self.snap.volume + d).clamp(0.0, 1.0)));
    }

    fn mouse(&mut self, m: MouseEvent) {
        let (x, y) = (m.column, m.row);
        let inside = |r: Rect| x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height;
        match m.kind {
            MouseEventKind::Down(MouseButton::Left) | MouseEventKind::Drag(MouseButton::Left)
                if inside(self.seek_area) =>
            {
                if let Some(d) = self.snap.duration {
                    let frac = (x - self.seek_area.x) as f64 / self.seek_area.width.max(1) as f64;
                    self.player
                        .send(Command::Seek(d.mul_f64(frac.clamp(0.0, 1.0))));
                }
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.volume_area) => {
                let frac = (x - self.volume_area.x) as f64 / self.volume_area.width.max(1) as f64;
                self.player.send(Command::SetVolume(frac.clamp(0.0, 1.0)));
            }
            MouseEventKind::Down(MouseButton::Left) if inside(self.list_area) => {
                // Row 0 is the header.
                if y > self.list_area.y {
                    let pos = self.offset + (y - self.list_area.y - 1) as usize;
                    if pos < self.queue_len() {
                        let double = self.last_click.is_some_and(|(t, row)| {
                            row == y && t.elapsed() < Duration::from_millis(400)
                        });
                        self.selected = pos;
                        self.follow = false;
                        if double {
                            if let Some(e) = self.entry_at(pos) {
                                self.player.send(Command::PlayEntry(e));
                                self.follow = true;
                            }
                            self.last_click = None;
                        } else {
                            self.last_click = Some((Instant::now(), y));
                        }
                    }
                }
            }
            MouseEventKind::ScrollUp if inside(self.volume_area) => self.nudge_volume(0.05),
            MouseEventKind::ScrollDown if inside(self.volume_area) => self.nudge_volume(-0.05),
            MouseEventKind::ScrollUp => self.move_sel(-3),
            MouseEventKind::ScrollDown => self.move_sel(3),
            _ => {}
        }
    }
}
