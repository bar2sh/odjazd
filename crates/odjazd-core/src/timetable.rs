//! A few days of the timetable held in memory, for a server that answers many queries:
//! departure boards, live delays and journey planning.
//!
//! [`Feed::departures`] streams all of `stop_times.txt` for every query, which suits a
//! command that runs once. [`Timetable`] reads it once and lays the trips out the way
//! RAPTOR (see [`crate::plan`]) wants them: grouped into *patterns*, trips that call at
//! the same stops in the same order, so that the time of a trip at a stop is a single
//! array lookup.

use std::collections::HashMap;

use jiff::{SignedDuration, Timestamp, civil::Date, tz::TimeZone};

use crate::{
    Error, Feed, GtfsTime, LatLon, Route, RouteKind, Stop, StopIdx,
    error::invalid,
    geo::walking_time,
    realtime::{MAX_DELAY_S, Realtime, Status},
    time::{number, service_day_origin},
};

/// Position of a trip in a [`Timetable`].
pub type TripIdx = usize;

/// Poles at most this far apart, in a straight line, are a walkable transfer.
const TRANSFER_RADIUS_M: f64 = 400.0;

/// The trips of three service days (yesterday, today and tomorrow), in memory.
#[derive(Debug)]
#[cfg_attr(
    feature = "cache",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
pub struct Timetable {
    #[cfg_attr(feature = "cache", rkyv(with = crate::cache::IanaName))]
    timezone: TimeZone,
    #[cfg_attr(feature = "cache", rkyv(with = rkyv::with::Map<crate::cache::Ymd>))]
    days: [Date; 3],
    /// Times are seconds after this instant, the start of the first service day.
    #[cfg_attr(feature = "cache", rkyv(with = crate::cache::Nanoseconds))]
    epoch: Timestamp,
    version: Option<String>,
    stops: Vec<Stop>,
    routes: Vec<Route>,
    pub(crate) trips: Vec<Trip>,
    pub(crate) patterns: Vec<Pattern>,
    /// Where each stop appears in patterns: (pattern, position in its stops).
    pub(crate) stop_patterns: Vec<Vec<(usize, usize)>>,
    /// Stops within walking distance of each stop, with the walk in seconds.
    pub(crate) footpaths: Vec<Vec<(StopIdx, u32)>>,
    /// The lines leaving each stop, in the order riders expect them.
    stop_routes: Vec<Vec<usize>>,
    /// GTFS `trip_id` → the trips made from it, by service day.
    #[cfg_attr(
        feature = "cache",
        rkyv(with = rkyv::with::MapKV<rkyv::with::Identity, rkyv::with::Map<crate::cache::DayTrip>>)
    )]
    trip_ids: HashMap<String, Vec<(Date, TripIdx)>>,
}

/// One run of a vehicle along a pattern.
#[derive(Debug)]
#[cfg_attr(
    feature = "cache",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
pub(crate) struct Trip {
    pub(crate) route: usize,
    pub(crate) headsign: String,
    pub(crate) pattern: usize,
    /// Position among the pattern's trips.
    pub(crate) slot: usize,
    /// Times estimated from a headway (`frequencies.txt`), as for the metro.
    pub(crate) approximate: bool,
}

/// Trips of a line that call at the same stops in the same order, with the same rules
/// about getting on and off. No trip of a pattern overtakes another.
#[derive(Debug)]
#[cfg_attr(
    feature = "cache",
    derive(rkyv::Archive, rkyv::Serialize, rkyv::Deserialize)
)]
pub(crate) struct Pattern {
    pub(crate) route: usize,
    pub(crate) stops: Vec<StopIdx>,
    pub(crate) pickup: Vec<bool>,
    pub(crate) drop_off: Vec<bool>,
    /// Earliest first.
    pub(crate) trips: Vec<TripIdx>,
    /// Arrival and departure of the trip in slot `s` at stop `i`, at `[s * stops.len() + i]`.
    arrivals: Vec<u32>,
    departures: Vec<u32>,
}

