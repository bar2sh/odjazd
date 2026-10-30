//! Live vehicle positions (GTFS-Realtime), and the delays they give away.
//!
//! Warsaw publishes where every bus and tram is, every 10–20 seconds, but not how late it
//! runs. odjazd works that out: it finds how far along its trip a vehicle is, compares
//! that with when the timetable says it should be there, and carries the difference over
//! to the stops ahead.

use std::collections::{HashMap, hash_map::Entry};

use jiff::{Timestamp, civil::Date};
use prost::Message;

use crate::{Error, LatLon, Timetable, TripIdx, feed::parse_date, timetable::Live};

/// A position farther than this from the straight line between two stops of the trip is
/// not on the trip at all: the vehicle is on a detour, or on its way to the depot.
const MAX_OFF_ROUTE_M: f64 = 250.0;

/// Within this distance of a stop, a vehicle is at the stop.
const AT_STOP_M: f64 = 30.0;

/// A position can fit two places on a trip, as when a line comes back along the same
/// street. Small delays being likelier than big ones, each second of delay counts as this
/// many metres off the route.
const METRES_PER_SECOND_LATE: f64 = 0.25;

/// Faster than any tram or bus in Warsaw (90 km/h): rules out jumps along a trip between
/// two positions.
const TOP_SPEED: f64 = 25.0;

/// Positions measured this long before the feed was made are out of date.
const STALE_AFTER_S: i64 = 5 * 60;

/// No vehicle is this late; a match that says so has found the wrong place on the trip.
pub(crate) const MAX_DELAY_S: i64 = 60 * 60;

/// Where a vehicle is, as reported by a GTFS-Realtime feed.
#[derive(Debug, Clone, PartialEq)]
pub struct VehiclePosition {
    /// The feed's id for the vehicle.
    pub id: String,
    /// What riders see on it, e.g. its fleet number.
    pub label: Option<String>,
    pub trip_id: Option<String>,
    /// The service day of the trip.
    pub service_day: Option<Date>,
    pub position: LatLon,
    /// Degrees clockwise from north.
    pub bearing: Option<f32>,
    /// When the position was measured.
    pub timestamp: Option<Timestamp>,
}

/// The vehicle positions in a GTFS-Realtime feed.
#[derive(Debug, Clone, Default)]
pub struct Positions {
    /// When the feed was made.
    pub published: Option<Timestamp>,
    pub vehicles: Vec<VehiclePosition>,
}

impl Positions {
    /// Reads a GTFS-Realtime `FeedMessage` (Protocol Buffers), keeping the vehicle
    /// positions and skipping everything else.
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let message = proto::FeedMessage::decode(bytes)?;
        let timestamp = |seconds: u64| Timestamp::from_second(seconds.try_into().ok()?).ok();
        let vehicles = message
            .entity
            .into_iter()
            .filter_map(|entity| {
                let vehicle = entity.vehicle?;
                let position = vehicle.position?;
                let descriptor = vehicle.vehicle.unwrap_or_default();
                let trip = vehicle.trip.unwrap_or_default();
                Some(VehiclePosition {
                    id: descriptor.id.or(entity.id)?,
                    label: descriptor.label,
                    trip_id: trip.trip_id,
                    service_day: trip.start_date.as_deref().and_then(parse_date),
                    position: LatLon::new(position.latitude?.into(), position.longitude?.into()),
                    bearing: position.bearing,
                    timestamp: vehicle.timestamp.and_then(timestamp),
                })
            })
            .collect();
        Ok(Self {
            published: message
                .header
                .and_then(|header| header.timestamp)
                .and_then(timestamp),
            vehicles,
        })
    }
}

/// The vehicles on the road, matched to the trips of a [`Timetable`].
#[derive(Debug, Clone, Default)]
pub struct Realtime {
    /// When the positions were published.
    pub published: Option<Timestamp>,
    pub vehicles: Vec<Vehicle>,
    /// Which of `vehicles` does each trip.
    by_trip: HashMap<TripIdx, usize>,
}

#[derive(Debug, Clone)]
pub struct Vehicle {
    pub position: VehiclePosition,
    /// The trip it is doing, when the timetable has it.
    pub trip: Option<TripIdx>,
    /// How far along the trip it is and how late, when the position fits the trip.
    pub progress: Option<Progress>,
}

/// How far along its trip a vehicle is, and how late.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Progress {
    /// Seconds behind the timetable; negative when ahead of it.
    pub delay: i32,
    /// The first stop of the trip (by its position on the trip) the vehicle has not left.
    pub next: usize,
    /// `i + f`: the fraction `f` of the way from the trip's `i`-th stop to the next.
    along: f64,
    at: Timestamp,
}

/// What the live data says about one stop of a trip.
pub(crate) enum Status {
    /// No vehicle on the trip is being tracked.
    Unknown,
    Ahead(Live),
    /// The vehicle has left this stop already.
    Passed,
}

