use std::collections::HashMap;

use jiff::{Zoned, civil::Date};
use serde::Deserialize;

use crate::{
    Error, Feed, GtfsTime, Route, Stop, StopIdx,
    error::invalid,
    feed::Frequency,
    time::{number, service_day_origin},
};

/// A vehicle leaving a stop.
#[derive(Debug, Clone)]
pub struct Departure<'f> {
    pub time: Zoned,
    pub route: &'f Route,
    pub headsign: String,
    pub stop: &'f Stop,
    /// The line runs "every few minutes" (in Warsaw: the metro) and the feed only gives the
    /// headway, so this time is an estimate.
    pub approximate: bool,
}

impl Feed {
    /// The next `limit` departures from any of `stops`, at or after `from`.
    ///
    /// Looks at the timetables of yesterday (for night trips past midnight), today and
    /// tomorrow.
    pub fn departures(
        &self,
        stops: &[StopIdx],
        from: &Zoned,
        limit: usize,
    ) -> Result<Vec<Departure<'_>>, Error> {
        let today = from.with_time_zone(self.timezone.clone()).date();
        let days = [today.yesterday()?, today, today.tomorrow()?];
        let origins = days
            .iter()
            .map(|&day| service_day_origin(day, &self.timezone))
            .collect::<Result<Vec<_>, _>>()?;

        let (trip_index, mut trips) = self.running_trips(&days)?;
        let calls = self.calls_at(stops, &trip_index, &mut trips)?;

        let mut times = Vec::new();
        for call in &calls {
            let trip = &trips[call.trip];
            // Only a trip that goes somewhere else afterwards is a departure: not at the end
            // of the line, nor when it just loops round to another pole of the same stop.
            if !trip.last_elsewhere.is_some_and(|last| last > call.sequence) {
                continue;
            }
            for (day, &origin) in origins.iter().enumerate() {
                if trip.days & (1 << day) == 0 {
                    continue;
                }
                match (trip.frequencies, trip.first) {
                    (None, _) => times.push((call.time.after(origin)?, call, false)),
                    // The stop times of a frequency-based trip are a template: offsets
                    // from its first stop, repeated every `headway` seconds.
                    (Some(frequencies), Some(first)) => {
                        let offset = call.time.secs().saturating_sub(first.time.secs());
                        for frequency in frequencies {
                            let (start, end) = (frequency.start.secs(), frequency.end.secs());
                            for start in (start..end).step_by(frequency.headway as usize) {
                                let time = GtfsTime::from_secs(start + offset).after(origin)?;
                                times.push((time, call, !frequency.exact));
                            }
                        }
                    }
                    (Some(_), None) => {}
                }
            }
        }

