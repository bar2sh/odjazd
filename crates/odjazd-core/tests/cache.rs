//! A timetable saved with `to_bytes` answers exactly like the one it was saved from, and
//! `load_or_build` reuses a saved one only for the same day and the same download.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

use jiff::civil::{Date, date};
use odjazd_core::{Error, Feed, LatLon, Timetable, realtime::Realtime};

const MINI: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/mini");
const COMMUTE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/commute");

fn build(dir: &str, day: Date) -> Timetable {
    Timetable::build(&Feed::open(dir).unwrap(), day).unwrap()
}

/// Every departure from every stop over the three days, one line each.
fn boards(timetable: &Timetable) -> Vec<String> {
    let from = timetable.days()[0]
        .to_zoned(timetable.timezone().clone())
        .unwrap()
        .timestamp();
    (0..timetable.stops().len())
        .flat_map(|stop| timetable.departures(&[stop], from, 100, &Realtime::default()))
        .map(|d| {
            let id = &timetable.stops()[d.stop].id;
            let (line, approximate) = (&d.route.name, d.approximate);
            format!("{id} {} {line} {} {approximate}", d.scheduled, d.headsign)
        })
        .collect()
}

fn round_trip(timetable: &Timetable) -> Timetable {
    Timetable::from_bytes(&timetable.to_bytes().unwrap()).unwrap()
}

#[test]
fn saved_timetable_gives_the_same_boards() {
    let built = build(MINI, date(2026, 10, 30));
    let loaded = round_trip(&built);
    assert_eq!(loaded.days(), built.days());
    assert_eq!(loaded.version(), built.version());
    assert_eq!(loaded.timezone().iana_name(), Some("Europe/Warsaw"));
    assert_eq!(loaded.stops(), built.stops());
    assert_eq!(loaded.routes(), built.routes());
    assert!(boards(&built).len() > 10);
    assert_eq!(boards(&loaded), boards(&built));
    // Live data finds trips by their GTFS id.
    let day = Some(date(2026, 10, 30));
    assert_eq!(loaded.trip_by_id("T1", day), built.trip_by_id("T1", day));
    assert!(loaded.trip_by_id("T1", day).is_some());
}

#[test]
fn saved_timetable_plans_the_same_journeys() {
    let built = build(COMMUTE, date(2026, 11, 2));
    let loaded = round_trip(&built);
    let at = date(2026, 11, 2)
        .at(7, 55, 0, 0)
        .in_tz("Europe/Warsaw")
        .unwrap()
        .timestamp();
    let (home, work) = (LatLon::new(52.2001, 21.0005), LatLon::new(52.2301, 21.0006));
    let plan = |timetable: &Timetable| {
        let plan = timetable.plan(home, work, at, &Realtime::default());
        format!("{plan:?}")
    };
    assert!(plan(&built).contains("Ride"));
    assert_eq!(plan(&loaded), plan(&built));
}

#[test]
fn rejects_bytes_that_are_not_a_saved_timetable() {
    let bytes = build(MINI, date(2026, 10, 30)).to_bytes().unwrap();
    assert!(matches!(
        Timetable::from_bytes(b"stop_id,stop_name"),
        Err(Error::BinaryFormat)
    ));
    let mut newer = bytes.clone();
    newer[8] += 1;
    assert!(matches!(
        Timetable::from_bytes(&newer),
        Err(Error::BinaryFormat)
    ));
    assert!(matches!(
        Timetable::from_bytes(&bytes[..bytes.len() / 2]),
        Err(Error::Binary(_))
    ));
}

#[test]
fn reads_bytes_wherever_they_are_in_memory() {
    let built = build(MINI, date(2026, 10, 30));
    let mut shifted = vec![0];
    shifted.extend(built.to_bytes().unwrap());
    let loaded = Timetable::from_bytes(&shifted[1..]).unwrap();
    assert_eq!(boards(&loaded), boards(&built));
}

/// A fresh copy of the mini feed, and an empty cache directory beside it.
fn scratch(name: &str) -> (PathBuf, PathBuf) {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = fs::remove_dir_all(&dir);
    let (feed_dir, cache_dir) = (dir.join("feed"), dir.join("cache"));
    fs::create_dir_all(&feed_dir).unwrap();
    for entry in fs::read_dir(MINI).unwrap() {
        let entry = entry.unwrap();
        fs::copy(entry.path(), feed_dir.join(entry.file_name())).unwrap();
    }
    (feed_dir, cache_dir)
}

fn saved(cache_dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(cache_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Overwrites `file` with junk of the same size and modification time, so that only
/// reading it would tell the difference.
fn spoil(file: &Path) {
    let metadata = fs::metadata(file).unwrap();
    fs::write(file, vec![b'#'; metadata.len() as usize]).unwrap();
    let file = fs::File::options().write(true).open(file).unwrap();
    file.set_modified(metadata.modified().unwrap()).unwrap();
}

#[test]
fn cache_is_reused_for_the_same_day_and_download() {
    let (feed_dir, cache_dir) = scratch("cache-reuse");
    let day = date(2026, 10, 30);
    let first = Timetable::load_or_build(&feed_dir, &cache_dir, day).unwrap();
    let names = saved(&cache_dir);
    assert_eq!(names.len(), 1);
    assert!(names[0].starts_with("timetable-20261030-"), "{names:?}");

    spoil(&feed_dir.join("stop_times.txt"));
    let cached = Timetable::load_or_build(&feed_dir, &cache_dir, day).unwrap();
    assert_eq!(boards(&cached), boards(&first));
    assert_eq!(saved(&cache_dir), names);
}

#[test]
fn new_day_or_download_builds_again() {
    let (feed_dir, cache_dir) = scratch("cache-rebuild");
    let day = date(2026, 10, 30);
    Timetable::load_or_build(&feed_dir, &cache_dir, day).unwrap();
    let stop_times = feed_dir.join("stop_times.txt");
    let original = fs::read(&stop_times).unwrap();

    // The feed now fails to build, so an answer could only come from the cache.
    spoil(&stop_times);
    let next_day = date(2026, 10, 31);
    assert!(Timetable::load_or_build(&feed_dir, &cache_dir, next_day).is_err());
    let touched = fs::metadata(&stop_times).unwrap().modified().unwrap() + Duration::from_secs(1);
    let file = fs::File::options().write(true).open(&stop_times).unwrap();
    file.set_modified(touched).unwrap();
    assert!(Timetable::load_or_build(&feed_dir, &cache_dir, day).is_err());

    // A good download replaces the old cache.
    fs::write(&stop_times, original).unwrap();
    let rebuilt = Timetable::load_or_build(&feed_dir, &cache_dir, next_day).unwrap();
    assert_eq!(rebuilt.days()[1], next_day);
    let names = saved(&cache_dir);
    assert_eq!(names.len(), 1);
    assert!(names[0].starts_with("timetable-20261031-"), "{names:?}");
}

#[test]
fn unwritable_cache_only_costs_time() {
    let (feed_dir, cache_dir) = scratch("cache-unwritable");
    // A file where the cache directory should be.
    fs::write(&cache_dir, "").unwrap();
    let timetable = Timetable::load_or_build(&feed_dir, &cache_dir, date(2026, 10, 30)).unwrap();
    assert!(!boards(&timetable).is_empty());
}