impl Realtime {
    /// The vehicle doing `trip`.
    pub fn vehicle(&self, trip: TripIdx) -> Option<&Vehicle> {
        self.by_trip.get(&trip).map(|&v| &self.vehicles[v])
    }

    /// Vehicles tracked along their trip.
    pub fn tracked(&self) -> usize {
        self.by_trip
            .values()
            .filter(|&&v| self.vehicles[v].progress.is_some())
            .count()
    }

    /// What is known about the `stop`-th stop (by position on the trip) of `trip`.
    pub(crate) fn status(&self, trip: TripIdx, stop: usize) -> Status {
        match self.vehicle(trip).and_then(|vehicle| vehicle.progress) {
            None => Status::Unknown,
            Some(progress) if stop < progress.next => Status::Passed,
            Some(progress) => Status::Ahead(Live {
                delay: progress.delay,
                stops_away: stop - progress.next,
            }),
        }
    }
}

impl Timetable {
    /// Matches vehicle positions to trips and works out how late each vehicle is.
    ///
    /// `previous` is what the last positions gave, for the same timetable. It keeps each
    /// estimate from jumping backwards along the trip, or further ahead than the vehicle
    /// could have driven since.
    pub fn track(&self, positions: Positions, previous: &Realtime) -> Realtime {
        let published = positions.published;
        let mut realtime = Realtime {
            published,
            ..Realtime::default()
        };
        for position in positions.vehicles {
            let measured = position.timestamp.or(published);
            if let (Some(measured), Some(published)) = (measured, published)
                && published.as_second() - measured.as_second() > STALE_AFTER_S
            {
                continue;
            }
            let trip = position
                .trip_id
                .as_deref()
                .and_then(|id| self.trip_by_id(id, position.service_day));
            let progress = trip.zip(measured).and_then(|(trip, measured)| {
                let before = previous.vehicle(trip).and_then(|vehicle| vehicle.progress);
                self.locate(trip, position.position, measured, before)
            });
            let v = realtime.vehicles.len();
            if let Some(trip) = trip {
                // Two vehicles claiming one trip: believe the latest position.
                match realtime.by_trip.entry(trip) {
                    Entry::Vacant(entry) => {
                        entry.insert(v);
                    }
                    Entry::Occupied(mut entry) => {
                        if realtime.vehicles[*entry.get()].position.timestamp < position.timestamp {
                            entry.insert(v);
                        }
                    }
                }
            }
            realtime.vehicles.push(Vehicle {
                position,
                trip,
                progress,
            });
        }
        realtime
    }

