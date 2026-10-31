//! What the commands print. Colours go out as ANSI codes; `anstream` drops them when the
//! output is piped or `NO_COLOR` is set.

use std::io::{self, Write};

use anstyle::{Color, RgbColor, Style};
use jiff::{SignedDuration, Timestamp, Zoned, tz::TimeZone};
use odjazd_core::{
    Feed, Live, Route, Stop, StopGroup, Timetable,
    plan::{Journey, Leg, Plan},
    realtime::Realtime,
    timetable::Departure,
};

const BOLD: Style = Style::new().bold();
const DIM: Style = Style::new().dimmed();

/// Late, in the live board's softer red.
const LATE: Style = Style::new().fg_color(Some(Color::Rgb(RgbColor(255, 123, 114))));

/// On time or early.
const ON_TIME: Style = Style::new().fg_color(Some(Color::Rgb(RgbColor(63, 185, 80))));

/// Longer headsigns are cut short with "…".
const HEADSIGN_WIDTH: usize = 28;

/// Setting off this many minutes later can be worth getting there later, or changing more.
const LATER: i64 = 5;

/// The next departures from `group`, how late the vehicles tracked in `realtime` run, and
/// the `others` stops that matched the name too.
pub fn board(
    out: &mut impl Write,
    timetable: &Timetable,
    group: &StopGroup,
    now: &Zoned,
    departures: &[Departure],
    others: &[&StopGroup],
    realtime: Option<&Realtime>,
) -> io::Result<()> {
    let zone = timetable.timezone();
    let when = now.strftime("%a %-d %b, %H:%M");
    writeln!(out, "{BOLD}{}{BOLD:#}  {DIM}{when}{DIM:#}", group.name)?;
    writeln!(out)?;
    if departures.is_empty() {
        writeln!(
            out,
            "No departures found. If that's unexpected, run `odjazd update`."
        )?;
    }

    let line_width = width(
        departures
            .iter()
            .map(|departure| departure.route.name.as_str()),
    );
    let headsign_width = width(departures.iter().map(|departure| departure.headsign));
    let tracked = departures.iter().any(|departure| departure.live.is_some());
    for departure in departures {
        let badge = line_style(departure.route);
        let line = &departure.route.name;
        let headsign = truncate(departure.headsign);
        let code = timetable.stops()[departure.stop].code.as_deref();
        let code = code.unwrap_or_default();
        let approximate = if departure.approximate { "~" } else { "" };
        let time = format!("{approximate}{}", hhmm(departure.scheduled, zone));
        write!(
            out,
            "{badge} {line:^line_width$} {badge:#}  {headsign:<headsign_width$}  \
            {DIM}{code:>3}{DIM:#}  {time:>6}"
        )?;
        if tracked {
            let (late, style) = delay(departure.live);
            write!(out, " {style}{late:<3}{style:#}")?;
        }
        let countdown = countdown(now.timestamp(), departure.expected);
        write!(out, " {BOLD}{countdown:>6}{BOLD:#}")?;
        if tracked {
            write!(out, "  {DIM}{}{DIM:#}", whereabouts(departure.live))?;
        }
        writeln!(out)?;
    }

    if let Some(realtime) = realtime {
        writeln!(out)?;
        let published = realtime.published.map(|published| {
            let published = published.to_zoned(zone.clone());
            published.strftime("%H:%M:%S").to_string()
        });
        match (realtime.tracked(), published) {
            (0, _) => writeln!(
                out,
                "{DIM}No vehicle can be followed right now, so the times are the timetable's.{DIM:#}"
            )?,
            (vehicles, published) => {
                let (vehicles, were) = match vehicles {
                    1 => ("1 bus or tram".to_owned(), "was"),
                    vehicles => (format!("{vehicles} buses and trams"), "were"),
                };
                let at = published
                    .map(|time| format!(" at {time}"))
                    .unwrap_or_default();
                writeln!(
                    out,
                    "{DIM}Live delays from where {vehicles} {were}{at}.{DIM:#}"
                )?;
            }
        }
    }

    if !others.is_empty() {
        let names: Vec<&str> = others.iter().map(|group| group.name.as_str()).collect();
        writeln!(out)?;
        writeln!(
            out,
            "{DIM}Not this one? Also matching: {}{DIM:#}",
            names.join(", ")
        )?;
    }
    Ok(())
}

