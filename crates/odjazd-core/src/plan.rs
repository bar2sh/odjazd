//! Journey planning with RAPTOR (Delling, Pajor and Werneck, *Round-Based Public Transit
//! Routing*, Microsoft Research, 2012).
//!
//! RAPTOR works in rounds. Round `k` finds the earliest arrival at every stop with `k`
//! rides: it takes every pattern through a stop reached in the round before, rides along
//! it on the first trip a rider there can still catch, then lets riders walk to nearby
//! poles. Each pattern is scanned at most once per round, so a search across the whole
//! city takes milliseconds.
//!
//! To answer "which pole near me should I walk to?", [`Timetable::plan`] runs a search
//! from each pole near the start and compares where each one gets you.

use jiff::Timestamp;

use crate::{
    LatLon, StopIdx, Timetable, TripIdx,
    geo::walking_time,
    realtime::{MAX_DELAY_S, Realtime, Status},
    timetable::{Live, Pattern},
};

/// Poles this close to the start, or to the destination, are within walking distance.
const WALK_RADIUS_M: f64 = 800.0;

/// At most this many poles near the start are tried, nearest first.
const MAX_STARTS: usize = 40;

/// Four changes at most.
const MAX_RIDES: usize = 5;

/// Time to get off one vehicle and onto another.
const TRANSFER_S: u32 = 60;

/// Arriving this much later is worth one change fewer.
const CHANGE_WORTH_S: u32 = 3 * 60;

/// A live vehicle may run up to this much ahead of its timetable.
const MAX_EARLY_S: u32 = 3 * 60;

/// Walking all the way is suggested when it takes at most this long.
const MAX_WALK_S: u32 = 40 * 60;

/// Options arriving later than this after the best one are not worth showing.
const MAX_LATER_S: i64 = 30 * 60;

const MAX_OPTIONS: usize = 6;

const UNREACHED: u32 = u32::MAX;

/// Ways to get from one point to another.
#[derive(Debug, Clone)]
pub struct Plan {
    /// At most one journey per starting pole, soonest arrival first. Each one offers
    /// something the others don't, such as a later start or fewer changes: a journey that
    /// sets off no later than another, arrives no sooner and has no fewer changes is left
    /// out.
    pub options: Vec<Journey>,
    /// Walking all the way, when that is not too far.
    pub walk: Option<Walk>,
}

/// A walk to a pole, rides with changes, and a walk to the destination.
#[derive(Debug, Clone)]
pub struct Journey {
    pub legs: Vec<Leg>,
    /// When to set off to catch the first ride.
    pub leave: Timestamp,
    pub arrive: Timestamp,
}

#[derive(Debug, Clone)]
pub enum Leg {
    Walk(Walk),
    Ride(Ride),
}

/// A walk between two stops, or between a stop and the start or destination (`None`).
#[derive(Debug, Clone, PartialEq)]
pub struct Walk {
    pub from: Option<StopIdx>,
    pub to: Option<StopIdx>,
    pub seconds: u32,
    /// Metres in a straight line.
    pub distance: f64,
}

#[derive(Debug, Clone)]
pub struct Ride {
    pub trip: TripIdx,
    pub from: StopIdx,
    pub to: StopIdx,
    /// When it leaves and arrives, live delay included.
    pub departure: Timestamp,
    pub arrival: Timestamp,
    /// When the timetable says it leaves.
    pub scheduled: Timestamp,
    pub live: Option<Live>,
    /// Stops ridden.
    pub stops: usize,
}

impl Journey {
    /// The pole to walk to first.
    pub fn start(&self) -> StopIdx {
        self.rides().next().expect("a journey has rides").from
    }

    pub fn rides(&self) -> impl Iterator<Item = &Ride> {
        self.legs.iter().filter_map(|leg| match leg {
            Leg::Ride(ride) => Some(ride),
            Leg::Walk(_) => None,
        })
    }

    pub fn transfers(&self) -> usize {
        self.rides().count().saturating_sub(1)
    }

    /// Walking at the start, in seconds.
    fn first_walk(&self) -> u32 {
        match self.legs.first() {
            Some(Leg::Walk(walk)) => walk.seconds,
            _ => 0,
        }
    }

    /// Whether this journey is the better one whatever you care about: it sets off no
    /// earlier, arrives no later and changes no more often than `other`, and differs from
    /// it in at least one of those.
    fn beats(&self, other: &Journey) -> bool {
        let mine = (self.leave, self.arrive, self.transfers());
        let theirs = (other.leave, other.arrive, other.transfers());
        mine.0 >= theirs.0 && mine.1 <= theirs.1 && mine.2 <= theirs.2 && mine != theirs
    }
}

