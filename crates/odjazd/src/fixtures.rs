//! The hand-written feeds of `odjazd-core`'s tests, to test what the app shows.

use jiff::{
    Timestamp, Zoned,
    civil::{Date, date},
};
use odjazd_core::{
    Feed, LatLon, Timetable,
    realtime::{Positions, Realtime, VehiclePosition},
};

/// The day the `mini` feed is written for, a Friday.
pub const FRIDAY: Date = date(2026, 10, 30);

/// The `commute` feed's Monday.
pub const MONDAY: Date = date(2026, 11, 2);

pub fn feed(name: &str) -> Feed {
    let fixtures = concat!(env!("CARGO_MANIFEST_DIR"), "/../odjazd-core/tests/fixtures");
    Feed::open(format!("{fixtures}/{name}")).unwrap()
}

pub fn at(feed: &Feed, day: Date, hour: i8, minute: i8, second: i8) -> Zoned {
    day.at(hour, minute, second, 0)
        .to_zoned(feed.timezone().clone())
        .unwrap()
}

/// In the `commute` feed, the 07:56 tram from the depot, still there at `at` (07:58): two
/// minutes late.
pub fn late_tram(timetable: &Timetable, at: Timestamp) -> Realtime {
    let positions = Positions {
        published: Some(at),
        vehicles: vec![VehiclePosition {
            id: "V/1/1".into(),
            label: Some("3101".into()),
            trip_id: Some("T1a".into()),
            service_day: Some(MONDAY),
            position: LatLon::new(52.1950, 21.0000),
            bearing: None,
            timestamp: Some(at),
        }],
    };
    timetable.track(positions, &Realtime::default())
}
