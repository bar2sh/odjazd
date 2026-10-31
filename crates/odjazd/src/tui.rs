//! The live board: `odjazd` with no stop opens it on a stop search, `odjazd <stop> --watch`
//! straight on that stop. Departures count down every second, with live delays worked out
//! from where the vehicles are.
//!
//! A thread of its own reads the timetable and then follows the vehicles, so the search is
//! there at once and the clock never stalls.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};

use anyhow::Result;
use jiff::{SignedDuration, Zoned, civil::Date, tz::TimeZone};
use odjazd_core::{
    Feed, Live, Route, StopGroup, Timetable, realtime::Realtime, search, timetable::Departure,
};
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph, Row, Table, Wrap},
};

use crate::{
    live,
    render::{late_by, whereabouts},
};

/// How often the vehicle positions are downloaded. Warsaw publishes new ones every 10–20
/// seconds.
const FOLLOW: Duration = Duration::from_secs(15);

/// Positions older than this no longer tell where the vehicles are.
const STALE: SignedDuration = SignedDuration::from_mins(2);

/// Departures worked out per board: more than fit on a tall terminal.
const LIMIT: usize = 40;

/// A departure stays on the board this long after its time, as "now", unless its vehicle
/// is seen leaving.
const GRACE: SignedDuration = SignedDuration::from_secs(30);

/// Stops listed at most while searching.
const MATCHES: usize = 30;

/// The amber of the LED displays at Warsaw stops, as in the README banner.
const AMBER: Color = Color::Rgb(255, 176, 0);

/// Delays, like the time a vehicle is late by, in a softer red than the terminal's.
const LATE: Color = Color::Rgb(255, 123, 114);

/// A vehicle on time or early.
const ON_TIME: Color = Color::Rgb(63, 185, 80);

/// The board never grows wider than this, so its columns stay together on a wide
/// terminal.
const WIDTH: u16 = 80;

/// Runs the live board until the user quits, opening the board of `stop` if given. With
/// `offline`, the times are the timetable's and the vehicles are not followed.
pub fn run(feed: Feed, data_dir: PathBuf, stop: Option<&str>, offline: bool) -> Result<()> {
    let groups = feed.stop_groups();
    let (news, inbox) = mpsc::channel();
    let (refresh, asked) = mpsc::channel();
    // Not joined: quitting should not wait for a download to time out.
    thread::spawn({
        let timezone = feed.timezone().clone();
        move || follow(&timezone, &data_dir, offline, &asked, &news)
    });
    let mut app = App::new(&feed, &groups, refresh, offline);
    if let Some(stop) = stop {
        app.search_for(stop);
        app.open(0);
    }
    ratatui::run(|terminal| app.run(terminal, &inbox))
}

/// What the background thread has found out.
enum News {
    /// The timetable, read for today, and again once a new day starts.
    Timetable(Arc<Timetable>),
    /// Where the vehicles are and how late, or why that is not known.
    Live(Result<Realtime, String>),
    /// The timetable cannot be read, so there is nothing to show.
    Broken(String),
}

