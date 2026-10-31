//! Live delays and "which pole should I walk to?" on `tests/fixtures/commute`: home has
//! two poles, Dom 01 (tram 1 at 08:00 and 08:10, from the Zajezdnia depot) and Dom 02
//! (bus 100 at 08:05), and both lines go to Praca.

use jiff::{Timestamp, civil::date};
use odjazd_core::{
    Feed, LatLon, Live, StopIdx, Timetable,
    realtime::{Positions, Realtime, VehiclePosition},
};

const HOME: LatLon = LatLon::new(52.2001, 21.0005);
const WORK: LatLon = LatLon::new(52.2301, 21.0006);

fn timetable() -> Timetable {
    let feed = Feed::open(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/commute"
    ))
    .unwrap();
    Timetable::build(&feed, date(2026, 11, 2)).unwrap()
}

fn at(hour: i8, minute: i8) -> Timestamp {
    date(2026, 11, 2)
        .at(hour, minute, 0, 0)
        .in_tz("Europe/Warsaw")
        .unwrap()
        .timestamp()
}

fn stop(timetable: &Timetable, id: &str) -> StopIdx {
    timetable
        .stops()
        .iter()
        .position(|stop| stop.id == id)
        .unwrap()
}

/// Tram trip `T1a` reported at `position` at `time`.
fn tram_at(timetable: &Timetable, position: LatLon, time: Timestamp) -> Realtime {
    let positions = Positions {
        published: Some(time),
        vehicles: vec![VehiclePosition {
            id: "V/1/1".into(),
            label: Some("3101".into()),
            trip_id: Some("T1a".into()),
            service_day: Some(date(2026, 11, 2)),
            position,
            bearing: None,
            timestamp: Some(time),
        }],
    };
    timetable.track(positions, &Realtime::default())
}

/// Names and codes of the poles to start from, best option first.
fn starts(timetable: &Timetable, live: &Realtime, time: Timestamp) -> Vec<String> {
    timetable
        .plan(HOME, WORK, time, live)
        .options
        .iter()
        .map(|journey| {
            let stop = &timetable.stops()[journey.start()];
            format!("{} {}", stop.name, stop.code.as_deref().unwrap_or(""))
        })
        .collect()
}

#[test]
fn walks_to_the_pole_that_gets_there_first() {
    let timetable = timetable();
    // The tram from Dom 01 arrives at 08:12, the bus from Dom 02 at 08:13. The bus is
    // still offered: it leaves at 08:05, so you can set off later.
    assert_eq!(
        starts(&timetable, &Realtime::default(), at(7, 58)),
        ["Dom 01", "Dom 02"]
    );
    let plan = timetable.plan(HOME, WORK, at(7, 58), &Realtime::default());
    let best = &plan.options[0];
    assert_eq!(best.transfers(), 0);
    // Leave 36 s before the tram, the walk to the pole.
    assert_eq!(best.leave, at(8, 0) - jiff::SignedDuration::from_secs(36));
    assert!(plan.walk.is_none(), "3.3 km is too far to walk");
}

#[test]
fn a_late_tram_makes_the_bus_the_better_choice() {
    let timetable = timetable();
    // At 07:58 the 07:56 tram is still at the depot: two minutes late, so it reaches
    // Praca at 08:14, after the bus. Taking it would mean setting off earlier to arrive
    // later, so it is not offered at all.
    let depot = LatLon::new(52.1950, 21.0000);
    let live = tram_at(&timetable, depot, at(7, 58));
    assert_eq!(starts(&timetable, &live, at(7, 58)), ["Dom 02"]);

    let departures = timetable.departures(&[stop(&timetable, "H1")], at(7, 58), 2, &live);
    let times: Vec<_> = departures.iter().map(|d| (d.expected, d.live)).collect();
    assert_eq!(
        times,
        [
            (
                at(8, 2),
                Some(Live {
                    delay: 120,
                    stops_away: 1
                })
            ),
            (at(8, 10), None)
        ]
    );
}

#[test]
fn a_late_tram_can_be_caught_after_its_time() {
    let timetable = timetable();
    // Only the tram goes to Aleja. The one due at Dom 01 at 08:00 is still at the depot at
    // 07:58, two minutes late, so at 08:00 there is time to walk to the pole and catch it
    // instead of waiting for the 08:10.
    let depot = LatLon::new(52.1950, 21.0000);
    let live = tram_at(&timetable, depot, at(7, 58));
    let aleja = LatLon::new(52.2100, 21.0000);
    let plan = timetable.plan(HOME, aleja, at(8, 0), &live);
    let rides: Vec<_> = plan.options[0]
        .rides()
        .map(|ride| (ride.departure, ride.arrival, ride.live))
        .collect();
    let late = Live {
        delay: 120,
        stops_away: 1,
    };
    assert_eq!(rides, [(at(8, 2), at(8, 6), Some(late))]);
}

#[test]
fn a_tram_that_has_left_is_not_on_the_board() {
    let timetable = timetable();
    // Halfway to Aleja at 07:59, three minutes early: it has left Dom 01 already.
    let halfway = LatLon::new(52.2050, 21.0000);
    let live = tram_at(&timetable, halfway, at(7, 59));
    assert_eq!(live.vehicles[0].progress.unwrap().delay, -180);

    let departures = timetable.departures(&[stop(&timetable, "H1")], at(7, 59), 2, &live);
    let times: Vec<_> = departures.iter().map(|d| d.expected).collect();
    assert_eq!(times, [at(8, 10)]);
}