/// Up to `limit` ways from `from` to `to` setting off at `start`, soonest there first:
/// each a timeline of walks and rides.
pub fn journeys(
    out: &mut impl Write,
    timetable: &Timetable,
    from: &StopGroup,
    to: &StopGroup,
    start: &Zoned,
    plan: &Plan,
    limit: usize,
) -> io::Result<()> {
    let zone = timetable.timezone();
    let stops = timetable.stops();
    let when = start.strftime("%a %-d %b, %H:%M");
    writeln!(
        out,
        "{BOLD}{} → {}{BOLD:#}  {DIM}{when}{DIM:#}",
        from.name, to.name
    )?;

    // Getting off at the stop asked for is getting there, whichever of its poles it is.
    let there = |journey: &Journey| match journey.rides().last() {
        Some(ride) if to.stops.contains(&ride.to) => ride.arrival,
        _ => journey.arrive,
    };
    // The planner leaves out journeys that another one beats on every count, to the second.
    // Shown to the minute, a journey is not worth it either if another gets there no later
    // with no more changes, unless it lets you set off a good while later.
    let beats = |one: &Journey, other: &Journey| {
        let (one_there, other_there) = (minute(there(one)), minute(there(other)));
        let (one_leave, other_leave) = (minute(one.leave), minute(other.leave));
        let (one_changes, other_changes) = (one.transfers(), other.transfers());
        let no_worse = one_there <= other_there
            && one_changes <= other_changes
            && one_leave + LATER > other_leave;
        no_worse
            && (one_there < other_there || one_changes < other_changes || one_leave > other_leave)
    };
    let all = &plan.options;
    let mut options: Vec<&Journey> = all
        .iter()
        .filter(|journey| !all.iter().any(|other| beats(other, journey)))
        .collect();
    options.sort_by_key(|journey| there(journey));
    options.truncate(limit);
    if options.is_empty() {
        writeln!(out)?;
        writeln!(out, "No journey found by public transport.")?;
    }

    let rides = || options.iter().flat_map(|journey| journey.rides());
    let line_width = width(rides().map(|ride| timetable.route_of(ride.trip).name.as_str()));
    let headsign_width = width(rides().map(|ride| timetable.headsign_of(ride.trip)));
    let tracked = rides().any(|ride| ride.live.is_some());
    for journey in &options {
        writeln!(out)?;
        let arrive = there(journey);
        let changes = match journey.transfers() {
            0 => "no changes".to_owned(),
            1 => "1 change".to_owned(),
            changes => format!("{changes} changes"),
        };
        writeln!(
            out,
            "{BOLD}{} → {}{BOLD:#}  {DIM}{}, {changes}{DIM:#}",
            hhmm(journey.leave, zone),
            hhmm(arrive, zone),
            duration(minute(arrive) - minute(journey.leave)),
        )?;

        let mut clock = journey.leave;
        for leg in &journey.legs {
            match leg {
                Leg::Walk(walk) => {
                    let minutes = walk.seconds.div_ceil(60);
                    let place = match walk.to {
                        Some(stop) => Some(pole(&stops[stop])),
                        // Off at the stop asked for, or right by it.
                        None if clock >= arrive || minutes == 0 => None,
                        None => Some(to.name.clone()),
                    };
                    if let Some(place) = place {
                        let time = hhmm(clock, zone);
                        let walk = match minutes {
                            ..=1 => "walk".to_owned(),
                            minutes => format!("walk {minutes} min"),
                        };
                        writeln!(out, "  {DIM}{time}  {walk} to {place}{DIM:#}")?;
                    }
                    clock += SignedDuration::from_secs(walk.seconds.into());
                }
                Leg::Ride(ride) => {
                    let route = timetable.route_of(ride.trip);
                    let badge = line_style(route);
                    let line = &route.name;
                    let headsign = truncate(timetable.headsign_of(ride.trip));
                    let departure = hhmm(ride.departure, zone);
                    write!(
                        out,
                        "  {departure}  {badge} {line:^line_width$} {badge:#}  \
                        {headsign:<headsign_width$}"
                    )?;
                    if tracked {
                        let (late, style) = delay(ride.live);
                        write!(out, "  {style}{late:<3}{style:#}")?;
                    }
                    let ridden = match ride.stops {
                        1 => "1 stop".to_owned(),
                        count => format!("{count} stops"),
                    };
                    let off = pole(&stops[ride.to]);
                    let arrival = hhmm(ride.arrival, zone);
                    writeln!(out, "  {DIM}{ridden} to {off}, {arrival}{DIM:#}")?;
                    clock = ride.arrival;
                }
            }
        }
    }

    if let Some(walk) = &plan.walk {
        writeln!(out)?;
        let there = start.timestamp() + SignedDuration::from_secs(walk.seconds.into());
        writeln!(
            out,
            "{DIM}Or walk all the way: about {}, there at {}.{DIM:#}",
            duration(walk.seconds.div_ceil(60).into()),
            hhmm(there, zone),
        )?;
    }
    Ok(())
}

