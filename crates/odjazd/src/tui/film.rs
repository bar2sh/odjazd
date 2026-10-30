//! The animation in the README, `docs/tui.svg`: the live board as the app itself draws it,
//! from the downloaded Warsaw timetable. Each frame is drawn on a test terminal and becomes
//! SVG, and CSS steps from frame to frame, so GitHub plays it without scripts. After
//! `odjazd update`, regenerate it with
//!
//! ```sh
//! cargo test -p odjazd --release -- --ignored --nocapture readme_animation
//! ```
//!
//! The film keeps a clock of its own: rush hour on a weekday morning, ticking in real time
//! while a stop is looked up, then fast-forwarding so departures count down and leave.

use std::{
    collections::HashMap,
    fmt::Write as _,
    fs,
    sync::mpsc::{self, Receiver},
    time::Instant,
};

use anyhow::Result;
use jiff::{SignedDuration, Zoned, civil::Weekday};
use odjazd_core::{Feed, StopGroup};
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::{Buffer, Cell},
    crossterm::event::KeyCode,
    layout::Position,
    style::{Color, Modifier},
};

use super::{Answer, App, LIMIT, WIDTH};

/// Height of the terminal the film is shot on; it is as wide as the board gets.
const ROWS: u16 = 16;

// Drawn like docs/demo.svg, the animation of a one-off board next to it in the README.
const FONT: &str =
    r#"ui-monospace, SFMono-Regular, "SF Mono", Menlo, Consolas, "Liberation Mono", monospace"#;
const CHAR: f64 = 9.05; // width of one character cell
const LINE: f64 = 23.0; // line height
const PAD: f64 = 22.0; // padding inside the window
const BAR: f64 = 36.0; // title bar height
const FG: [u8; 3] = [0xe6, 0xed, 0xf3];
const BG: [u8; 3] = [0x0d, 0x11, 0x17];
const DIM: [u8; 3] = [0x7d, 0x85, 0x90];

#[test]
#[ignore = "needs the timetable from `odjazd update`; rewrites docs/tui.svg"]
fn readme_animation() -> Result<()> {
    let feed = Feed::open(crate::data_dir()?.join("feed"))?;
    let groups = feed.stop_groups();
    let (requests, inbox) = mpsc::channel();
    let mut film = Film {
        feed: &feed,
        app: App::new(&feed, &groups, requests),
        inbox,
        now: rush_hour(&feed)?,
        shots: Vec::new(),
    };

    // The stop search, empty at first, then a name typed in.
    film.wait(1200);
    for c in "centrum".chars() {
        film.app.key(KeyCode::Char(c).into());
        film.wait(130);
    }
    film.wait(1000);

    // Enter opens the board. It waits for the timetable as long as reading it really takes.
    film.app.key(KeyCode::Enter.into());
    let group = film.inbox.try_recv()?;
    let asked = film.now.clone();
    let reading = Instant::now();
    let departures = feed.departures(&group.stops, &asked, LIMIT)?;
    let took = i64::try_from(reading.elapsed().as_millis())?;
    film.wait((took + 50) / 100 * 100);
    film.app.receive(Answer {
        group,
        at: asked.timestamp(),
        departures: Ok(departures),
    });

    // The board in real time, then faster and faster.
    film.wait(1500);
    let still = film.shots.len() - 1;
    film.wait(3500);
    for (steps, millis, passes) in [(3, 300, 1), (3, 200, 2), (30, 200, 5)] {
        for _ in 0..steps {
            film.fast_forward(millis, SignedDuration::from_secs(passes))?;
        }
    }
    film.wait(2500);

    let label = "odjazd with no stop: a stop search, then the live board of Centrum in Warsaw, \
        counting down to each departure";
    let svg = svg(&film.shots, still, label);
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/tui.svg");
    fs::write(path, &svg)?;
    println!(
        "wrote docs/tui.svg: {} frames, {:.1} s, {} kB; the timetable took {took} ms",
        film.shots.len(),
        film.shots.iter().map(|shot| shot.millis).sum::<i64>() as f64 / 1000.0,
        svg.len() / 1000,
    );
    Ok(())
}

