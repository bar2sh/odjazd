//! How fast odjazd is on Warsaw's real timetable, the one `odjazd update` downloads, at
//! the morning rush hour of the next weekday:
//!
//! ```bash
//! cargo bench -p odjazd-core --features cache
//! ```
//!
//! Without a downloaded timetable there is nothing to measure, so nothing is.

use std::{
    collections::{HashMap, hash_map::Entry},
    env,
    hint::black_box,
    path::{Path, PathBuf},
    time::Duration,
};

use criterion::{BatchSize, Criterion, criterion_group, criterion_main};
use jiff::{SignedDuration, Timestamp, Zoned, civil::Date, civil::Weekday};
use odjazd_core::{
    Feed, LatLon, StopGroup, Timetable,
    realtime::{Positions, Realtime, VehiclePosition},
    search,
};

/// Where `odjazd update` unpacks the timetable: `$ODJAZD_DATA_DIR/feed`, or the system
/// cache directory's.
fn feed_dir() -> Option<PathBuf> {
    let data_dir = match env::var_os("ODJAZD_DATA_DIR") {
        Some(dir) => PathBuf::from(dir),
        None => dirs::cache_dir()?.join("odjazd"),
    };
    Some(data_dir.join("feed")).filter(|dir| dir.is_dir())
}

fn warsaw(c: &mut Criterion) {
    let Some(feed_dir) = feed_dir() else {
        eprintln!("No timetable to measure: run `odjazd update`, or set ODJAZD_DATA_DIR.");
        return;
    };
    let feed = Feed::open(&feed_dir).unwrap();
    let day = weekday(&feed);
    let at = day
        .at(8, 15, 0, 0)
        .to_zoned(feed.timezone().clone())
        .unwrap()
        .timestamp();
    let timetable = Timetable::build(&feed, day).unwrap();
    let groups = timetable.stop_groups();
    let stop = |name| search(&groups, name)[0];
    let (centrum, from, to) = (
        stop("centrum"),
        stop("plac narutowicza"),
        stop("dworzec wilenski"),
    );
    let positions = on_time(&feed_dir, &timetable, day, at);
    let live = timetable.track(positions.clone(), &Realtime::default());
    if timetable
        .departures(&centrum.stops, at, 1, &live)
        .is_empty()
    {
        eprintln!("The timetable has no trips on {day}: run `odjazd update` for a fresh one.");
        return;
    }
    eprintln!(
        "{} at 08:15: the board of {}, from {} to {}, and {} buses and trams, {} of them tracked.",
        day.strftime("%A %-d %B"),
        centrum.name,
        from.name,
        to.name,
        positions.vehicles.len(),
        live.tracked(),
    );
    let (from, to) = (centre(&timetable, from), centre(&timetable, to));

    let mut warsaw = c.benchmark_group("warsaw");
    warsaw.bench_function("open the feed", |b| {
        b.iter(|| Feed::open(&feed_dir).unwrap())
    });
    // About a second each: ten of them, the fewest criterion takes.
    warsaw
        .sample_size(10)
        .measurement_time(Duration::from_secs(15));
    warsaw.bench_function("build the timetable", |b| {
        b.iter(|| Timetable::build(&feed, day).unwrap())
    });
    warsaw
        .sample_size(100)
        .measurement_time(Duration::from_secs(5));
    #[cfg(feature = "cache")]
    {
        let cache_dir = env::temp_dir().join("odjazd-bench");
        Timetable::load_or_build(&feed_dir, &cache_dir, day).unwrap();
        warsaw.bench_function("load the saved timetable", |b| {
            b.iter(|| Timetable::load_or_build(&feed_dir, &cache_dir, day).unwrap())
        });
        let _ = std::fs::remove_dir_all(&cache_dir);
    }
    warsaw.bench_function("search for a stop", |b| {
        b.iter(|| search(&groups, black_box("plac narutowicza")).len())
    });
    warsaw.bench_function("show a board", |b| {
        b.iter(|| timetable.departures(&centrum.stops, black_box(at), 12, &live))
    });
    warsaw.bench_function("plan a journey", |b| {
        b.iter(|| timetable.plan(from, to, black_box(at), &live))
    });
    warsaw.bench_function("track the vehicles", |b| {
        b.iter_batched(
            || positions.clone(),
            |positions| timetable.track(positions, &Realtime::default()),
            BatchSize::LargeInput,
        )
    });
    warsaw.finish();
}

/// Today, or the next weekday when today is a weekend: Saturday has fewer trips.
fn weekday(feed: &Feed) -> Date {
    let mut day = Zoned::now().with_time_zone(feed.timezone().clone()).date();
    while matches!(day.weekday(), Weekday::Saturday | Weekday::Sunday) {
        day = day.tomorrow().unwrap();
    }
    day
}

/// The middle of a stop's poles, as `odjazd route` plans from.
fn centre(timetable: &Timetable, group: &StopGroup) -> LatLon {
    let count = group.stops.len() as f64;
    let (lat, lon) = group.stops.iter().fold((0.0, 0.0), |(lat, lon), &stop| {
        let pole = timetable.stops()[stop].position();
        (lat + pole.lat, lon + pole.lon)
    });
    LatLon::new(lat / count, lon / count)
}

/// Where the buses and trams would be at `at` if they all ran on time: at the next stop
/// they leave, if they leave it within two minutes. That makes about as many as Warsaw
/// reports at that hour.
fn on_time(feed_dir: &Path, timetable: &Timetable, day: Date, at: Timestamp) -> Positions {
    // The live feed names trips by their GTFS ids.
    let mut trips = csv::Reader::from_path(feed_dir.join("trips.txt")).unwrap();
    let column = trips.headers().unwrap().iter().position(|h| h == "trip_id");
    let mut ids = HashMap::new();
    for record in trips.records() {
        let id = record.unwrap()[column.unwrap()].to_owned();
        if let Some(trip) = timetable.trip_by_id(&id, Some(day)) {
            ids.insert(trip, id);
        }
    }

    let soon = at + SignedDuration::from_mins(2);
    let mut next = HashMap::new();
    for stop in 0..timetable.stops().len() {
        for departure in timetable.departures(&[stop], at, 10, &Realtime::default()) {
            if departure.scheduled > soon {
                break;
            }
            // The metro is not in the live feed.
            if departure.approximate {
                continue;
            }
            match next.entry(departure.trip) {
                Entry::Vacant(entry) => {
                    entry.insert((departure.scheduled, stop));
                }
                Entry::Occupied(mut entry) if departure.scheduled < entry.get().0 => {
                    entry.insert((departure.scheduled, stop));
                }
                Entry::Occupied(_) => {}
            }
        }
    }
    let vehicles = next
        .into_iter()
        .filter_map(|(trip, (_, stop))| {
            Some(VehiclePosition {
                id: format!("V/{trip}"),
                label: None,
                trip_id: Some(ids.get(&trip)?.clone()),
                service_day: Some(day),
                position: timetable.stops()[stop].position(),
                bearing: None,
                timestamp: Some(at),
            })
        })
        .collect();
    Positions {
        published: Some(at),
        vehicles,
    }
}

criterion_group!(benches, warsaw);
criterion_main!(benches);
