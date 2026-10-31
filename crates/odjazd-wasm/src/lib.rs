//! [odjazd-core] compiled to WebAssembly for the map page in `site/`: stop search and
//! departure boards worked out in the browser, from a timetable saved with
//! [`odjazd_core::Timetable::to_bytes`] and compressed with brotli.
//!
//! Results come back as plain JavaScript objects. Times are milliseconds since 1970, as
//! from `Date.now()`, and the page shows them in Warsaw time with `Intl.DateTimeFormat`.
//!
//! [odjazd-core]: https://github.com/bar2sh/odjazd/tree/main/crates/odjazd-core

use std::io::Read;

use jiff::{SignedDuration, Timestamp, civil::Time};
use odjazd_core::{LatLon, Route, RouteKind, StopGroup, StopIdx, realtime::Realtime, search};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// Panics, which would be bugs, show up in the browser console with their message.
#[wasm_bindgen(start)]
fn start() {
    console_error_panic_hook::set_once();
}

/// A timetable loaded in the browser.
#[wasm_bindgen]
pub struct Timetable {
    timetable: odjazd_core::Timetable,
    groups: Vec<StopGroup>,
    /// The group of each stop.
    group_of: Vec<usize>,
    /// Always empty: the page has the timetable, not live vehicle positions.
    live: Realtime,
}

/// What the page needs to draw a line's badge.
#[derive(Debug, PartialEq, Serialize)]
pub struct Line<'t> {
    pub name: &'t str,
    pub kind: &'static str,
    /// Background and text colour as `#rrggbb`, when the feed has them.
    pub color: Option<String>,
    pub text: Option<String>,
}

/// One pole on the map.
#[derive(Debug, PartialEq, Serialize)]
pub struct Pole<'t> {
    pub name: &'t str,
    /// The number on the stop sign.
    pub code: Option<&'t str>,
    pub lat: f64,
    pub lon: f64,
    pub group: usize,
    /// The first of metro, tram, rail and bus leaving from here; `None` when nothing does.
    pub kind: Option<&'static str>,
}

/// A stop as riders know it, e.g. all the poles of "Pl. Narutowicza".
#[derive(Debug, PartialEq, Serialize)]
pub struct Group<'t> {
    pub id: usize,
    pub name: &'t str,
    /// The middle of its poles.
    pub lat: f64,
    pub lon: f64,
    /// The lines leaving it: metro first, then trams, trains and buses, by number.
    pub lines: Vec<Line<'t>>,
    /// Metres from the point given to [`Timetable::nearby`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub distance: Option<f64>,
}

#[derive(Debug, PartialEq, Serialize)]
pub struct Departure<'t> {
    /// Milliseconds since 1970.
    pub time: f64,
    pub line: Line<'t>,
    pub headsign: &'t str,
    /// The number of the pole it leaves from.
    pub pole: Option<&'t str>,
    /// The timetable only gives a headway (the metro), so the time is an estimate.
    pub approximate: bool,
}

#[wasm_bindgen]
impl Timetable {
    /// Reads the bytes of `odjazd_core::Timetable::to_bytes`.
    #[wasm_bindgen(constructor)]
    pub fn new(bytes: &[u8]) -> Result<Timetable, JsError> {
        Ok(Self::from(odjazd_core::Timetable::from_bytes(bytes)?))
    }

    /// Reads the same bytes compressed with brotli, as the page downloads them.
    #[wasm_bindgen(js_name = fromBrotli)]
    pub fn from_brotli(compressed: &[u8]) -> Result<Timetable, JsError> {
        Ok(Self::from(decompress(compressed)?))
    }

    /// The publisher's version of the feed, e.g. `2026-10-30 00:15:11`.
    #[wasm_bindgen(getter)]
    pub fn version(&self) -> Option<String> {
        self.timetable.version().map(str::to_owned)
    }

    /// The three service days loaded, as `YYYY-MM-DD`: the day the timetable was saved
    /// for, with the day before and after it.
    pub fn days(&self) -> Vec<String> {
        self.timetable.days().map(|day| day.to_string()).to_vec()
    }

    #[wasm_bindgen(getter, js_name = groupCount)]
    pub fn group_count(&self) -> usize {
        self.groups.len()
    }