/// 8:14:52 this morning, or on the next weekday: the board at its busiest.
fn rush_hour(feed: &Feed) -> Result<Zoned> {
    let zone = feed.timezone().clone();
    let mut day = Zoned::now().with_time_zone(zone.clone()).date();
    while matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday) {
        day = day.tomorrow()?;
    }
    Ok(day.at(8, 14, 52, 0).to_zoned(zone)?)
}

/// Plays the user at the keyboard and the worker thread, on a clock of its own, and keeps
/// every frame the app draws.
struct Film<'f> {
    feed: &'f Feed,
    app: App<'f>,
    inbox: Receiver<&'f StopGroup>,
    now: Zoned,
    shots: Vec<Shot>,
}

/// A frame, and how long it stays up.
struct Shot {
    buffer: Buffer,
    cursor: Option<Position>,
    millis: i64,
}

impl Film<'_> {
    /// Lets time go by as it would: the frame changes as the clock ticks over each second.
    fn wait(&mut self, millis: i64) {
        let mut left = millis;
        while left > 0 {
            let into_second = i64::from(self.now.subsec_nanosecond()) / 1_000_000;
            let step = left.min(1000 - into_second);
            self.shoot(step, SignedDuration::from_millis(step));
            left -= step;
        }
    }

    /// Lets `passes` go by in `millis`. At that speed a refresh takes no time at all, so
    /// the board's queries are answered at once.
    fn fast_forward(&mut self, millis: i64, passes: SignedDuration) -> Result<()> {
        self.app.refresh_if_stale(&self.now);
        while let Ok(group) = self.inbox.try_recv() {
            let departures = self.feed.departures(&group.stops, &self.now, LIMIT)?;
            self.app.receive(Answer {
                group,
                at: self.now.timestamp(),
                departures: Ok(departures),
            });
        }
        self.shoot(millis, passes);
        Ok(())
    }

    /// Draws the app as it is now, keeps the frame up for `millis` and moves the clock on
    /// by `passes`.
    fn shoot(&mut self, millis: i64, passes: SignedDuration) {
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, ROWS)).unwrap();
        terminal
            .draw(|frame| self.app.draw(frame, &self.now))
            .unwrap();
        let backend = terminal.backend();
        let cursor = backend.cursor_visible().then(|| backend.cursor_position());
        let buffer = backend.buffer().clone();
        match self.shots.last_mut() {
            // Nothing changed on screen, so the last frame just stays up longer.
            Some(last) if last.buffer == buffer && last.cursor == cursor => last.millis += millis,
            _ => self.shots.push(Shot {
                buffer,
                cursor,
                millis,
            }),
        }
        self.now = &self.now + passes;
    }
}