/// Reads the timetable, then follows the vehicles: downloads their positions every
/// `FOLLOW`, or sooner when asked, until the app quits.
fn follow(
    timezone: &TimeZone,
    data_dir: &Path,
    offline: bool,
    asked: &Receiver<()>,
    news: &Sender<News>,
) {
    let mut read: Option<(Date, Arc<Timetable>)> = None;
    let mut realtime = Realtime::default();
    loop {
        let today = Zoned::now().with_time_zone(timezone.clone()).date();
        if read.as_ref().is_none_or(|&(day, _)| day != today) {
            let timetable = match crate::timetable(data_dir, today) {
                Ok(timetable) => Arc::new(timetable),
                Err(error) => {
                    let error = format!("cannot read the timetable ({error}); try `odjazd update`");
                    let _ = news.send(News::Broken(error));
                    return;
                }
            };
            realtime = Realtime::default();
            if news.send(News::Timetable(Arc::clone(&timetable))).is_err() {
                return;
            }
            read = Some((today, timetable));
        }
        if let Some((_, timetable)) = read.as_ref().filter(|_| !offline) {
            let update = live::track(timetable, &realtime).map_err(|error| format!("{error:#}"));
            if let Ok(update) = &update {
                realtime = update.clone();
            }
            if news.send(News::Live(update)).is_err() {
                return;
            }
        }
        match asked.recv_timeout(FOLLOW) {
            // Pressing `r` a few times still makes one download.
            Ok(()) => while asked.try_recv().is_ok() {},
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

struct App<'f> {
    feed: &'f Feed,
    groups: &'f [StopGroup],
    /// Asks the background thread for fresh vehicle positions.
    refresh: Sender<()>,
    offline: bool,
    timetable: Option<Arc<Timetable>>,
    /// Where the vehicles were last seen, and why the latest positions did not come.
    realtime: Option<Realtime>,
    trouble: Option<String>,
    /// Why there is no timetable.
    broken: Option<String>,
    /// The search box, the stops matching it and the one picked in the list.
    query: String,
    matches: Vec<&'f StopGroup>,
    selected: ListState,
    /// The stop on display, once one is opened.
    board: Option<Board<'f>>,
    quit: bool,
}

struct Board<'f> {
    group: &'f StopGroup,
    /// Its place in `App::matches`, so Tab can move on to the next one.
    index: usize,
}

impl<'f> App<'f> {
    fn new(feed: &'f Feed, groups: &'f [StopGroup], refresh: Sender<()>, offline: bool) -> Self {
        Self {
            feed,
            groups,
            refresh,
            offline,
            timetable: None,
            realtime: None,
            trouble: None,
            broken: None,
            query: String::new(),
            matches: Vec::new(),
            selected: ListState::default(),
            board: None,
            quit: false,
        }
    }

    fn run(&mut self, terminal: &mut DefaultTerminal, inbox: &Receiver<News>) -> Result<()> {
        while !self.quit {
            for news in inbox.try_iter() {
                self.receive(news);
            }
            let now = Zoned::now().with_time_zone(self.feed.timezone().clone());
            terminal.draw(|frame| self.draw(frame, &now))?;
            // Wake up four times a second, so the clock and the countdowns stay current.
            if event::poll(Duration::from_millis(250))?
                && let Some(key) = event::read()?.as_key_press_event()
            {
                self.key(key);
            }
        }
        Ok(())
    }

    fn receive(&mut self, news: News) {
        match news {
            News::Timetable(timetable) => {
                self.timetable = Some(timetable);
                // Matched to the timetable that was replaced.
                self.realtime = None;
            }
            News::Live(Ok(realtime)) => {
                self.realtime = Some(realtime);
                self.trouble = None;
            }
            News::Live(Err(trouble)) => self.trouble = Some(trouble),
            News::Broken(error) => self.broken = Some(error),
        }
    }

    fn search_for(&mut self, query: &str) {
        self.query = query.to_owned();
        self.find();
    }

    fn find(&mut self) {
        self.matches = search(self.groups, &self.query);
        self.matches.truncate(MATCHES);
        self.selected
            .select((!self.matches.is_empty()).then_some(0));
    }

    /// Shows the board of the `index`-th matching stop.
    fn open(&mut self, index: usize) {
        let Some(&group) = self.matches.get(index) else {
            return;
        };
        self.selected.select(Some(index));
        self.board = Some(Board { group, index });
    }

    fn key(&mut self, key: KeyEvent) {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && key.code == KeyCode::Char('c') {
            self.quit = true;
        } else if let Some(board) = &self.board {
            let index = board.index;
            let count = self.matches.len();
            match key.code {
                KeyCode::Char('q') => self.quit = true,
                KeyCode::Esc | KeyCode::Backspace | KeyCode::Char('/') => self.board = None,
                // The background thread only stops after the app does, so this arrives.
                KeyCode::Char('r') => _ = self.refresh.send(()),
                KeyCode::Tab if count > 1 => self.open((index + 1) % count),
                KeyCode::BackTab if count > 1 => self.open((index + count - 1) % count),
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Esc => self.quit = true,
                KeyCode::Enter => {
                    if let Some(index) = self.selected.selected() {
                        self.open(index.min(self.matches.len().saturating_sub(1)));
                    }
                }
                KeyCode::Up => self.selected.select_previous(),
                KeyCode::Down => self.selected.select_next(),
                KeyCode::Char('p') if ctrl => self.selected.select_previous(),
                KeyCode::Char('n') if ctrl => self.selected.select_next(),
                KeyCode::Char('u') if ctrl => self.search_for(""),
                KeyCode::Backspace => {
                    self.query.pop();
                    self.find();
                }
                KeyCode::Char(c) if !ctrl => {
                    self.query.push(c);
                    self.find();
                }
                _ => {}
            }
        }
    }

    /// The vehicle positions, unless they are too old to go by.
    fn live(&self, now: &Zoned) -> Option<&Realtime> {
        self.realtime.as_ref().filter(|realtime| {
            realtime
                .published
                .is_some_and(|published| now.timestamp().duration_since(published) < STALE)
        })
    }

    /// What the footer says about the data.
    fn status(&self, now: &Zoned) -> String {
        if self.broken.is_some() {
            return "no timetable".to_owned();
        }
        if self.timetable.is_none() {
            return "reading the timetable…".to_owned();
        }
        if self.offline {
            return "timetable only".to_owned();
        }
        match self.live(now).and_then(|realtime| realtime.published) {
            Some(published) => {
                let age = now.timestamp().duration_since(published).as_secs().max(0);
                format!("live, positions {age} s old")
            }
            None if self.trouble.is_some() => "no live data".to_owned(),
            None => "finding the vehicles…".to_owned(),
        }
    }

    fn draw(&mut self, frame: &mut Frame, now: &Zoned) {
        let [area] = Layout::horizontal([Constraint::Max(WIDTH)])
            .flex(Flex::Center)
            .areas(frame.area());
        let [header, rule, body, _, footer] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .areas(area);

        let title = match &self.board {
            Some(board) => Span::from(board.group.name.as_str()).bold(),
            None => Span::from("odjazd").bold().fg(AMBER),
        };
        frame.render_widget(Line::from(title), header);
        let clock = now.strftime("%a %-d %b  %H:%M:%S").to_string();
        // The span is dimmed, not the line: a line's style covers the whole row, title too.
        frame.render_widget(Line::from(clock.dim()).right_aligned(), header);
        frame.render_widget(Line::from("─".repeat(rule.width.into())).dim(), rule);

        let status = self.status(now);
        match &self.board {
            Some(board) => self.draw_board(frame, board, now, &status, body, footer),
            None => draw_search(
                frame,
                self.feed,
                &self.query,
                &self.matches,
                &mut self.selected,
                &status,
                body,
                footer,
            ),
        }
    }

    fn draw_board(
        &self,
        frame: &mut Frame,
        board: &Board,
        now: &Zoned,
        status: &str,
        body: Rect,
        footer: Rect,
    ) {
        if let Some(error) = &self.broken {
            let error = Paragraph::new(error.as_str())
                .red()
                .wrap(Wrap { trim: true });
            frame.render_widget(error, body);
        } else if let Some(timetable) = &self.timetable {
            let none = Realtime::default();
            let live = self.live(now).unwrap_or(&none);
            let from = now.timestamp() - GRACE;
            let departures = timetable.departures(&board.group.stops, from, LIMIT, live);
            if departures.is_empty() {
                let note = Line::from("No departures in the next day or so.").dim();
                frame.render_widget(note, body);
            } else {
                frame.render_widget(table(timetable, &departures, now), body);
            }
        } else {
            frame.render_widget(Line::from("Reading the timetable…").dim(), body);
        }

        let mut keys = vec![("esc", "search")];
        let next = self
            .matches
            .get((board.index + 1) % self.matches.len().max(1));
        if let Some(next) = next.filter(|_| self.matches.len() > 1) {
            keys.push(("tab", next.name.as_str()));
        }
        keys.extend([("r", "refresh"), ("q", "quit")]);
        draw_footer(frame, footer, &keys, status);
    }
}

/// One row per departure: the line in its own colours, where it goes, from which pole,
/// when, and in how long. Tracked vehicles also show how late they are and where.
fn table<'a>(timetable: &'a Timetable, departures: &[Departure<'a>], now: &Zoned) -> Table<'a> {
    let zone = timetable.timezone();
    let width = departures
        .iter()
        .map(|departure| departure.route.name.chars().count())
        .max()
        .unwrap_or(0);
    let tracked = departures.iter().any(|departure| departure.live.is_some());
    let rows = departures.iter().map(|departure| {
        let minutes = departure.expected.duration_since(now.timestamp()).as_mins();
        let countdown = match minutes {
            ..1 => "now".to_owned(),
            1..60 => format!("{minutes} min"),
            _ => String::new(),
        };
        let countdown = Line::from(countdown).bold().right_aligned();
        let approximate = if departure.approximate { "~" } else { "" };
        let time = departure.scheduled.to_zoned(zone.clone()).strftime("%H:%M");
        let route = &departure.route.name;
        let code = timetable.stops()[departure.stop].code.as_deref();
        let mut cells = vec![
            Line::from(Span::styled(
                format!(" {route:^width$} "),
                badge(departure.route),
            )),
            Line::from(departure.headsign),
            Line::from(code.unwrap_or_default()).dim(),
            Line::from(format!("{approximate}{time}")).right_aligned(),
        ];
        if tracked {
            cells.push(delay(departure.live));
        }
        // About to leave: in amber, like the LED boards.
        cells.push(if minutes < 2 {
            countdown.fg(AMBER)
        } else {
            countdown
        });
        if tracked {
            cells.push(Line::from(whereabouts(departure.live)).dim());
        }
        Row::new(cells)
    });
    let mut widths = vec![
        Constraint::Length(u16::try_from(width + 2).unwrap_or(u16::MAX)),
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(6),
    ];
    if tracked {
        widths.push(Constraint::Length(3));
    }
    widths.push(Constraint::Length(6));
    if tracked {
        // "10 stops away"
        widths.push(Constraint::Length(13));
    }
    Table::new(rows, widths).column_spacing(2)
}

/// How late a tracked vehicle runs, in whole minutes: `+2`, `−1`, or `●` when on time.
fn delay(live: Option<Live>) -> Line<'static> {
    let Some(live) = live else {
        return Line::default();
    };
    match late_by(live) {
        0 => Line::from("●".fg(ON_TIME)),
        minutes @ 1.. => Line::from(format!("+{minutes}").fg(LATE)),
        minutes => Line::from(format!("−{}", -minutes).fg(ON_TIME)),
    }
}

/// A line's own colours (red trams, purple buses…), or inverted text without them.
fn badge(route: &Route) -> Style {
    let rgb = |[r, g, b]: [u8; 3]| Color::Rgb(r, g, b);
    match route.color {
        Some(color) => Style::new()
            .bold()
            .bg(rgb(color))
            .fg(rgb(route.text_color.unwrap_or([255; 3]))),
        None => Style::new().bold().reversed(),
    }
}

#[expect(clippy::too_many_arguments, reason = "the parts of the app it draws")]
fn draw_search(
    frame: &mut Frame,
    feed: &Feed,
    query: &str,
    matches: &[&StopGroup],
    selected: &mut ListState,
    status: &str,
    body: Rect,
    footer: Rect,
) {
    let [input, _, list] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(body);
    frame.render_widget(Line::from(vec!["› ".bold().fg(AMBER), query.into()]), input);
    let typed = u16::try_from(query.chars().count()).unwrap_or(u16::MAX);
    frame.set_cursor_position((input.x.saturating_add(2).saturating_add(typed), input.y));

    if query.is_empty() {
        let hint = Paragraph::new(vec![
            Line::from("Type a stop name. Typos and missing Polish letters are fine:"),
            Line::from("plac narutowicza, dworzec centralny, metro politechnika…"),
            Line::default(),
            Line::from("Timetable: ZTM Warszawa. Vehicle positions: the City of Warsaw."),
        ]);
        frame.render_widget(hint.dim(), list);
    } else if matches.is_empty() {
        let note = format!("No stop called anything like “{query}”.");
        frame.render_widget(Line::from(note).dim(), list);
    } else {
        let items = matches.iter().map(|group| {
            let codes: Vec<&str> = group
                .stops
                .iter()
                .filter_map(|&stop| feed.stops()[stop].code.as_deref())
                .collect();
            ListItem::new(Line::from(vec![
                group.name.as_str().bold(),
                "  ".into(),
                codes.join(" ").dim(),
            ]))
        });
        let items = List::new(items).highlight_style(Style::new().bg(AMBER).fg(Color::Black));
        frame.render_stateful_widget(items, list, selected);
    }

    let keys = [("↑↓", "choose"), ("enter", "departures"), ("esc", "quit")];
    draw_footer(frame, footer, &keys, status);
}

/// Keys and what they do on the left, `status` on the right if there is room for it.
fn draw_footer(frame: &mut Frame, area: Rect, keys: &[(&str, &str)], status: &str) {
    let keys = Line::from_iter(
        keys.iter()
            .flat_map(|&(key, action)| [key.bold(), format!(" {action}  ").dim()]),
    );
    let status = Line::from(status.dim()).right_aligned();
    if keys.width() + status.width() <= area.width.into() {
        frame.render_widget(status, area);
    }
    frame.render_widget(keys, area);
}

#[cfg(test)]
mod film;

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Modifier};

    use super::*;
    use crate::fixtures::{FRIDAY, MONDAY, at, feed, late_tram};

    /// The app with the timetable of `day` read, as the background thread leaves it.
    fn app<'f>(feed: &'f Feed, groups: &'f [StopGroup], day: Date) -> (App<'f>, Receiver<()>) {
        let (refresh, asked) = mpsc::channel();
        let mut app = App::new(feed, groups, refresh, false);
        let timetable = Timetable::build(feed, day).unwrap();
        app.receive(News::Timetable(Arc::new(timetable)));
        (app, asked)
    }

    /// What the app draws at `now` on a `width`×`height` terminal.
    fn draw(app: &mut App, now: &Zoned, width: u16, height: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|frame| app.draw(frame, now)).unwrap();
        terminal.backend().buffer().clone()
    }

    /// The text of `screen`, one string per row.
    fn rows(screen: &Buffer) -> Vec<String> {
        screen
            .content()
            .chunks(screen.area.width.into())
            .map(|row| {
                let row: String = row.iter().map(|cell| cell.symbol()).collect();
                row.trim_end().to_owned()
            })
            .collect()
    }

    fn type_in(app: &mut App, text: &str) {
        for c in text.chars() {
            app.key(KeyCode::Char(c).into());
        }
    }

    #[test]
    fn board_counts_down_to_each_departure() {
        let feed = feed("mini");
        let groups = feed.stop_groups();
        let (mut app, _) = app(&feed, &groups, FRIDAY);
        app.offline = true;
        app.search_for("plac testowy");
        app.open(0);

        let now = at(&feed, FRIDAY, 9, 55, 12);
        let screen = draw(&mut app, &now, 56, 8);
        assert_eq!(
            rows(&screen),
            [
                "Pl. Testowy                         Fri 30 Oct  09:55:12",
                "────────────────────────────────────────────────────────",
                " 15   Końcowa                        01    10:00   4 min",
                " 15   Okrężna                        01    11:00",
                " N1   Końcowa                        02    00:05",
                "",
                "",
                "esc search  r refresh  q quit             timetable only",
            ]
        );
        // Only the clock and the status are dimmed, not the stop or the keys beside them.
        let dim = |x: u16, y: u16| screen[(x, y)].modifier.contains(Modifier::DIM);
        assert!(!dim(0, 0) && dim(55, 0));
        assert!(!dim(0, 7) && dim(55, 7));
        // Where the status does not fit beside the keys, it is left out.
        let narrow = rows(&draw(&mut app, &now, 40, 8));
        assert_eq!(narrow[7], "esc search  r refresh  q quit");
    }

    #[test]
    fn a_tracked_vehicle_shows_its_delay_and_where_it_is() {
        let feed = feed("commute");
        let groups = feed.stop_groups();
        let (mut app, _) = app(&feed, &groups, MONDAY);
        app.search_for("dom");
        app.open(0);
        let now = at(&feed, MONDAY, 7, 58, 0);
        let timetable = Arc::clone(app.timetable.as_ref().unwrap());
        app.receive(News::Live(Ok(late_tram(&timetable, now.timestamp()))));

        let screen = rows(&draw(&mut app, &now, 72, 7));
        // The tram counts down to when it will come, not to its time in the timetable.
        assert_eq!(
            screen[2..5],
            [
                "  1    Praca                     01    08:00  +2    4 min  1 stop away",
                " 100   Praca                     02    08:05        7 min",
                "  1    Praca                     01    08:10       12 min",
            ]
        );
        assert!(screen[6].ends_with("live, positions 0 s old"), "{screen:?}");
        // Positions from long ago say nothing about now.
        let later = now.checked_add(STALE).unwrap();
        let screen = rows(&draw(&mut app, &later, 72, 7));
        assert!(screen[6].ends_with("finding the vehicles…"), "{screen:?}");
    }

    #[test]
    fn typing_narrows_the_stops_and_enter_opens_one() {
        let feed = feed("mini");
        let groups = feed.stop_groups();
        let (mut app, _) = app(&feed, &groups, FRIDAY);
        type_in(&mut app, "centrm");

        let screen = rows(&draw(&mut app, &at(&feed, FRIDAY, 12, 0, 0), 40, 8));
        assert_eq!(
            screen[2..6],
            [
                "› centrm",
                "",
                "Centrum (Warszawa)  01",
                "Centrum (Łomianki)  01"
            ]
        );

        app.key(KeyCode::Down.into());
        app.key(KeyCode::Enter.into());
        assert_eq!(app.board.as_ref().unwrap().group.name, "Centrum (Łomianki)");
        // Tab moves on to the next match; Esc goes back to the search, still filled in.
        app.key(KeyCode::Tab.into());
        assert_eq!(app.board.as_ref().unwrap().group.name, "Centrum (Warszawa)");
        app.key(KeyCode::Esc.into());
        assert!(app.board.is_none());
        assert_eq!(app.query, "centrm");
    }

    #[test]
    fn r_asks_for_fresh_positions() {
        let feed = feed("mini");
        let groups = feed.stop_groups();
        let (mut app, asked) = app(&feed, &groups, FRIDAY);
        app.search_for("plac testowy");
        app.open(0);
        app.key(KeyCode::Char('r').into());
        assert!(asked.try_recv().is_ok());
    }
}