impl Pattern {
    pub(crate) fn arrival(&self, slot: usize, i: usize) -> u32 {
        self.arrivals[slot * self.stops.len() + i]
    }

    pub(crate) fn departure(&self, slot: usize, i: usize) -> u32 {
        self.departures[slot * self.stops.len() + i]
    }

    /// The first slot whose departure from stop `i` is at or after `time`.
    pub(crate) fn first_departing(&self, i: usize, time: u32) -> usize {
        let (mut low, mut high) = (0, self.trips.len());
        while low < high {
            let mid = low + (high - low) / 2;
            if self.departure(mid, i) < time {
                low = mid + 1;
            } else {
                high = mid;
            }
        }
        low
    }
}

/// A vehicle leaving a stop, with what the live data says about it.
#[derive(Debug, Clone)]
pub struct Departure<'t> {
    pub trip: TripIdx,
    pub route: &'t Route,
    pub headsign: &'t str,
    pub stop: StopIdx,
    pub scheduled: Timestamp,
    /// When it will really leave: [`Self::scheduled`] plus the live delay.
    pub expected: Timestamp,
    pub live: Option<Live>,
    /// The timetable only gives a headway (the metro), so this time is an estimate.
    pub approximate: bool,
}

/// What a tracked vehicle tells about one of its stops.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Live {
    /// Seconds late; negative when early.
    pub delay: i32,
    /// Stops the vehicle still has to make before this one.
    pub stops_away: usize,
}

impl Timetable {
    /// Loads the trips of the three service days around `today`: yesterday's, whose night
    /// trips run past midnight, today's and tomorrow's.
    pub fn build(feed: &Feed, today: Date) -> Result<Self, Error> {
        let timezone = feed.timezone().clone();
        let days = [today.yesterday()?, today, today.tomorrow()?];
        let mut origins = [Timestamp::UNIX_EPOCH; 3];
        for (origin, &day) in origins.iter_mut().zip(&days) {
            *origin = service_day_origin(day, &timezone)?;
        }
        let epoch = origins[0];

        let (trip_index, gtfs_trips) = feed.running_trips(&days)?;
        let mut gtfs_ids = vec![""; gtfs_trips.len()];
        for (id, &i) in &trip_index {
            gtfs_ids[i] = id;
        }
        let stop_times = read_stop_times(feed, &trip_index)?;

        let mut builder = Builder::default();
        let mut trip_ids: HashMap<String, Vec<(Date, TripIdx)>> = HashMap::new();
        for calls in stop_times.chunk_by(|a, b| a.trip == b.trip) {
            let gtfs = &gtfs_trips[calls[0].trip];
            let Some(times) = trip_times(calls) else {
                continue;
            };
            let key = PatternKey {
                route: gtfs.route,
                calls: calls
                    .iter()
                    .map(|call| (call.stop, call.pickup, call.drop_off))
                    .collect(),
            };
            let pattern = builder.pattern(key);
            for (day, &origin) in origins.iter().enumerate() {
                if gtfs.days & (1 << day) == 0 {
                    continue;
                }
                let offset = origin.as_second() - epoch.as_second();
                let mut add = |shift: i64, approximate: bool| {
                    builder.add(
                        pattern,
                        Trip {
                            route: gtfs.route,
                            headsign: gtfs.headsign.clone(),
                            pattern,
                            slot: 0,
                            approximate,
                        },
                        &times,
                        shift,
                    )
                };
                match gtfs.frequencies {
                    None => {
                        let trip = add(offset, false);
                        let id = gtfs_ids[calls[0].trip].to_owned();
                        trip_ids.entry(id).or_default().push((days[day], trip));
                    }
                    // The stop times are a template: offsets from the first stop,
                    // repeated every `headway` seconds.
                    Some(frequencies) => {
                        let first = i64::from(times[0].1);
                        for frequency in frequencies {
                            let (start, end) = (frequency.start.secs(), frequency.end.secs());
                            for start in (start..end).step_by(frequency.headway as usize) {
                                add(offset + i64::from(start) - first, !frequency.exact);
                            }
                        }
                    }
                }
            }
        }
        let (trips, patterns) = builder.finish();

        let stops = feed.stops().to_vec();
        let routes = feed.routes().to_vec();
        let mut stop_patterns = vec![Vec::new(); stops.len()];
        let mut stop_routes = vec![Vec::new(); stops.len()];
        for (p, pattern) in patterns.iter().enumerate() {
            for (i, &stop) in pattern.stops.iter().enumerate() {
                stop_patterns[stop].push((p, i));
                if pattern.pickup[i] && pattern.stops[i + 1..].iter().any(|&s| s != stop) {
                    stop_routes[stop].push(pattern.route);
                }
            }
        }
        for lines in &mut stop_routes {
            lines.sort_by_key(|&r| line_order(&routes[r]));
            lines.dedup();
        }

        Ok(Self {
            timezone,
            days,
            epoch,
            version: feed.version().map(str::to_owned),
            footpaths: footpaths(&stops),
            stops,
            routes,
            trips,
            patterns,
            stop_patterns,
            stop_routes,
            trip_ids,
        })
    }