/// The film as one SVG. Each element is drawn once and shown by CSS in the frames it is in;
/// without motion, the `still` frame stays up.
fn svg(shots: &[Shot], still: usize, label: &str) -> String {
    let mut starts = Vec::with_capacity(shots.len());
    let mut total = 0;
    for shot in shots {
        starts.push(total);
        total += shot.millis;
    }

    // Every element, with the frames it is in, layer by layer: backgrounds, text, cursor.
    let mut layers: [Vec<(String, Vec<usize>)>; 3] = Default::default();
    let mut seen = HashMap::new();
    for (frame, shot) in shots.iter().enumerate() {
        for (layer, element) in elements(shot) {
            let index = *seen.entry((layer, element.clone())).or_insert_with(|| {
                layers[layer].push((element, Vec::new()));
                layers[layer].len() - 1
            });
            layers[layer][index].1.push(frame);
        }
    }

    let mut css = format!(
        ".f{{opacity:0;animation:{total}ms step-end infinite}}.s{{opacity:1}}\
        @media (prefers-reduced-motion:reduce){{.f{{animation:none!important}}}}"
    );
    let mut body = String::new();
    let mut animations = HashMap::new();
    for layer in layers {
        // Elements that come and go together are grouped, to share one animation.
        let mut groups: Vec<(Vec<usize>, String)> = Vec::new();
        let mut group_of = HashMap::new();
        for (element, frames) in layer {
            let index = *group_of.entry(frames.clone()).or_insert_with(|| {
                groups.push((frames, String::new()));
                groups.len() - 1
            });
            groups[index].1.push_str(&element);
        }
        for (frames, elements) in groups {
            if frames.len() == shots.len() {
                body.push_str(&elements);
                body.push('\n');
                continue;
            }
            let next = animations.len();
            let animation = *animations.entry(frames.clone()).or_insert(next);
            if animation == next {
                write!(css, "@keyframes k{animation}{{").unwrap();
                let mut shown = None;
                for (frame, &start) in starts.iter().enumerate() {
                    let show = frames.binary_search(&frame).is_ok();
                    if shown != Some(show) {
                        let at = percent(start, total);
                        write!(css, "{at}%{{opacity:{}}}", u8::from(show)).unwrap();
                        shown = Some(show);
                    }
                }
                css.push('}');
            }
            let class = if frames.binary_search(&still).is_ok() {
                "f s"
            } else {
                "f"
            };
            writeln!(
                body,
                r#"<g class="{class}" style="animation-name:k{animation}">{elements}</g>"#
            )
            .unwrap();
        }
    }

    let width = 2.0 * PAD + f64::from(WIDTH) * CHAR;
    let height = baseline(usize::from(ROWS) - 1) + PAD;
    let (w, h) = (px(width), px(height));
    let label = escape(label).replace('"', "&quot;");
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}" role="img" aria-label="{label}">
<title>{label}</title>
<style>{css}</style>
<rect x="0.5" y="0.5" width="{}" height="{}" rx="10" fill="{}" stroke="#30363d"/>
<path d="M10.5 0.5h{}a10 10 0 0 1 10 10v{}h-{}v-{}a10 10 0 0 1 10-10z" fill="#161b22"/>
<line x1="0.5" y1="{BAR}" x2="{}" y2="{BAR}" stroke="#30363d"/>
<circle cx="20" cy="{}" r="6" fill="#ff5f57"/><circle cx="40" cy="{}" r="6" fill="#febc2e"/><circle cx="60" cy="{}" r="6" fill="#28c840"/>
<text x="{}" y="{}" fill="{}" font-family="system-ui, -apple-system, 'Segoe UI', sans-serif" font-size="13" text-anchor="middle">odjazd</text>
<g font-family='{FONT}' font-size="15">
{body}</g>
</svg>
"##,
        px(width - 1.0),
        px(height - 1.0),
        hex(BG),
        px(width - 21.0),
        px(BAR - 10.5),
        px(width - 1.0),
        px(BAR - 10.5),
        px(width - 0.5),
        px(BAR / 2.0),
        px(BAR / 2.0),
        px(BAR / 2.0),
        px(width / 2.0),
        px(BAR / 2.0 + 4.5),
        hex(DIM),
    )
}

