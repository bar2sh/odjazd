//! Departure boards for a tiny hand-written feed (`tests/fixtures/mini`) covering the
//! tricky parts of GTFS: trips past midnight, trips ending at the stop, frequency-based
//! metro and calendar exceptions.

use jiff::{Zoned, civil::date};
use odjazd_core::{Feed, search};

fn feed() -> Feed {
    Feed::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mini")).unwrap()
}

fn at(feed: &Feed, day: (i16, i8, i8), hour: i8, minute: i8) -> Zoned {
    let (year, month, day) = day;
    date(year, month, day)
        .at(hour, minute, 0, 0)
        .to_zoned(feed.timezone().clone())
        .unwrap()
}

/// The board of the stop best matching `query`, one `"Day HH:MM line headsign"` per row.
fn board(feed: &Feed, query: &str, from: &Zoned) -> Vec<String> {
    let groups = feed.stop_groups();
    let group = search(&groups, query)[0];
    let departures = feed.departures(&group.stops, from, 10).unwrap();
    departures
        .iter()
        .map(|d| {
            let approximate = if d.approximate { " ~" } else { "" };
            format!(
                "{} {} {}{approximate}",
                d.time.strftime("%a %H:%M"),
                d.route.name,
                d.headsign
            )
        })
        .collect()
}

#[test]
fn skips_trips_that_end_or_loop_around_here() {
    let feed = feed();
    // T2 ends at Pl. Testowy; T3 only loops from pole 01 to pole 02 of it. T4 is a
    // circular line that starts and ends here: leaving is a departure, coming back is not.
    let from = at(&feed, (2026, 10, 30), 9, 30);
    assert_eq!(
        board(&feed, "plac testowy", &from),
        [
            "Fri 10:00 15 Końcowa",
            "Fri 11:00 15 Okrężna",
            "Sat 00:05 N1 Końcowa"
        ]
    );
}

#[test]
fn night_trips_past_midnight_belong_to_the_previous_day() {
    let feed = feed();
    // N1 is in Friday's timetable as 24:05, i.e. five past midnight on Saturday.
    let from = at(&feed, (2026, 10, 31), 0, 0);
    assert_eq!(board(&feed, "pl testowy", &from), ["Sat 00:05 N1 Końcowa"]);
}

#[test]
fn calendar_exceptions_cancel_a_weekly_service() {
    let feed = feed();
    // S1 runs on Saturdays, but not on 31 October.
    assert!(board(&feed, "pl testowy", &at(&feed, (2026, 10, 31), 8, 0)).is_empty());
    assert_eq!(
        board(&feed, "pl testowy", &at(&feed, (2026, 11, 7), 8, 0)),
        ["Sat 09:00 S1 Końcowa"]
    );
}

#[test]
fn frequency_based_trips_repeat_every_headway() {
    let feed = feed();
    // The metro runs every 10 minutes from 06:00 to 07:00.
    let from = at(&feed, (2026, 10, 30), 6, 35);
    assert_eq!(
        board(&feed, "testowa", &from),
        ["Fri 06:40 M1 Kabacka ~", "Fri 06:50 M1 Kabacka ~"]
    );
    // Kabacka is the terminus.
    assert!(board(&feed, "kabacka", &at(&feed, (2026, 10, 30), 6, 0)).is_empty());
}

#[test]
fn metro_station_joins_the_metro_stops_above_it() {
    let feed = feed();
    let groups = feed.stop_groups();
    let group = search(&groups, "metro testowa")[0];
    let ids: Vec<&str> = group
        .stops
        .iter()
        .map(|&i| feed.stops()[i].id.as_str())
        .collect();
    assert_eq!(
        (group.name.as_str(), ids),
        ("Metro Testowa", vec!["MB", "MP"])
    );
}

#[test]
fn same_name_far_apart_are_different_stops() {
    let feed = feed();
    let groups = feed.stop_groups();
    let names: Vec<&str> = search(&groups, "centrum")
        .iter()
        .map(|g| g.name.as_str())
        .collect();
    assert_eq!(names, ["Centrum (Warszawa)", "Centrum (Łomianki)"]);
}