    pub fn timezone(&self) -> &TimeZone {
        &self.timezone
    }

    /// The service days loaded.
    pub fn days(&self) -> [Date; 3] {
        self.days
    }

    /// See [`Feed::version`].
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    pub fn stops(&self) -> &[Stop] {
        &self.stops
    }

    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// The lines leaving `stop`: metro first, then trams, trains and buses, by number.
    pub fn routes_at(&self, stop: StopIdx) -> impl Iterator<Item = &Route> {
        self.stop_routes[stop].iter().map(|&r| &self.routes[r])
    }

    pub fn route_of(&self, trip: TripIdx) -> &Route {
        &self.routes[self.trips[trip].route]
    }

    pub fn headsign_of(&self, trip: TripIdx) -> &str {
        &self.trips[trip].headsign
    }

    /// Whether the times of `trip` are estimated from a headway.
    pub fn is_approximate(&self, trip: TripIdx) -> bool {
        self.trips[trip].approximate
    }

    /// The trip made from GTFS `trip_id` on `service_day`, or on any day when that is
    /// not known.
    pub fn trip_by_id(&self, trip_id: &str, service_day: Option<Date>) -> Option<TripIdx> {
        let trips = self.trip_ids.get(trip_id)?;
        match service_day {
            Some(day) => trips.iter().find(|&&(d, _)| d == day).map(|&(_, t)| t),
            None => trips.first().map(|&(_, t)| t),
        }
    }

    /// Stops with something leaving them within `radius` metres of `point`, nearest
    /// first, with their distance.
    pub fn nearby(&self, point: LatLon, radius: f64) -> Vec<(StopIdx, f64)> {
        let mut found = self.within(point, radius);
        found.retain(|&(stop, _)| !self.stop_routes[stop].is_empty());
        found
    }

    /// All stops within `radius` metres of `point`, nearest first, with their distance.
    pub(crate) fn within(&self, point: LatLon, radius: f64) -> Vec<(StopIdx, f64)> {
        let mut found: Vec<_> = self
            .stops
            .iter()
            .enumerate()
            .map(|(i, stop)| (i, point.distance(stop.position())))
            .filter(|&(_, distance)| distance <= radius)
            .collect();
        found.sort_by(|a, b| a.1.total_cmp(&b.1));
        found
    }