impl Timetable {
    /// The fastest journey from each pole near `from` to `to`, setting off at `at`, using
    /// live delays from `live`. Of journeys arriving within a few minutes of each other,
    /// the one with fewer changes wins, and journeys that another one beats on every count
    /// are left out.
    pub fn plan(&self, from: LatLon, to: LatLon, at: Timestamp, live: &Realtime) -> Plan {
        let direct = from.distance(to);
        let walk = Some(Walk {
            from: None,
            to: None,
            seconds: walking_time(direct),
            distance: direct,
        })
        .filter(|walk| walk.seconds <= MAX_WALK_S);
        let Ok(start) = u32::try_from(self.seconds(at)) else {
            return Plan {
                options: Vec::new(),
                walk,
            };
        };

        // Any stop riders can get off at will do, the end of a line too.
        let mut egress = vec![UNREACHED; self.stops().len()];
        for (stop, distance) in self.within(to, WALK_RADIUS_M) {
            egress[stop] = walking_time(distance);
        }
        let mut search = Search::new(self, live, &egress);
        let mut options: Vec<Journey> = self
            .nearby(from, WALK_RADIUS_M)
            .into_iter()
            .take(MAX_STARTS)
            .filter_map(|(stop, distance)| {
                let access = walking_time(distance);
                let (round, last, arrival) = search.run(stop, start + access)?;
                let mut legs = vec![Leg::Walk(Walk {
                    from: None,
                    to: Some(stop),
                    seconds: access,
                    distance,
                })];
                legs.extend(search.legs(round, last));
                legs.push(Leg::Walk(Walk {
                    from: Some(last),
                    to: None,
                    seconds: egress[last],
                    distance: self.stops()[last].position().distance(to),
                }));
                let legs = merge_walks(legs);
                let journey = Journey {
                    leave: legs.iter().find_map(|leg| match leg {
                        Leg::Ride(ride) => Some(ride.departure),
                        Leg::Walk(_) => None,
                    })? - jiff::SignedDuration::from_secs(access.into()),
                    arrive: self.timestamp(arrival),
                    legs,
                };
                Some(journey)
            })
            .collect();

        // Catching the same vehicle at two poles: keep the one that lets you leave later.
        options.sort_by_key(|journey| {
            let first = journey.rides().next().map(|ride| ride.trip);
            (
                first,
                std::cmp::Reverse(journey.leave),
                journey.first_walk(),
            )
        });
        options.dedup_by_key(|journey| journey.rides().next().map(|ride| ride.trip));
        // Setting off earlier to arrive later, with as many changes, is no choice at all.
        let beaten: Vec<bool> = options
            .iter()
            .map(|journey| options.iter().any(|other| other.beats(journey)))
            .collect();
        let mut options: Vec<Journey> = options
            .into_iter()
            .zip(beaten)
            .filter_map(|(journey, beaten)| (!beaten).then_some(journey))
            .collect();
        options.sort_by_key(|journey| (journey.arrive, journey.transfers(), journey.first_walk()));
        if let Some(best) = options.first().map(|journey| journey.arrive) {
            options.retain(|journey| journey.arrive.as_second() - best.as_second() <= MAX_LATER_S);
        }
        options.truncate(MAX_OPTIONS);
        Plan { options, walk }
    }
}

/// Joins walks that follow each other, e.g. a change between poles and the walk on to the
/// destination.
fn merge_walks(legs: Vec<Leg>) -> Vec<Leg> {
    let mut merged: Vec<Leg> = Vec::with_capacity(legs.len());
    for leg in legs {
        match (merged.last_mut(), leg) {
            (Some(Leg::Walk(previous)), Leg::Walk(walk)) => {
                previous.to = walk.to;
                previous.seconds += walk.seconds;
                previous.distance += walk.distance;
            }
            (_, leg) => merged.push(leg),
        }
    }
    merged
}

/// How a stop was reached.
#[derive(Debug, Clone, Copy)]
struct Label {
    time: u32,
    parent: Parent,
}

#[derive(Debug, Clone, Copy)]
enum Parent {
    Start,
    Ride {
        pattern: usize,
        slot: usize,
        board: usize,
        alight: usize,
        delay: i32,
    },
    Walk {
        from: StopIdx,
    },
}