/// The SVG elements of one frame, each with its layer: 0 for backgrounds, 1 for text and 2
/// for the cursor.
fn elements(shot: &Shot) -> Vec<(usize, String)> {
    let mut elements = Vec::new();
    let width = usize::from(shot.buffer.area.width);
    for (row, cells) in shot.buffer.content().chunks(width).enumerate() {
        let y = baseline(row);
        let paints: Vec<Paint> = cells.iter().map(paint).collect();

        // A rounded box behind each run of cells on one background colour.
        let mut col = 0;
        while col < cells.len() {
            let start = col;
            while col < cells.len() && paints[col].bg == paints[start].bg {
                col += 1;
            }
            if let Some(bg) = paints[start].bg {
                let (x, width) = (cell_x(start), (col - start) as f64 * CHAR);
                elements.push((
                    0,
                    format!(
                        r#"<rect x="{}" y="{}" width="{}" height="21" rx="3" fill="{}"/>"#,
                        px(x),
                        px(y - 16.0),
                        px(width),
                        hex(bg)
                    ),
                ));
            }
        }

        // The text word by word, each word in its own cells, so the columns line up in any
        // monospace font.
        let mut col = 0;
        while col < cells.len() {
            if cells[col].symbol() == " " {
                col += 1;
                continue;
            }
            let start = col;
            let looks = |paint: &Paint| (paint.fg, paint.bold);
            while col < cells.len()
                && cells[col].symbol() != " "
                && looks(&paints[col]) == looks(&paints[start])
            {
                col += 1;
            }
            let word: String = cells[start..col].iter().map(Cell::symbol).collect();
            let weight = if paints[start].bold {
                r#" font-weight="700""#
            } else {
                ""
            };
            // A line of box drawing joins up across its cells, even in a narrower font.
            let stretch = if word.chars().all(|c| ('\u{2500}'..='\u{257f}').contains(&c)) {
                let length = (col - start) as f64 * CHAR;
                format!(
                    r#" textLength="{}" lengthAdjust="spacingAndGlyphs""#,
                    px(length)
                )
            } else {
                String::new()
            };
            elements.push((
                1,
                format!(
                    r#"<text x="{}" y="{}" fill="{}"{weight}{stretch}>{}</text>"#,
                    px(cell_x(start)),
                    px(y),
                    hex(paints[start].fg),
                    escape(&word)
                ),
            ));
        }
    }

    if let Some(Position { x, y }) = shot.cursor {
        elements.push((
            2,
            format!(
                r#"<rect x="{}" y="{}" width="{CHAR}" height="18" fill="{}" opacity="0.85"/>"#,
                px(cell_x(x.into())),
                px(baseline(y.into()) - 14.0),
                hex(FG)
            ),
        ));
    }
    elements
}

/// How a cell looks: its text colour and weight, and its background unless it has none.
struct Paint {
    fg: [u8; 3],
    bg: Option<[u8; 3]>,
    bold: bool,
}

fn paint(cell: &Cell) -> Paint {
    let mut fg = rgb(cell.fg).unwrap_or(FG);
    let mut bg = rgb(cell.bg);
    if cell.modifier.contains(Modifier::REVERSED) {
        (fg, bg) = (bg.unwrap_or(BG), Some(fg));
    }
    if cell.modifier.contains(Modifier::DIM) {
        // Faint text: the theme's grey, or the colour halfway to its background.
        fg = match bg {
            None if fg == FG => DIM,
            _ => {
                let bg = bg.unwrap_or(BG);
                [0, 1, 2].map(|i| ((u16::from(fg[i]) + u16::from(bg[i])) / 2) as u8)
            }
        };
    }
    Paint {
        fg,
        bg,
        bold: cell.modifier.contains(Modifier::BOLD),
    }
}

/// A terminal colour in RGB, or `None` for the terminal's own.
fn rgb(color: Color) -> Option<[u8; 3]> {
    match color {
        Color::Reset => None,
        Color::Rgb(r, g, b) => Some([r, g, b]),
        // The named colours the app uses, as GitHub's dark theme has them.
        Color::Black => Some(BG),
        Color::Red => Some([0xff, 0x7b, 0x72]),
        other => panic!("no colour for {other:?} in the film yet"),
    }
}

fn baseline(row: usize) -> f64 {
    BAR + PAD + 12.0 + row as f64 * LINE
}

fn cell_x(col: usize) -> f64 {
    PAD + col as f64 * CHAR
}

/// A coordinate, to a tenth of a pixel.
fn px(value: f64) -> String {
    let text = format!("{value:.1}");
    text.strip_suffix(".0").unwrap_or(&text).to_owned()
}

/// A moment of the film as a percentage of its length, for keyframes.
fn percent(millis: i64, total: i64) -> String {
    let text = format!("{:.3}", millis as f64 * 100.0 / total as f64);
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

fn hex([r, g, b]: [u8; 3]) -> String {
    format!("#{r:02x}{g:02x}{b:02x}")
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}