    #[wasm_bindgen(getter, js_name = lineCount)]
    pub fn line_count(&self) -> usize {
        self.timetable.routes().len()
    }

    /// Every pole, for drawing the map.
    #[wasm_bindgen(js_name = poles)]
    pub fn js_poles(&self) -> Result<JsValue, JsError> {
        to_js(&self.poles())
    }

    /// Stops matching `query`, best first, as `odjazd stops` finds them.
    #[wasm_bindgen(js_name = search)]
    pub fn js_search(&self, query: &str, limit: usize) -> Result<JsValue, JsError> {
        to_js(&self.search(query, limit))
    }

    /// The stop with this `id`, or `undefined`.
    #[wasm_bindgen(js_name = group)]
    pub fn js_group(&self, id: usize) -> Result<JsValue, JsError> {
        to_js(&self.group(id))
    }

    /// Stops with something leaving them within `radius` metres of a point, nearest
    /// first.
    #[wasm_bindgen(js_name = nearby)]
    pub fn js_nearby(
        &self,
        lat: f64,
        lon: f64,
        radius: f64,
        limit: usize,
    ) -> Result<JsValue, JsError> {
        to_js(&self.nearby(LatLon::new(lat, lon), radius, limit))
    }

    /// The next `limit` departures from stop `group` at or after `at` (milliseconds since
    /// 1970).
    #[wasm_bindgen(js_name = departures)]
    pub fn js_departures(&self, group: usize, at: f64, limit: usize) -> Result<JsValue, JsError> {
        let at = Timestamp::from_millisecond(at as i64)?;
        to_js(&self.departures(group, at, limit))
    }

    /// The moment a rider means by `time` (`HH:MM`, Warsaw time) at `now`, in
    /// milliseconds since 1970.
    #[wasm_bindgen(js_name = leavingAt)]
    pub fn js_leaving_at(&self, now: f64, time: &str) -> Result<f64, JsError> {
        let now = Timestamp::from_millisecond(now as i64)?;
        Ok(self.leaving_at(now, time.parse()?)?.as_millisecond() as f64)
    }
}

/// The methods behind the JavaScript ones, which return Rust values and so can be tested
/// off the browser.
impl Timetable {
    pub fn poles(&self) -> Vec<Pole<'_>> {
        let timetable = &self.timetable;
        timetable
            .stops()
            .iter()
            .enumerate()
            .map(|(i, stop)| Pole {
                name: &stop.name,
                code: stop.code.as_deref(),
                lat: stop.lat,
                lon: stop.lon,
                group: self.group_of[i],
                kind: timetable.routes_at(i).next().map(|route| kind(route.kind)),
            })
            .collect()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<Group<'_>> {
        search(&self.groups, query)
            .into_iter()
            .take(limit)
            .map(|group| self.view(self.group_of[group.stops[0]], None))
            .collect()
    }

    pub fn group(&self, id: usize) -> Option<Group<'_>> {
        (id < self.groups.len()).then(|| self.view(id, None))
    }

    pub fn nearby(&self, point: LatLon, radius: f64, limit: usize) -> Vec<Group<'_>> {
        let mut found: Vec<Group<'_>> = Vec::new();
        for (stop, distance) in self.timetable.nearby(point, radius) {
            let id = self.group_of[stop];
            if found.len() < limit && found.iter().all(|group| group.id != id) {
                found.push(self.view(id, Some(distance)));
            }
        }
        found
    }

    pub fn departures(&self, group: usize, at: Timestamp, limit: usize) -> Vec<Departure<'_>> {
        let Some(group) = self.groups.get(group) else {
            return Vec::new();
        };
        let stops = self.timetable.stops();
        self.timetable
            .departures(&group.stops, at, limit, &self.live)
            .into_iter()
            .map(|departure| Departure {
                time: departure.scheduled.as_millisecond() as f64,
                line: line(departure.route),
                headsign: departure.headsign,
                pole: stops[departure.stop].code.as_deref(),
                approximate: departure.approximate,
            })
            .collect()
    }

    /// `time` today, or tomorrow when today's is more than three hours ago: asked late in
    /// the evening, "06:00" means tomorrow morning, but "21:00" still means this evening.
    pub fn leaving_at(&self, now: Timestamp, time: Time) -> Result<Timestamp, jiff::Error> {
        let now = now.to_zoned(self.timetable.timezone().clone());
        let today = now.with().time(time).build()?;
        if now.timestamp().duration_since(today.timestamp()) > SignedDuration::from_hours(3) {
            Ok(today.tomorrow()?.timestamp())
        } else {
            Ok(today.timestamp())
        }
    }

    fn view(&self, id: usize, distance: Option<f64>) -> Group<'_> {
        let group = &self.groups[id];
        let stops = self.timetable.stops();
        let poles = group.stops.len() as f64;
        let (lat, lon) = group.stops.iter().fold((0.0, 0.0), |(lat, lon), &stop| {
            (lat + stops[stop].lat / poles, lon + stops[stop].lon / poles)
        });
        Group {
            id,
            name: &group.name,
            lat,
            lon,
            lines: self.lines(&group.stops),
            distance,
        }
    }

    /// The lines leaving any of `stops`, in the order riders expect.
    fn lines(&self, stops: &[StopIdx]) -> Vec<Line<'_>> {
        let mut routes: Vec<&Route> = stops
            .iter()
            .flat_map(|&stop| self.timetable.routes_at(stop))
            .collect();
        routes.sort_by_key(|route| line_order(route));
        routes.dedup_by_key(|route| (route.kind, &route.name));
        routes.into_iter().map(line).collect()
    }
}