    /// The next `limit` departures from any of `stops`, at or after `from`, by expected
    /// time. A vehicle that has already passed the stop is left out even if the timetable
    /// says it is still to come; a late one is kept even if its time has gone.
    pub fn departures(
        &self,
        stops: &[StopIdx],
        from: Timestamp,
        limit: usize,
        live: &Realtime,
    ) -> Vec<Departure<'_>> {
        let from_secs = self.seconds(from);
        let mut found = Vec::new();
        for &stop in stops {
            for &(p, i) in &self.stop_patterns[stop] {
                let pattern = &self.patterns[p];
                // Only a trip that goes somewhere else afterwards is a departure: not at
                // the end of the line, nor when it just loops round to another pole.
                if !pattern.pickup[i] || pattern.stops[i + 1..].iter().all(|s| stops.contains(s)) {
                    continue;
                }
                let earliest = (from_secs - MAX_DELAY_S).clamp(0, i64::from(u32::MAX)) as u32;
                let mut on_time = 0;
                for slot in pattern.first_departing(i, earliest)..pattern.trips.len() {
                    let trip = pattern.trips[slot];
                    let scheduled = self.timestamp(pattern.departure(slot, i));
                    let (expected, live) = match live.status(trip, i) {
                        Status::Passed => continue,
                        Status::Unknown => (scheduled, None),
                        Status::Ahead(progress) => {
                            let expected =
                                scheduled + SignedDuration::from_secs(progress.delay.into());
                            (expected.max(from), Some(progress))
                        }
                    };
                    if expected < from {
                        continue;
                    }
                    found.push(Departure {
                        trip,
                        route: &self.routes[pattern.route],
                        headsign: &self.trips[trip].headsign,
                        stop,
                        scheduled,
                        expected,
                        live,
                        approximate: self.trips[trip].approximate,
                    });
                    // Later trips can only leave later than these.
                    if scheduled >= from {
                        on_time += 1;
                        if on_time == limit {
                            break;
                        }
                    }
                }
            }
        }
        found.sort_by_key(|departure| departure.expected);
        found.truncate(limit);
        found
    }

    /// The instant `seconds` after the start of the first service day.
    pub(crate) fn timestamp(&self, seconds: u32) -> Timestamp {
        self.epoch + SignedDuration::from_secs(seconds.into())
    }

    /// Seconds from the start of the first service day to `time`.
    pub(crate) fn seconds(&self, time: Timestamp) -> i64 {
        time.as_second() - self.epoch.as_second()
    }
}

/// Riders find lines in this order: by kind, then by number, then by name ("N01").
fn line_order(route: &Route) -> (RouteKind, u32, &str) {
    let digits = route.name.bytes().take_while(u8::is_ascii_digit).count();
    let number = route.name[..digits].parse().unwrap_or(u32::MAX);
    (route.kind, number, &route.name)
}

/// One row of `stop_times.txt` belonging to a trip we load.
struct StopTime {
    trip: usize,
    sequence: u32,
    stop: StopIdx,
    arrival: Option<GtfsTime>,
    departure: Option<GtfsTime>,
    pickup: bool,
    drop_off: bool,
}