pub fn stops(out: &mut impl Write, feed: &Feed, groups: &[&StopGroup]) -> io::Result<()> {
    let width = width(groups.iter().map(|group| group.name.as_str()));
    for group in groups {
        let codes: Vec<&str> = group
            .stops
            .iter()
            .filter_map(|&stop| feed.stops()[stop].code.as_deref())
            .collect();
        writeln!(
            out,
            "{BOLD}{:<width$}{BOLD:#}  {DIM}{}{DIM:#}",
            group.name,
            codes.join(" ")
        )?;
    }
    Ok(())
}

/// How late a tracked vehicle runs, in whole minutes: `+2`, `−1`, or `●` when on time.
fn delay(live: Option<Live>) -> (String, Style) {
    let Some(live) = live else {
        return (String::new(), Style::new());
    };
    match late_by(live) {
        0 => ("●".to_owned(), ON_TIME),
        minutes @ 1.. => (format!("+{minutes}"), LATE),
        minutes => (format!("−{}", -minutes), ON_TIME),
    }
}

/// Minutes late, to the nearest minute; negative when early.
pub fn late_by(live: Live) -> i64 {
    (f64::from(live.delay) / 60.0).round() as i64
}

/// Where a tracked vehicle is: how many stops it still has to make before this one.
pub fn whereabouts(live: Option<Live>) -> String {
    match live.map(|live| live.stops_away) {
        None => String::new(),
        Some(0) => "approaching".to_owned(),
        Some(1) => "1 stop away".to_owned(),
        Some(stops) => format!("{stops} stops away"),
    }
}

/// A pole by its stop's name and the number on its sign: "Pl. Narutowicza 08".
fn pole(stop: &Stop) -> String {
    match &stop.code {
        Some(code) => format!("{} {code}", stop.name),
        None => stop.name.clone(),
    }
}

/// The line's own colours (red trams, purple buses…), or inverted text without them.
fn line_style(route: &Route) -> Style {
    let rgb = |[r, g, b]: [u8; 3]| Some(Color::Rgb(RgbColor(r, g, b)));
    match route.color {
        Some(color) => Style::new()
            .bold()
            .bg_color(rgb(color))
            .fg_color(rgb(route.text_color.unwrap_or([255, 255, 255]))),
        None => Style::new().bold().invert(),
    }
}

/// "now", "5 min", or nothing for departures an hour or more away.
fn countdown(now: Timestamp, time: Timestamp) -> String {
    match time.duration_since(now).as_mins() {
        ..1 => "now".to_owned(),
        minutes @ 1..60 => format!("{minutes} min"),
        _ => String::new(),
    }
}