impl From<odjazd_core::Timetable> for Timetable {
    fn from(timetable: odjazd_core::Timetable) -> Self {
        let groups = timetable.stop_groups();
        let mut group_of = vec![0; timetable.stops().len()];
        for (id, group) in groups.iter().enumerate() {
            for &stop in &group.stops {
                group_of[stop] = id;
            }
        }
        Self {
            timetable,
            groups,
            group_of,
            live: Realtime::default(),
        }
    }
}

fn decompress(compressed: &[u8]) -> Result<odjazd_core::Timetable, odjazd_core::Error> {
    let mut bytes = Vec::new();
    brotli_decompressor::Decompressor::new(compressed, 1 << 16)
        .read_to_end(&mut bytes)
        .map_err(|_| odjazd_core::Error::BinaryFormat)?;
    odjazd_core::Timetable::from_bytes(&bytes)
}

fn to_js(value: &impl Serialize) -> Result<JsValue, JsError> {
    Ok(serde_wasm_bindgen::to_value(value)?)
}

fn kind(kind: RouteKind) -> &'static str {
    match kind {
        RouteKind::Metro => "metro",
        RouteKind::Tram => "tram",
        RouteKind::Rail => "rail",
        RouteKind::Bus => "bus",
        RouteKind::Other => "other",
    }
}

fn line(route: &Route) -> Line<'_> {
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02x}{g:02x}{b:02x}");
    Line {
        name: &route.name,
        kind: kind(route.kind),
        color: route.color.map(hex),
        text: route.text_color.map(hex),
    }
}