    /// Where along `trip` a vehicle at `point` is, and how late, or `None` when the point
    /// is nowhere on the trip.
    fn locate(
        &self,
        trip: TripIdx,
        point: LatLon,
        at: Timestamp,
        before: Option<Progress>,
    ) -> Option<Progress> {
        let (slot, pattern) = (
            self.trips[trip].slot,
            &self.patterns[self.trips[trip].pattern],
        );
        // Stops in metres east and north of the vehicle, and how far along the trip each is.
        let stops: Vec<(f64, f64)> = pattern
            .stops
            .iter()
            .map(|&stop| point.offset(self.stops()[stop].position()))
            .collect();
        let mut metres = vec![0.0; stops.len()];
        for i in 1..stops.len() {
            let (a, b) = (stops[i - 1], stops[i]);
            metres[i] = metres[i - 1] + (b.0 - a.0).hypot(b.1 - a.1);
        }
        let metres_at = |along: f64| {
            let i = (along as usize).min(stops.len() - 1);
            let next = (i + 1).min(stops.len() - 1);
            metres[i] + (along - i as f64) * (metres[next] - metres[i])
        };
        let now = self.seconds(at);
        let reachable = |along: f64| {
            before.is_none_or(|before| {
                let moved = metres_at(along) - metres_at(before.along);
                let elapsed = (at.as_second() - before.at.as_second()).max(0) as f64;
                moved >= -AT_STOP_M && moved <= TOP_SPEED * elapsed + MAX_OFF_ROUTE_M
            })
        };

        let mut best: Option<(f64, Progress)> = None;
        let mut fallback: Option<(f64, Progress)> = None;
        for i in 0..stops.len().saturating_sub(1) {
            let (a, b) = (stops[i], stops[i + 1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let length = dx.hypot(dy);
            // The vehicle is at (0, 0): find the nearest point of the segment.
            let f = if length > 0.0 {
                (-(a.0 * dx + a.1 * dy) / (length * length)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let off_route = (a.0 + f * dx).hypot(a.1 + f * dy);
            if off_route > MAX_OFF_ROUTE_M {
                continue;
            }
            let departure = |i: usize| i64::from(pattern.departure(slot, i));
            let (along, next, delay) = if f * length <= AT_STOP_M {
                // At a stop, or not there yet. Vehicles never leave a stop early, so one
                // that is there ahead of time is waiting, not early.
                (i as f64, i, (now - departure(i)).max(0))
            } else if (1.0 - f) * length <= AT_STOP_M {
                (i as f64 + 1.0, i + 1, (now - departure(i + 1)).max(0))
            } else {
                let from = departure(i) as f64;
                let to = f64::from(pattern.arrival(slot, i + 1));
                let scheduled = from + f * (to - from);
                (i as f64 + f, i + 1, (now as f64 - scheduled).round() as i64)
            };
            if delay.abs() > MAX_DELAY_S {
                continue;
            }
            let score = off_route + METRES_PER_SECOND_LATE * delay.abs() as f64;
            let progress = Progress {
                delay: delay as i32,
                next,
                along,
                at,
            };
            let slot = if reachable(along) {
                &mut best
            } else {
                &mut fallback
            };
            if slot.is_none_or(|(best, _)| score < best) {
                *slot = Some((score, progress));
            }
        }
        // When nothing fits the earlier estimate, that estimate was probably wrong.
        best.or(fallback).map(|(_, progress)| progress)
    }
}

/// The parts of the GTFS-Realtime schema odjazd reads
/// (<https://gtfs.org/documentation/realtime/proto/>); every other field is skipped.
mod proto {
    #[derive(Clone, PartialEq, prost::Message)]
    pub struct FeedMessage {
        #[prost(message, optional, tag = "1")]
        pub header: Option<FeedHeader>,
        #[prost(message, repeated, tag = "2")]
        pub entity: Vec<FeedEntity>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct FeedHeader {
        #[prost(uint64, optional, tag = "3")]
        pub timestamp: Option<u64>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct FeedEntity {
        #[prost(string, optional, tag = "1")]
        pub id: Option<String>,
        #[prost(message, optional, tag = "4")]
        pub vehicle: Option<VehiclePosition>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct VehiclePosition {
        #[prost(message, optional, tag = "1")]
        pub trip: Option<TripDescriptor>,
        #[prost(message, optional, tag = "2")]
        pub position: Option<Position>,
        #[prost(uint64, optional, tag = "5")]
        pub timestamp: Option<u64>,
        #[prost(message, optional, tag = "8")]
        pub vehicle: Option<VehicleDescriptor>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct Position {
        #[prost(float, optional, tag = "1")]
        pub latitude: Option<f32>,
        #[prost(float, optional, tag = "2")]
        pub longitude: Option<f32>,
        #[prost(float, optional, tag = "3")]
        pub bearing: Option<f32>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct TripDescriptor {
        #[prost(string, optional, tag = "1")]
        pub trip_id: Option<String>,
        #[prost(string, optional, tag = "3")]
        pub start_date: Option<String>,
    }

    #[derive(Clone, PartialEq, prost::Message)]
    pub struct VehicleDescriptor {
        #[prost(string, optional, tag = "1")]
        pub id: Option<String>,
        #[prost(string, optional, tag = "2")]
        pub label: Option<String>,
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    #[test]
    fn decodes_vehicle_positions() {
        let vehicle = proto::VehiclePosition {
            trip: Some(proto::TripDescriptor {
                trip_id: Some("2026-10-30:112:PtS:6:2125".into()),
                start_date: Some("20261030".into()),
            }),
            position: Some(proto::Position {
                latitude: Some(52.26087),
                longitude: Some(20.92504),
                bearing: Some(61.7),
            }),
            timestamp: Some(1_793_391_926),
            vehicle: Some(proto::VehicleDescriptor {
                id: Some("V/112/6".into()),
                label: Some("9947".into()),
            }),
        };
        let message = proto::FeedMessage {
            header: Some(proto::FeedHeader {
                timestamp: Some(1_793_391_927),
            }),
            entity: vec![
                proto::FeedEntity {
                    id: Some("V/112/6".into()),
                    vehicle: Some(vehicle),
                },
                // An alert or a trip update: no vehicle, skipped.
                proto::FeedEntity {
                    id: Some("A/1".into()),
                    vehicle: None,
                },
            ],
        };

        let positions = Positions::decode(&message.encode_to_vec()).unwrap();
        assert_eq!(positions.published.unwrap().as_second(), 1_793_391_927);
        let [vehicle] = positions.vehicles.as_slice() else {
            panic!("expected one vehicle, got {:?}", positions.vehicles);
        };
        assert_eq!(vehicle.id, "V/112/6");
        assert_eq!(vehicle.label.as_deref(), Some("9947"));
        assert_eq!(
            vehicle.trip_id.as_deref(),
            Some("2026-10-30:112:PtS:6:2125")
        );
        assert_eq!(vehicle.service_day, Some(date(2026, 10, 30)));
        assert!((vehicle.position.lat - 52.26087).abs() < 1e-5);
        assert_eq!(vehicle.timestamp.unwrap().as_second(), 1_793_391_926);

        assert!(Positions::decode(b"not protobuf").is_err());
    }
}