/// The stop times of the trips in `trip_index`, grouped by trip and in stop order.
fn read_stop_times(
    feed: &Feed,
    trip_index: &HashMap<String, usize>,
) -> Result<Vec<StopTime>, Error> {
    let file = "stop_times.txt";
    let error = feed.read_error(file);
    let mut reader = feed.stream(file)?;
    let headers = reader.byte_headers().map_err(&error)?.clone();
    let column = |name: &'static str| {
        headers
            .iter()
            .position(|header| header == name.as_bytes())
            .ok_or(Error::MissingColumn { file, column: name })
    };
    let trip_column = column("trip_id")?;
    let stop_column = column("stop_id")?;
    let sequence_column = column("stop_sequence")?;
    let arrival_column = column("arrival_time")?;
    let departure_column = column("departure_time")?;
    let pickup_column = column("pickup_type").ok();
    let drop_off_column = column("drop_off_type").ok();

    let stop_index: HashMap<&[u8], StopIdx> = feed
        .stops()
        .iter()
        .enumerate()
        .map(|(i, stop)| (stop.id.as_bytes(), i))
        .collect();
    // "1" means riders can't get on (or off) there.
    let allowed = |record: &csv::ByteRecord, column: Option<usize>| {
        column.is_none_or(|column| record[column].trim_ascii() != b"1")
    };

    let mut rows = Vec::new();
    let mut record = csv::ByteRecord::new();
    let (mut previous_trip, mut current) = (Vec::new(), None);
    while reader.read_byte_record(&mut record).map_err(&error)? {
        // As in `Feed::departures`: look the trip up only when the id changes.
        let trip_id = record[trip_column].trim_ascii();
        if trip_id != previous_trip.as_slice() {
            previous_trip.clear();
            previous_trip.extend_from_slice(trip_id);
            current = std::str::from_utf8(trip_id)
                .ok()
                .and_then(|id| trip_index.get(id).copied());
        }
        let Some(trip) = current else {
            continue;
        };
        let Some(&stop) = stop_index.get(record[stop_column].trim_ascii()) else {
            continue;
        };
        let sequence = record[sequence_column].trim_ascii();
        rows.push(StopTime {
            trip,
            sequence: number(sequence).ok_or_else(|| invalid(file, "stop_sequence", sequence))?,
            stop,
            arrival: GtfsTime::parse(record[arrival_column].trim_ascii()),
            departure: GtfsTime::parse(record[departure_column].trim_ascii()),
            pickup: allowed(&record, pickup_column),
            drop_off: allowed(&record, drop_off_column),
        });
    }
    if !rows.is_sorted_by_key(|row| (row.trip, row.sequence)) {
        rows.sort_by_key(|row| (row.trip, row.sequence));
    }
    Ok(rows)
}

/// A trip's arrival and departure at each of its stops, in seconds of its service day.
/// GTFS may leave out the times of stops between timing points; those are spread
/// evenly. `None` when the trip has no time at its first or last stop.
fn trip_times(calls: &[StopTime]) -> Option<Vec<(u32, u32)>> {
    let known: Vec<Option<(u32, u32)>> = calls
        .iter()
        .map(|call| {
            let arrival = call.arrival.or(call.departure)?.secs();
            let departure = call.departure.or(call.arrival)?.secs();
            Some((arrival, departure.max(arrival)))
        })
        .collect();
    known.first().copied().flatten()?;
    known.last().copied().flatten()?;

    let mut times: Vec<(u32, u32)> = Vec::with_capacity(known.len());
    // The last stop with a time of its own.
    let mut previous = 0;
    for (i, &time) in known.iter().enumerate() {
        let time = match time {
            Some(time) => {
                previous = i;
                time
            }
            None => {
                let (next, (next_arrival, _)) = (i..known.len())
                    .find_map(|j| Some((j, known[j]?)))
                    .expect("the last stop has a time");
                let departure = times[previous].1;
                let elapsed = next_arrival.saturating_sub(departure) * (i - previous) as u32;
                let time = departure + elapsed / (next - previous) as u32;
                (time, time)
            }
        };
        times.push(time);
    }
    // Times must never run backwards along a trip.
    times
        .windows(2)
        .all(|pair| pair[0].1 <= pair[1].0)
        .then_some(times)
}

/// Stops and boarding rules shared by the trips of a pattern.
#[derive(Clone, PartialEq, Eq, Hash)]
struct PatternKey {
    route: usize,
    calls: Vec<(StopIdx, bool, bool)>,
}

/// A trip with its arrival and departure at each stop.
type TimedTrip = (TripIdx, Vec<(u32, u32)>);

/// Groups trips into patterns while the timetable loads.
#[derive(Default)]
struct Builder {
    keys: HashMap<PatternKey, usize>,
    /// Per pattern: its key, and its trips.
    groups: Vec<(PatternKey, Vec<TimedTrip>)>,
    trips: Vec<Trip>,
}