/// By kind, then by number, then by name ("N01"), as on the stop signs.
fn line_order(route: &Route) -> (RouteKind, u32, &str) {
    let digits = route.name.bytes().take_while(u8::is_ascii_digit).count();
    let number = route.name[..digits].parse().unwrap_or(u32::MAX);
    (route.kind, number, &route.name)
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;
    use odjazd_core::Feed;

    use super::*;

    /// The mini feed of odjazd-core's tests, saved and compressed as for the page.
    fn timetable() -> Timetable {
        let dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../odjazd-core/tests/fixtures/mini"
        );
        let built = odjazd_core::Timetable::build(&Feed::open(dir).unwrap(), date(2026, 10, 30));
        let bytes = built.unwrap().to_bytes().unwrap();
        let mut compressed = Vec::new();
        let params = brotli::enc::BrotliEncoderParams::default();
        brotli::BrotliCompress(&mut &bytes[..], &mut compressed, &params).unwrap();
        Timetable::from(decompress(&compressed).unwrap())
    }

    fn at(day: i8, hour: i8, minute: i8) -> Timestamp {
        date(2026, 10, day)
            .at(hour, minute, 0, 0)
            .in_tz("Europe/Warsaw")
            .unwrap()
            .timestamp()
    }

    #[test]
    fn finds_stops_with_their_lines() {
        let timetable = timetable();
        let mut found = timetable.search("plac testowy", 5);
        assert_eq!(found.len(), 1);
        let names: Vec<&str> = found[0].lines.iter().map(|line| line.name).collect();
        assert_eq!(found[0].name, "Pl. Testowy");
        // S1 runs on Saturdays, but not on the only one loaded (31 October).
        assert_eq!(names, ["15", "N1"]);
        assert_eq!(found[0].lines[0].color.as_deref(), Some("#b60000"));
        assert_eq!(found[0].lines[0].kind, "tram");
        assert!((found[0].lat - 52.2201).abs() < 1e-9);
        assert_eq!(timetable.group(found[0].id), Some(found.remove(0)));
        assert_eq!(timetable.group(timetable.group_count()), None);
    }

    #[test]
    fn boards_match_the_command_line() {
        let timetable = timetable();
        let id = timetable.search("pl testowy", 1)[0].id;
        let board: Vec<String> = timetable
            .departures(id, at(30, 9, 30), 10)
            .iter()
            .map(|d| {
                let time = Timestamp::from_millisecond(d.time as i64).unwrap();
                let time = time.in_tz("Europe/Warsaw").unwrap().strftime("%a %H:%M");
                format!(
                    "{time} {} {} {}",
                    d.line.name,
                    d.headsign,
                    d.pole.unwrap_or("-")
                )
            })
            .collect();
        assert_eq!(
            board,
            [
                "Fri 10:00 15 Końcowa 01",
                "Fri 11:00 15 Okrężna 01",
                "Sat 00:05 N1 Końcowa 02"
            ]
        );
        assert!(
            timetable
                .departures(usize::MAX, at(30, 9, 30), 10)
                .is_empty()
        );
    }

    #[test]
    fn metro_times_are_estimates() {
        let timetable = timetable();
        let id = timetable.search("testowa", 1)[0].id;
        // Every 10 minutes from 06:00 until 07:00.
        let board = timetable.departures(id, at(30, 6, 35), 3);
        let headsigns: Vec<&str> = board.iter().map(|d| d.headsign).collect();
        assert_eq!(headsigns, ["Kabacka", "Kabacka"]);
        assert!(
            board
                .iter()
                .all(|d| d.approximate && d.line.kind == "metro")
        );
    }

    #[test]
    fn poles_know_what_leaves_from_them() {
        let timetable = timetable();
        let poles = timetable.poles();
        let pole = |name: &str| poles.iter().find(|pole| pole.name == name).unwrap();
        assert_eq!(pole("Pl. Testowy").kind, Some("tram"));
        assert_eq!(pole("Testowa").kind, Some("metro"));
        // The metro ends at Kabacka, so nothing leaves it.
        assert_eq!(pole("Kabacka").kind, None);
        let groups: std::collections::HashSet<usize> = poles.iter().map(|p| p.group).collect();
        assert_eq!(groups.len(), timetable.group_count());
    }

    #[test]
    fn nearby_stops_come_nearest_first_once_each() {
        let timetable = timetable();
        let found = timetable.nearby(LatLon::new(52.2201, 21.0001), 2_000.0, 10);
        let names: Vec<&str> = found.iter().map(|group| group.name).collect();
        assert_eq!(names, ["Pl. Testowy", "Metro Testowa", "Końcowa"]);
        assert!(found.is_sorted_by(|a, b| a.distance <= b.distance));
        assert_eq!(
            timetable
                .nearby(LatLon::new(52.2201, 21.0001), 2_000.0, 1)
                .len(),
            1
        );
    }

    #[test]
    fn leaving_times_are_the_next_ones_riders_mean() {
        let timetable = timetable();
        let leaving = |now, time: &str| timetable.leaving_at(now, time.parse().unwrap()).unwrap();
        // What left two hours ago is still today's.
        assert_eq!(leaving(at(30, 10, 0), "08:00"), at(30, 8, 0));
        assert_eq!(leaving(at(30, 10, 0), "18:30"), at(30, 18, 30));
        assert_eq!(leaving(at(30, 23, 30), "06:00"), at(31, 6, 0));
        assert_eq!(leaving(at(30, 23, 30), "00:15"), at(31, 0, 15));
    }

    #[test]
    fn rejects_files_that_are_not_a_timetable() {
        assert!(decompress(b"not brotli at all").is_err());
    }
}
