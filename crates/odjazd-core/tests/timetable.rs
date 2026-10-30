//! The in-memory [`Timetable`] must give the same boards as [`Feed::departures`] for the
//! tricky cases of `tests/fixtures/mini` (see `departures.rs`).

use jiff::{Timestamp, civil::date};
use odjazd_core::{Feed, Timetable, realtime::Realtime, search};

fn feed() -> Feed {
    Feed::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mini")).unwrap()
}

/// The board at the stop best matching `query`, loading the timetable around `day`.
fn board(feed: &Feed, query: &str, day: (i16, i8, i8), hour: i8, minute: i8) -> Vec<String> {
    let (year, month, day) = day;
    let timetable = Timetable::build(feed, date(year, month, day)).unwrap();
    let from: Timestamp = date(year, month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(feed.timezone().clone())
        .unwrap()
        .timestamp();
    let groups = feed.stop_groups();
    let stops = &search(&groups, query)[0].stops;
    timetable
        .departures(stops, from, 10, &Realtime::default())
        .iter()
        .map(|d| {
            let approximate = if d.approximate { " ~" } else { "" };
            let time = d.expected.to_zoned(timetable.timezone().clone());
            let time = time.strftime("%a %H:%M");
            format!("{time} {} {}{approximate}", d.route.name, d.headsign)
        })
        .collect()
}

#[test]
fn skips_trips_that_end_or_loop_around_here() {
    assert_eq!(
        board(&feed(), "plac testowy", (2026, 10, 30), 9, 30),
        [
            "Fri 10:00 15 Końcowa",
            "Fri 11:00 15 Okrężna",
            "Sat 00:05 N1 Końcowa"
        ]
    );
}

#[test]
fn night_trips_past_midnight_belong_to_the_previous_day() {
    assert_eq!(
        board(&feed(), "pl testowy", (2026, 10, 31), 0, 0),
        ["Sat 00:05 N1 Końcowa"]
    );
}

#[test]
fn calendar_exceptions_cancel_a_weekly_service() {
    let feed = feed();
    assert!(board(&feed, "pl testowy", (2026, 10, 31), 8, 0).is_empty());
    assert_eq!(
        board(&feed, "pl testowy", (2026, 11, 7), 8, 0),
        ["Sat 09:00 S1 Końcowa"]
    );
}

#[test]
fn frequency_based_trips_repeat_every_headway() {
    let feed = feed();
    assert_eq!(
        board(&feed, "testowa", (2026, 10, 30), 6, 35),
        ["Fri 06:40 M1 Kabacka ~", "Fri 06:50 M1 Kabacka ~"]
    );
    assert!(board(&feed, "kabacka", (2026, 10, 30), 6, 0).is_empty());
}
