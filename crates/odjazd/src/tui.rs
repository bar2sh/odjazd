//! The live board: `odjazd` with no stop opens it on a stop search, `odjazd <stop> --live`
//! straight on that stop. Departures count down every second and refresh themselves.
//!
//! Reading the timetable takes a moment, so queries run on a worker thread and the clock
//! keeps ticking meanwhile.

use std::{
    ptr,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::Duration,
};

use anyhow::Result;
use jiff::{SignedDuration, Timestamp, Zoned};
use odjazd_core::{Departure, Feed, Route, StopGroup, search};
use ratatui::{
    DefaultTerminal, Frame,
    crossterm::event::{self, KeyCode, KeyEvent, KeyModifiers},
    layout::{Constraint, Flex, Layout, Rect},
    style::{Color, Style, Stylize},
    text::{Line, Span},
    widgets::{List, ListItem, ListState, Paragraph, Row, Table, Wrap},
};

/// How often a board asks the timetable again.
const REFRESH: SignedDuration = SignedDuration::from_secs(30);

/// Departures per query: more than fit on a tall terminal.
const LIMIT: usize = 40;

/// A departure stays on the board this long after its time, as "now".
const GRACE: SignedDuration = SignedDuration::from_secs(30);

/// Stops listed at most while searching.
const MATCHES: usize = 30;

/// The amber of the LED displays at Warsaw stops, as in the README banner.
const AMBER: Color = Color::Rgb(255, 176, 0);

/// The board never grows wider than this, so its columns stay together on a wide
/// terminal.
const WIDTH: u16 = 80;

/// Runs the live board until the user quits, opening the board of `stop` if given.
pub fn run(feed: &Feed, stop: Option<&str>) -> Result<()> {
    let groups = feed.stop_groups();
    thread::scope(|scope| {
        let (requests, inbox) = mpsc::channel();
        let (outbox, answers) = mpsc::channel();
        scope.spawn(move || worker(feed, &inbox, &outbox));
        let mut app = App::new(feed, &groups, requests);
        if let Some(stop) = stop {
            app.search_for(stop);
            app.open(0);
        }
        ratatui::run(|terminal| app.run(terminal, &answers))
    })
}

/// Answers departure queries until the app quits. When the user flicks through stops
/// faster than the timetable is read, only the stop they ended up on is looked up.
fn worker<'f>(feed: &'f Feed, inbox: &Receiver<&'f StopGroup>, outbox: &Sender<Answer<'f>>) {
    while let Ok(group) = inbox.recv() {
        let group = inbox.try_iter().last().unwrap_or(group);
        let now = Zoned::now().with_time_zone(feed.timezone().clone());
        let departures = feed
            .departures(&group.stops, &now, LIMIT)
            .map_err(|error| format!("{:#}", anyhow::Error::from(error)));
        let answer = Answer {
            group,
            at: now.timestamp(),
            departures,
        };
        if outbox.send(answer).is_err() {
            return;
        }
    }
}

/// The worker's reply: departures from `group` as of `at`, or why there are none.
struct Answer<'f> {
    group: &'f StopGroup,
    at: Timestamp,
    departures: Result<Vec<Departure<'f>>, String>,
}

struct App<'f> {
    feed: &'f Feed,
    groups: &'f [StopGroup],
    requests: Sender<&'f StopGroup>,
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
    departures: Vec<Departure<'f>>,
    error: Option<String>,
    /// When the departures on display were looked up, and whether a query is running now.
    answered: Option<Timestamp>,
    waiting: bool,
}

impl<'f> App<'f> {
    fn new(feed: &'f Feed, groups: &'f [StopGroup], requests: Sender<&'f StopGroup>) -> Self {
        Self {
            feed,
            groups,
            requests,
            query: String::new(),
            matches: Vec::new(),
            selected: ListState::default(),
            board: None,
            quit: false,
        }
    }

    fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        answers: &Receiver<Answer<'f>>,
    ) -> Result<()> {
        while !self.quit {
            for answer in answers.try_iter() {
                self.receive(answer);
            }
            let now = Zoned::now().with_time_zone(self.feed.timezone().clone());
            self.refresh_if_stale(&now);
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
        self.board = Some(Board {
            group,
            index,
            departures: Vec::new(),
            error: None,
            answered: None,
            waiting: false,
        });
        self.ask();
    }

    /// Asks again once the departures on display are `REFRESH` old.
    fn refresh_if_stale(&mut self, now: &Zoned) {
        let answered = self.board.as_ref().and_then(|board| board.answered);
        if answered.is_some_and(|at| now.timestamp().duration_since(at) >= REFRESH) {
            self.ask();
        }
    }

    /// Asks the worker for fresh departures, unless it is already on it.
    fn ask(&mut self) {
        if let Some(board) = &mut self.board
            && !board.waiting
        {
            board.waiting = true;
            // The worker only stops after the app does, so the request always arrives.
            let _ = self.requests.send(board.group);
        }
    }

    fn receive(&mut self, answer: Answer<'f>) {
        // An answer for a stop the user has already left is of no use.
        let Some(board) = self
            .board
            .as_mut()
            .filter(|board| ptr::eq(board.group, answer.group))
        else {
            return;
        };
        board.waiting = false;
        board.answered = Some(answer.at);
        match answer.departures {
            Ok(departures) => {
                board.departures = departures;
                board.error = None;
            }
            Err(error) => board.error = Some(error),
        }
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
                KeyCode::Char('r') => self.ask(),
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

        match &self.board {
            Some(board) => draw_board(frame, board, &self.matches, now, body, footer),
            None => draw_search(
                frame,
                self.feed,
                &self.query,
                &self.matches,
                &mut self.selected,
                body,
                footer,
            ),
        }
    }
}

fn draw_board(
    frame: &mut Frame,
    board: &Board,
    matches: &[&StopGroup],
    now: &Zoned,
    body: Rect,
    footer: Rect,
) {
    let cutoff = now.timestamp() - GRACE;
    let departures: Vec<&Departure> = board
        .departures
        .iter()
        .filter(|departure| departure.time.timestamp() >= cutoff)
        .collect();
    if let Some(error) = &board.error {
        let error = Paragraph::new(error.as_str())
            .red()
            .wrap(Wrap { trim: true });
        frame.render_widget(error, body);
    } else if departures.is_empty() {
        let note = if board.waiting {
            "Reading the timetable…"
        } else {
            "No departures in the next day or so."
        };
        frame.render_widget(Line::from(note).dim(), body);
    } else {
        frame.render_widget(table(&departures, now), body);
    }

    let mut keys = vec![("esc", "search")];
    let next = matches.get((board.index + 1) % matches.len().max(1));
    if let Some(next) = next.filter(|_| matches.len() > 1) {
        keys.push(("tab", next.name.as_str()));
    }
    keys.extend([("r", "refresh"), ("q", "quit")]);
    let status = match board.answered {
        _ if board.waiting => "updating…".to_owned(),
        Some(at) => {
            let age = now.timestamp().duration_since(at).as_secs().max(0);
            format!("updated {age} s ago")
        }
        None => String::new(),
    };
    draw_footer(frame, footer, &keys, &status);
}

/// One row per departure: the line in its own colours, where it goes, from which pole,
/// when, and in how long.
fn table<'a>(departures: &[&'a Departure], now: &Zoned) -> Table<'a> {
    let width = departures
        .iter()
        .map(|departure| departure.route.name.chars().count())
        .max()
        .unwrap_or(0);
    let rows = departures.iter().map(|departure| {
        let minutes = departure.time.duration_since(now).as_mins();
        let countdown = match minutes {
            ..1 => "now".to_owned(),
            1..60 => format!("{minutes} min"),
            _ => String::new(),
        };
        let countdown = Line::from(countdown).bold().right_aligned();
        let approximate = if departure.approximate { "~" } else { "" };
        let time = format!("{approximate}{}", departure.time.strftime("%H:%M"));
        let route = &departure.route.name;
        Row::new([
            Line::from(Span::styled(
                format!(" {route:^width$} "),
                badge(departure.route),
            )),
            Line::from(departure.headsign.as_str()),
            Line::from(departure.stop.code.as_deref().unwrap_or_default()).dim(),
            Line::from(time).right_aligned(),
            // About to leave: in amber, like the LED boards.
            if minutes < 2 {
                countdown.fg(AMBER)
            } else {
                countdown
            },
        ])
    });
    let widths = [
        Constraint::Length(u16::try_from(width + 2).unwrap_or(u16::MAX)),
        Constraint::Fill(1),
        Constraint::Length(3),
        Constraint::Length(6),
        Constraint::Length(6),
    ];
    Table::new(rows, widths).column_spacing(2)
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