impl Builder {
    fn pattern(&mut self, key: PatternKey) -> usize {
        if let Some(&p) = self.keys.get(&key) {
            return p;
        }
        let p = self.groups.len();
        self.groups.push((key.clone(), Vec::new()));
        self.keys.insert(key, p);
        p
    }

    /// Adds a trip whose times are `times` (seconds of its service day) moved by `shift`.
    fn add(&mut self, pattern: usize, trip: Trip, times: &[(u32, u32)], shift: i64) -> TripIdx {
        let moved = |t: u32| (i64::from(t) + shift).clamp(0, i64::from(u32::MAX)) as u32;
        let times = times.iter().map(|&(a, d)| (moved(a), moved(d))).collect();
        let t = self.trips.len();
        self.trips.push(trip);
        self.groups[pattern].1.push((t, times));
        t
    }

    /// Sorts each pattern's trips by time and splits off trips that overtake others, so
    /// that every pattern can be searched by time at any of its stops.
    fn finish(mut self) -> (Vec<Trip>, Vec<Pattern>) {
        let mut patterns = Vec::new();
        for (key, mut trips) in self.groups {
            trips.sort_by_key(|(_, times)| (times[0].1, times[0].0));
            // Each trip joins the first sub-pattern whose last trip it doesn't overtake.
            let mut fifo: Vec<Vec<TimedTrip>> = Vec::new();
            for trip in trips {
                let keeps_order = |group: &&mut Vec<TimedTrip>| {
                    let last = &group.last().expect("groups are never empty").1;
                    last.iter()
                        .zip(&trip.1)
                        .all(|(a, b)| a.0 <= b.0 && a.1 <= b.1)
                };
                match fifo.iter_mut().find(keeps_order) {
                    Some(group) => group.push(trip),
                    None => fifo.push(vec![trip]),
                }
            }
            for group in fifo {
                let p = patterns.len();
                let mut pattern = Pattern {
                    route: key.route,
                    stops: key.calls.iter().map(|&(stop, ..)| stop).collect(),
                    pickup: key.calls.iter().map(|&(_, pickup, _)| pickup).collect(),
                    drop_off: key.calls.iter().map(|&(.., drop_off)| drop_off).collect(),
                    trips: Vec::with_capacity(group.len()),
                    arrivals: Vec::with_capacity(group.len() * key.calls.len()),
                    departures: Vec::with_capacity(group.len() * key.calls.len()),
                };
                for (slot, (t, times)) in group.into_iter().enumerate() {
                    self.trips[t].pattern = p;
                    self.trips[t].slot = slot;
                    pattern.trips.push(t);
                    pattern.arrivals.extend(times.iter().map(|&(a, _)| a));
                    pattern.departures.extend(times.iter().map(|&(_, d)| d));
                }
                patterns.push(pattern);
            }
        }
        (self.trips, patterns)
    }
}

/// For each stop, the others within [`TRANSFER_RADIUS_M`] and the walk there in seconds.
fn footpaths(stops: &[Stop]) -> Vec<Vec<(StopIdx, u32)>> {
    // One degree of latitude is 111 km anywhere, so after sorting by latitude only a
    // narrow band of stops needs checking.
    let band = TRANSFER_RADIUS_M / 111_000.0;
    let mut by_lat: Vec<StopIdx> = (0..stops.len()).collect();
    by_lat.sort_by(|&a, &b| stops[a].lat.total_cmp(&stops[b].lat));
    let mut paths = vec![Vec::new(); stops.len()];
    for (n, &a) in by_lat.iter().enumerate() {
        for &b in &by_lat[n + 1..] {
            if stops[b].lat - stops[a].lat > band {
                break;
            }
            let distance = stops[a].position().distance(stops[b].position());
            if distance <= TRANSFER_RADIUS_M {
                let walk = walking_time(distance);
                paths[a].push((b, walk));
                paths[b].push((a, walk));
            }
        }
    }
    paths
}