        let from = from.timestamp();
        times.retain(|&(time, ..)| time >= from);
        times.sort_by_key(|&(time, ..)| time);
        times.truncate(limit);
        Ok(times
            .into_iter()
            .map(|(time, call, approximate)| {
                let trip = &trips[call.trip];
                Departure {
                    time: time.to_zoned(self.timezone.clone()),
                    route: &self.routes[trip.route],
                    headsign: trip.headsign.clone(),
                    stop: &self.stops()[call.stop],
                    approximate,
                }
            })
            .collect())
    }

    /// Trips that run on at least one of `days`, and an index from their ids.
    pub(crate) fn running_trips(
        &self,
        days: &[Date],
    ) -> Result<(HashMap<String, usize>, Vec<Trip<'_>>), Error> {
        #[derive(Deserialize)]
        struct Row<'a> {
            trip_id: &'a str,
            route_id: &'a str,
            service_id: &'a str,
            trip_headsign: Option<&'a str>,
        }

        let services = self.calendar.running_on(days);
        let error = self.read_error("trips.txt");
        let mut reader = self.stream("trips.txt")?;
        let headers = reader.headers().map_err(&error)?.clone();
        let mut record = csv::StringRecord::new();
        let (mut index, mut trips) = (HashMap::new(), Vec::new());
        while reader.read_record(&mut record).map_err(&error)? {
            let row: Row = record.deserialize(Some(&headers)).map_err(&error)?;
            let Some(&days) = services.get(row.service_id.trim()) else {
                continue;
            };
            let route_id = row.route_id.trim();
            let route = *self
                .route_index
                .get(route_id)
                .ok_or_else(|| invalid("trips.txt", "route_id", route_id))?;
            let trip_id = row.trip_id.trim();
            index.insert(trip_id.to_owned(), trips.len());
            trips.push(Trip {
                route,
                headsign: row.trip_headsign.unwrap_or_default().trim().to_owned(),
                days,
                frequencies: self.frequencies.get(trip_id).map(Vec::as_slice),
                first: None,
                last_elsewhere: None,
            });
        }
        Ok((index, trips))
    }

    /// Streams `stop_times.txt` once, collecting the calls at `stops` and, on the way,
    /// where every running trip starts and where it last stops outside `stops`.
    fn calls_at(
        &self,
        stops: &[StopIdx],
        trip_index: &HashMap<String, usize>,
        trips: &mut [Trip<'_>],
    ) -> Result<Vec<Call>, Error> {
        let file = "stop_times.txt";
        let error = self.read_error(file);
        let mut reader = self.stream(file)?;
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
        let departure_column = column("departure_time")?;
        let pickup_column = column("pickup_type").ok();

        let targets: Vec<(&[u8], StopIdx)> = stops
            .iter()
            .map(|&i| (self.stops()[i].id.as_bytes(), i))
            .collect();
        let mut calls = Vec::new();
        let mut record = csv::ByteRecord::new();
        let (mut previous_trip, mut current) = (Vec::new(), None);
        while reader.read_byte_record(&mut record).map_err(&error)? {
            // A trip's rows sit next to each other, so look the trip up only when the id
            // changes. Most rows belong to days we don't care about and end right here.
            let trip_id = record[trip_column].trim_ascii();
            if trip_id != previous_trip.as_slice() {
                previous_trip.clear();
                previous_trip.extend_from_slice(trip_id);
                current = std::str::from_utf8(trip_id)
                    .ok()
                    .and_then(|id| trip_index.get(id).copied());
            }
            let Some(t) = current else {
                continue;
            };

            let sequence = record[sequence_column].trim_ascii();
            let sequence =
                number(sequence).ok_or_else(|| invalid(file, "stop_sequence", sequence))?;
            let time = GtfsTime::parse(record[departure_column].trim_ascii());
            let stop_id = record[stop_column].trim_ascii();
            let target = targets
                .iter()
                .find(|(id, _)| *id == stop_id)
                .map(|&(_, stop)| stop);

            let trip = &mut trips[t];
            if let Some(time) = time
                && trip.first.is_none_or(|first| sequence < first.sequence)
            {
                trip.first = Some(First { sequence, time });
            }
            let Some(stop) = target else {
                trip.last_elsewhere = trip.last_elsewhere.max(Some(sequence));
                continue;
            };
            let boards = pickup_column.is_none_or(|column| record[column].trim_ascii() != b"1");
            if let (true, Some(time)) = (boards, time) {
                calls.push(Call {
                    trip: t,
                    stop,
                    sequence,
                    time,
                });
            }
        }
        Ok(calls)
    }
}

/// A trip running on at least one of the service days a query looks at.
pub(crate) struct Trip<'f> {
    pub(crate) route: usize,
    pub(crate) headsign: String,
    /// Bit `i` is set when the trip runs on service day `i` of the query.
    pub(crate) days: u8,
    pub(crate) frequencies: Option<&'f [Frequency]>,
    /// Filled in while streaming `stop_times.txt`: the first stop, and the last
    /// `stop_sequence` at a stop other than the ones asked about.
    first: Option<First>,
    last_elsewhere: Option<u32>,
}

#[derive(Clone, Copy)]
struct First {
    sequence: u32,
    time: GtfsTime,
}

/// A trip stopping at one of the stops we were asked about.
struct Call {
    trip: usize,
    stop: StopIdx,
    sequence: u32,
    time: GtfsTime,
}