/// "26 min" or "1 h 5 min".
fn duration(minutes: i64) -> String {
    match (minutes / 60, minutes % 60) {
        (0, minutes) => format!("{minutes} min"),
        (hours, 0) => format!("{hours} h"),
        (hours, minutes) => format!("{hours} h {minutes} min"),
    }
}

/// Minutes since 1970, to tell times apart as the clock shows them.
fn minute(time: Timestamp) -> i64 {
    time.as_second().div_euclid(60)
}

fn hhmm(time: Timestamp, zone: &TimeZone) -> String {
    time.to_zoned(zone.clone()).strftime("%H:%M").to_string()
}

/// How wide a column of these texts is, headsigns cut short included.
fn width<'a>(texts: impl Iterator<Item = &'a str>) -> usize {
    texts
        .map(|text| text.chars().count().min(HEADSIGN_WIDTH))
        .max()
        .unwrap_or(0)
}

fn truncate(text: &str) -> String {
    if text.chars().count() <= HEADSIGN_WIDTH {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(HEADSIGN_WIDTH - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{MONDAY, at, feed, late_tram};

    /// The lines `print` writes, without colours or spaces at their ends.
    fn lines(print: impl FnOnce(&mut Vec<u8>) -> io::Result<()>) -> Vec<String> {
        let mut out = Vec::new();
        print(&mut out).unwrap();
        let text = anstream::adapter::strip_str(str::from_utf8(&out).unwrap()).to_string();
        text.lines()
            .map(|line| line.trim_end().to_owned())
            .collect()
    }

    #[test]
    fn the_board_shows_how_late_the_tracked_vehicles_are() {
        let feed = feed("commute");
        let groups = feed.stop_groups();
        let timetable = Timetable::build(&feed, MONDAY).unwrap();
        let now = at(&feed, MONDAY, 7, 58, 0);
        let realtime = late_tram(&timetable, now.timestamp());
        let dom = groups.iter().find(|group| group.name == "Dom").unwrap();
        let departures = timetable.departures(&dom.stops, now.timestamp(), 3, &realtime);

        let lines = lines(|out| {
            board(
                out,
                &timetable,
                dom,
                &now,
                &departures,
                &[],
                Some(&realtime),
            )
        });
        assert_eq!(
            lines,
            [
                "Dom  Mon 2 Nov, 07:58",
                "",
                "  1    Praca   01   08:00 +2   4 min  1 stop away",
                " 100   Praca   02   08:05      7 min",
                "  1    Praca   01   08:10     12 min",
                "",
                "Live delays from where 1 bus or tram was at 07:58:00.",
            ]
        );
    }

    #[test]
    fn a_journey_is_a_timeline_of_walks_and_rides() {
        let feed = feed("commute");
        let groups = feed.stop_groups();
        let timetable = Timetable::build(&feed, MONDAY).unwrap();
        let group = |name| groups.iter().find(|group| group.name == name).unwrap();
        let (dom, praca) = (group("Dom"), group("Praca"));
        let start = at(&feed, MONDAY, 7, 55, 0);
        let none = Realtime::default();
        let plan = timetable.plan(
            crate::centre(&feed, dom),
            crate::centre(&feed, praca),
            start.timestamp(),
            &none,
        );

        let lines = lines(|out| journeys(out, &timetable, dom, praca, &start, &plan, 3));
        // From the depot, the next tram would get there later, though setting off sooner.
        assert_eq!(
            lines,
            [
                "Dom → Praca  Mon 2 Nov, 07:55",
                "",
                "07:59 → 08:12  13 min, no changes",
                "  07:59  walk to Dom 01",
                "  08:00    1    Praca  3 stops to Praca 01, 08:12",
                "",
                "08:04 → 08:13  9 min, no changes",
                "  08:04  walk to Dom 02",
                "  08:05   100   Praca  1 stop to Praca 02, 08:13",
            ]
        );
    }
}