const NOT_REACHED: Label = Label {
    time: UNREACHED,
    parent: Parent::Start,
};

/// A RAPTOR search, whose buffers are reused from one starting pole to the next.
struct Search<'t> {
    timetable: &'t Timetable,
    live: &'t Realtime,
    /// Seconds from each stop to the destination on foot; [`UNREACHED`] when too far.
    egress: &'t [u32],
    /// `labels[k][stop]`: the best arrival found in round `k`, if it improved on earlier
    /// rounds.
    labels: Vec<Vec<Label>>,
    /// Earliest arrival so far at each stop, over all rounds.
    best: Vec<u32>,
    /// `best` as it was before this round: where riders can board from.
    reached: Vec<u32>,
    marked: Vec<bool>,
    marked_stops: Vec<StopIdx>,
    /// Per pattern, the first position to scan it from this round.
    queue: Vec<usize>,
    queued: Vec<usize>,
}

impl<'t> Search<'t> {
    fn new(timetable: &'t Timetable, live: &'t Realtime, egress: &'t [u32]) -> Self {
        let stops = timetable.stops().len();
        Self {
            timetable,
            live,
            egress,
            labels: vec![vec![NOT_REACHED; stops]; MAX_RIDES + 1],
            best: vec![UNREACHED; stops],
            reached: vec![UNREACHED; stops],
            marked: vec![false; stops],
            marked_stops: Vec::new(),
            queue: vec![usize::MAX; timetable.patterns.len()],
            queued: Vec::new(),
        }
    }

    /// Searches from `source`, reached at `start`. Returns the round (number of rides),
    /// the last stop and the arrival at the destination of the best journey.
    fn run(&mut self, source: StopIdx, start: u32) -> Option<(usize, StopIdx, u32)> {
        for round in &mut self.labels {
            round.fill(NOT_REACHED);
        }
        self.best.fill(UNREACHED);
        self.labels[0][source] = Label {
            time: start,
            parent: Parent::Start,
        };
        self.best[source] = start;
        self.marked_stops.clear();
        self.marked_stops.push(source);

        let mut target = UNREACHED;
        // Each round that got to the destination sooner: (round, last stop, arrival).
        let mut found = Vec::new();
        for round in 1..=MAX_RIDES {
            self.reached.copy_from_slice(&self.best);
            for &stop in &self.marked_stops {
                self.marked[stop] = false;
                for &(p, i) in &self.timetable.stop_patterns[stop] {
                    if self.queue[p] == usize::MAX {
                        self.queued.push(p);
                    }
                    self.queue[p] = self.queue[p].min(i);
                }
            }
            self.marked_stops.clear();

            for n in 0..self.queued.len() {
                let p = self.queued[n];
                self.scan(round, p, self.queue[p], target);
                self.queue[p] = usize::MAX;
            }
            self.queued.clear();

            // Walks to nearby poles, only from stops reached by riding: one walk at a time.
            let ridden = self.marked_stops.len();
            for n in 0..ridden {
                let stop = self.marked_stops[n];
                let time = self.labels[round][stop].time;
                for &(other, walk) in &self.timetable.footpaths[stop] {
                    let time = time + walk;
                    if time < self.best[other].min(target) {
                        self.best[other] = time;
                        self.labels[round][other] = Label {
                            time,
                            parent: Parent::Walk { from: stop },
                        };
                        self.mark(other);
                    }
                }
            }

            let mut improved = None;
            for &stop in &self.marked_stops {
                if self.egress[stop] != UNREACHED {
                    let arrival = self.labels[round][stop].time + self.egress[stop];
                    if arrival < target {
                        target = arrival;
                        improved = Some((round, stop, arrival));
                    }
                }
            }
            found.extend(improved);
            if self.marked_stops.is_empty() {
                break;
            }
        }
        for &stop in &self.marked_stops {
            self.marked[stop] = false;
        }

        // Later rounds only ever arrive sooner, so the last is the earliest.
        let &(.., earliest) = found.last()?;
        found
            .into_iter()
            .find(|&(.., arrival)| arrival <= earliest + CHANGE_WORTH_S)
    }

    fn mark(&mut self, stop: StopIdx) {
        if !self.marked[stop] {
            self.marked[stop] = true;
            self.marked_stops.push(stop);
        }
    }