fn draw_search(
    frame: &mut Frame,
    feed: &Feed,
    query: &str,
    matches: &[&StopGroup],
    selected: &mut ListState,
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
    draw_footer(frame, footer, &keys, "data: ZTM Warszawa");
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
mod tests {
    use jiff::civil::date;
    use ratatui::{Terminal, backend::TestBackend, buffer::Buffer, style::Modifier};

    use super::*;

    fn feed() -> Feed {
        Feed::open(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../odjazd-core/tests/fixtures/mini"
        ))
        .unwrap()
    }

    fn friday(feed: &Feed, hour: i8, minute: i8) -> Zoned {
        date(2026, 10, 30)
            .at(hour, minute, 0, 0)
            .to_zoned(feed.timezone().clone())
            .unwrap()
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
        let feed = feed();
        let groups = feed.stop_groups();
        let (requests, inbox) = mpsc::channel();
        let mut app = App::new(&feed, &groups, requests);
        app.search_for("plac testowy");
        app.open(0);

        // Opening a stop asks the worker for its departures; here we answer ourselves.
        let group = inbox.try_recv().unwrap();
        let now = friday(&feed, 9, 55);
        let departures = feed.departures(&group.stops, &now, LIMIT).unwrap();
        app.receive(Answer {
            group,
            at: now.timestamp(),
            departures: Ok(departures),
        });

        let later = now.checked_add(SignedDuration::from_secs(12)).unwrap();
        let screen = draw(&mut app, &later, 56, 8);
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
                "esc search  r refresh  q quit           updated 12 s ago",
            ]
        );
        // Only the clock and the status are dimmed, not the stop or the keys beside them.
        let dim = |x: u16, y: u16| screen[(x, y)].modifier.contains(Modifier::DIM);
        assert!(!dim(0, 0) && dim(55, 0));
        assert!(!dim(0, 7) && dim(55, 7));
        // Where the status does not fit beside the keys, it is left out.
        let narrow = rows(&draw(&mut app, &later, 40, 8));
        assert_eq!(narrow[7], "esc search  r refresh  q quit");
    }

    #[test]
    fn a_stale_board_asks_again() {
        let feed = feed();
        let groups = feed.stop_groups();
        let (requests, inbox) = mpsc::channel();
        let mut app = App::new(&feed, &groups, requests);
        app.search_for("plac testowy");
        app.open(0);
        let group = inbox.try_recv().unwrap();
        let now = friday(&feed, 9, 55);
        app.receive(Answer {
            group,
            at: now.timestamp(),
            departures: Ok(Vec::new()),
        });

        app.refresh_if_stale(&now.checked_add(SignedDuration::from_secs(29)).unwrap());
        assert!(inbox.try_recv().is_err());
        app.refresh_if_stale(&now.checked_add(REFRESH).unwrap());
        assert_eq!(inbox.try_recv().unwrap().name, "Pl. Testowy");
        // Until the answer comes, the board does not ask again.
        app.refresh_if_stale(&now.checked_add(REFRESH * 2).unwrap());
        assert!(inbox.try_recv().is_err());
    }

    #[test]
    fn typing_narrows_the_stops_and_enter_opens_one() {
        let feed = feed();
        let groups = feed.stop_groups();
        let (requests, inbox) = mpsc::channel();
        let mut app = App::new(&feed, &groups, requests);
        type_in(&mut app, "centrm");

        let screen = rows(&draw(&mut app, &friday(&feed, 12, 0), 40, 8));
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
        assert_eq!(inbox.try_recv().unwrap().name, "Centrum (Łomianki)");
        // Tab moves on to the next match; Esc goes back to the search, still filled in.
        app.key(KeyCode::Tab.into());
        assert_eq!(inbox.try_recv().unwrap().name, "Centrum (Warszawa)");
        app.key(KeyCode::Esc.into());
        assert!(app.board.is_none());
        assert_eq!(app.query, "centrm");
    }
}