    /// Rides pattern `p` from position `from` on, catching the first trip riders can get
    /// at each stop, and updates the stops it gets to sooner than before.
    fn scan(&mut self, round: usize, p: usize, from: usize, target: u32) {
        let pattern = &self.timetable.patterns[p];
        // Slot, boarding position and delay of the trip being ridden.
        let mut boarded: Option<(usize, usize, i32)> = None;
        for i in from..pattern.stops.len() {
            let stop = pattern.stops[i];
            if let Some((slot, board, delay)) = boarded
                && pattern.drop_off[i]
            {
                let arrival = shift(pattern.arrival(slot, i), delay);
                if arrival < self.best[stop].min(target) {
                    self.best[stop] = arrival;
                    self.labels[round][stop] = Label {
                        time: arrival,
                        parent: Parent::Ride {
                            pattern: p,
                            slot,
                            board,
                            alight: i,
                            delay,
                        },
                    };
                    self.mark(stop);
                }
            }

            let reached = self.reached[stop];
            if reached == UNREACHED || !pattern.pickup[i] {
                continue;
            }
            let ready = reached + if round > 1 { TRANSFER_S } else { 0 };
            let current = boarded.map(|(slot, _, delay)| shift(pattern.departure(slot, i), delay));
            if current.is_some_and(|departure| departure <= ready) {
                continue;
            }
            if let Some((slot, delay, departure)) = self.catch(pattern, i, ready)
                && current.is_none_or(|current| departure < current)
            {
                boarded = Some((slot, i, delay));
            }
        }
    }

    /// The trip leaving the `i`-th stop of `pattern` soonest at or after `ready`, live
    /// delays included: slot, delay and departure.
    fn catch(&self, pattern: &Pattern, i: usize, ready: u32) -> Option<(usize, i32, u32)> {
        let earliest = ready.saturating_sub(MAX_DELAY_S as u32);
        let mut best: Option<(usize, i32, u32)> = None;
        for slot in pattern.first_departing(i, earliest)..pattern.trips.len() {
            let scheduled = pattern.departure(slot, i);
            // Later trips leave later still, give or take a vehicle running early.
            if best.is_some_and(|(.., departure)| scheduled > departure + MAX_EARLY_S) {
                break;
            }
            let delay = match self.live.status(pattern.trips[slot], i) {
                Status::Passed => continue,
                Status::Unknown => 0,
                Status::Ahead(live) => live.delay,
            };
            let departure = shift(scheduled, delay);
            if departure >= ready && best.is_none_or(|(.., best)| departure < best) {
                best = Some((slot, delay, departure));
            }
        }
        best
    }

    /// The legs of the journey reaching `stop` in `round`, from the starting pole on.
    fn legs(&self, mut round: usize, mut stop: StopIdx) -> Vec<Leg> {
        let timetable = self.timetable;
        let mut legs = Vec::new();
        loop {
            let label = self.labels[round][stop];
            match label.parent {
                Parent::Start => break,
                Parent::Walk { from } => {
                    let distance = timetable.stops()[from]
                        .position()
                        .distance(timetable.stops()[stop].position());
                    legs.push(Leg::Walk(Walk {
                        from: Some(from),
                        to: Some(stop),
                        seconds: label.time - self.labels[round][from].time,
                        distance,
                    }));
                    stop = from;
                }
                Parent::Ride {
                    pattern,
                    slot,
                    board,
                    alight,
                    delay,
                } => {
                    let pattern = &timetable.patterns[pattern];
                    let trip = pattern.trips[slot];
                    let scheduled = pattern.departure(slot, board);
                    legs.push(Leg::Ride(Ride {
                        trip,
                        from: pattern.stops[board],
                        to: stop,
                        departure: timetable.timestamp(shift(scheduled, delay)),
                        arrival: timetable.timestamp(label.time),
                        scheduled: timetable.timestamp(scheduled),
                        live: match self.live.status(trip, board) {
                            Status::Ahead(live) => Some(live),
                            Status::Unknown | Status::Passed => None,
                        },
                        stops: alight - board,
                    }));
                    stop = pattern.stops[board];
                    // The rider got there in an earlier round: the last one that improved it.
                    round = (0..round)
                        .rev()
                        .find(|&r| self.labels[r][stop].time != UNREACHED)
                        .expect("riders board where they have been");
                }
            }
        }
        legs.reverse();
        legs
    }
}

/// `time` moved by `delay` seconds.
fn shift(time: u32, delay: i32) -> u32 {
    (i64::from(time) + i64::from(delay)).clamp(0, i64::from(u32::MAX - 1)) as u32
}
