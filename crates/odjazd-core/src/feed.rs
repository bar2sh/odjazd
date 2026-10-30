use std::{
    collections::HashMap,
    fs::File,
    path::{Path, PathBuf},
};

use jiff::{civil::Date, tz::TimeZone};
use serde::{Deserialize, de::DeserializeOwned};

use crate::{Error, GtfsTime, LatLon, calendar::Calendar, error::invalid};

/// Position of a stop in [`Feed::stops`].
pub type StopIdx = usize;

/// A single pole or platform where vehicles stop (`location_type` 0 in GTFS).
#[derive(Debug, Clone, PartialEq)]
pub struct Stop {
    pub id: String,
    pub name: String,
    /// Pole number from the sign (`07`), or the platform inside a station (`M1`).
    pub code: Option<String>,
    /// Town (a non-standard column of the Warsaw feed), used to tell apart two different
    /// stops called "Centrum".
    pub town: Option<String>,
    /// Street the pole stands on (another Warsaw-only column), which tells apart poles
    /// on either side of a junction.
    pub street: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

impl Stop {
    pub fn position(&self) -> LatLon {
        LatLon::new(self.lat, self.lon)
    }
}

/// A line, e.g. tram `15` or metro `M1`.
#[derive(Debug, Clone, PartialEq)]
pub struct Route {
    pub id: String,
    /// The name riders know the line by.
    pub name: String,
    pub kind: RouteKind,
    /// Brand colour and matching text colour (RGB), when the feed has them.
    pub color: Option<[u8; 3]>,
    pub text_color: Option<[u8; 3]>,
}

/// What runs on a line, from GTFS `route_type`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum RouteKind {
    Metro,
    Tram,
    Rail,
    Bus,
    Other,
}

impl RouteKind {
    fn from_gtfs(route_type: Option<u16>) -> Self {
        match route_type {
            Some(0 | 5 | 900..=999) => Self::Tram,
            Some(1 | 400..=499) => Self::Metro,
            Some(2 | 100..=199) => Self::Rail,
            Some(3 | 11 | 200..=299 | 700..=899) => Self::Bus,
            _ => Self::Other,
        }
    }
}

/// "Every `headway` seconds between `start` and `end`" for one trip (`frequencies.txt`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Frequency {
    pub start: GtfsTime,
    pub end: GtfsTime,
    pub headway: u32,
    /// Whether vehicles keep to these exact times or only roughly to the headway.
    pub exact: bool,
}

/// An unpacked GTFS feed.
///
/// Opening a feed reads only its small tables. The large ones (`stop_times.txt` is 545 MB
/// for Warsaw) are streamed by each query that needs them.
#[derive(Debug)]
pub struct Feed {
    dir: PathBuf,
    pub(crate) timezone: TimeZone,
    stops: Vec<Stop>,
    pub(crate) routes: Vec<Route>,
    pub(crate) route_index: HashMap<String, usize>,
    pub(crate) calendar: Calendar,
    pub(crate) frequencies: HashMap<String, Vec<Frequency>>,
    version: Option<String>,
}

impl Feed {
    /// Opens the feed unpacked in `dir`.
    pub fn open(dir: impl Into<PathBuf>) -> Result<Self, Error> {
        let dir = dir.into();
        let routes = read_routes(&dir)?;
        Ok(Self {
            timezone: read_timezone(&dir)?,
            stops: read_stops(&dir)?,
            route_index: routes
                .iter()
                .enumerate()
                .map(|(i, route)| (route.id.clone(), i))
                .collect(),
            routes,
            calendar: read_calendar(&dir)?,
            frequencies: read_frequencies(&dir)?,
            version: read_version(&dir)?,
            dir,
        })
    }

    /// The time zone all times in the feed are in.
    pub fn timezone(&self) -> &TimeZone {
        &self.timezone
    }

    pub fn stops(&self) -> &[Stop] {
        &self.stops
    }

    pub fn routes(&self) -> &[Route] {
        &self.routes
    }

    /// The publisher's version of the feed (`feed_info.txt`), often the date it was made.
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    /// Opens one of the feed's large tables for streaming. Unlike the small tables, its
    /// fields are not trimmed: trimming every field makes `stop_times.txt` three times
    /// slower to read, so callers trim just the fields they use.
    pub(crate) fn stream(&self, file: &str) -> Result<csv::Reader<File>, Error> {
        let path = self.dir.join(file);
        csv::ReaderBuilder::new()
            .trim(csv::Trim::Headers)
            .from_path(&path)
            .map_err(|source| Error::Read { path, source })
    }

    /// Turns a CSV error while streaming `file` into an [`Error`].
    pub(crate) fn read_error(&self, file: &str) -> impl Fn(csv::Error) -> Error {
        let path = self.dir.join(file);
        move |source| Error::Read {
            path: path.clone(),
            source,
        }
    }
}

fn reader(path: &Path) -> Result<csv::Reader<File>, Error> {
    csv::ReaderBuilder::new()
        .trim(csv::Trim::All)
        .from_path(path)
        .map_err(|source| Error::Read {
            path: path.to_owned(),
            source,
        })
}

/// Reads every row of a small table. A missing optional table has no rows.
fn rows<T: DeserializeOwned>(dir: &Path, file: &str, required: bool) -> Result<Vec<T>, Error> {
    let path = dir.join(file);
    if !required && !path.exists() {
        return Ok(Vec::new());
    }
    reader(&path)?
        .deserialize()
        .collect::<Result<_, _>>()
        .map_err(|source| Error::Read { path, source })
}

#[derive(Deserialize)]
struct AgencyRow {
    agency_timezone: String,
}

fn read_timezone(dir: &Path) -> Result<TimeZone, Error> {
    let agencies: Vec<AgencyRow> = rows(dir, "agency.txt", true)?;
    // GTFS requires all agencies of a feed to share one time zone.
    let name = agencies
        .into_iter()
        .next()
        .map(|agency| agency.agency_timezone)
        .unwrap_or_default();
    TimeZone::get(&name).map_err(|source| Error::TimeZone { name, source })
}

#[derive(Deserialize)]
struct StopRow {
    stop_id: String,
    #[serde(default)]
    stop_name: String,
    stop_code: Option<String>,
    platform_code: Option<String>,
    stop_lat: Option<f64>,
    stop_lon: Option<f64>,
    location_type: Option<u8>,
    parent_station: Option<String>,
    town_name: Option<String>,
    street_name: Option<String>,
}

fn read_stops(dir: &Path) -> Result<Vec<Stop>, Error> {
    let rows: Vec<StopRow> = rows(dir, "stops.txt", true)?;
    Ok(rows
        .into_iter()
        // Stations, entrances and other nodes have no departures of their own.
        .filter(|row| row.location_type.unwrap_or(0) == 0)
        .map(|row| Stop {
            id: row.stop_id,
            name: row.stop_name,
            // Inside a station `stop_code` is an internal code ("A11"), not a sign.
            code: row
                .platform_code
                .or(row.stop_code.filter(|_| row.parent_station.is_none())),
            town: row.town_name,
            street: row.street_name,
            lat: row.stop_lat.unwrap_or_default(),
            lon: row.stop_lon.unwrap_or_default(),
        })
        .collect())
}

#[derive(Deserialize)]
struct RouteRow {
    route_id: String,
    route_short_name: Option<String>,
    route_long_name: Option<String>,
    route_type: Option<u16>,
    route_color: Option<String>,
    route_text_color: Option<String>,
}

fn read_routes(dir: &Path) -> Result<Vec<Route>, Error> {
    let rows: Vec<RouteRow> = rows(dir, "routes.txt", true)?;
    Ok(rows
        .into_iter()
        .map(|row| Route {
            name: row
                .route_short_name
                .or(row.route_long_name)
                .unwrap_or_else(|| row.route_id.clone()),
            kind: RouteKind::from_gtfs(row.route_type),
            color: parse_color(row.route_color.as_deref()),
            text_color: parse_color(row.route_text_color.as_deref()),
            id: row.route_id,
        })
        .collect())
}

#[derive(Deserialize)]
struct FeedInfoRow {
    feed_version: Option<String>,
}

fn read_version(dir: &Path) -> Result<Option<String>, Error> {
    let rows: Vec<FeedInfoRow> = rows(dir, "feed_info.txt", false)?;
    Ok(rows.into_iter().next().and_then(|row| row.feed_version))
}

#[derive(Deserialize)]
struct CalendarRow {
    service_id: String,
    monday: u8,
    tuesday: u8,
    wednesday: u8,
    thursday: u8,
    friday: u8,
    saturday: u8,
    sunday: u8,
    start_date: String,
    end_date: String,
}

#[derive(Deserialize)]
struct CalendarDateRow {
    service_id: String,
    date: String,
    exception_type: u8,
}

fn read_calendar(dir: &Path) -> Result<Calendar, Error> {
    let mut calendar = Calendar::default();
    for row in rows::<CalendarRow>(dir, "calendar.txt", false)? {
        let date = |s: &str| parse_date(s).ok_or_else(|| invalid("calendar.txt", "date", s));
        let days = [
            row.monday,
            row.tuesday,
            row.wednesday,
            row.thursday,
            row.friday,
            row.saturday,
            row.sunday,
        ];
        let (start, end) = (date(&row.start_date)?, date(&row.end_date)?);
        calendar.add_weekly(row.service_id, days.map(|day| day == 1), start, end);
    }
    for row in rows::<CalendarDateRow>(dir, "calendar_dates.txt", false)? {
        let file = "calendar_dates.txt";
        let date = parse_date(&row.date).ok_or_else(|| invalid(file, "date", &row.date))?;
        let runs = match row.exception_type {
            1 => true,
            2 => false,
            other => return Err(invalid(file, "exception_type", other.to_string())),
        };
        calendar.add_exception(row.service_id, date, runs);
    }
    Ok(calendar)
}

#[derive(Deserialize)]
struct FrequencyRow {
    trip_id: String,
    start_time: String,
    end_time: String,
    headway_secs: u32,
    exact_times: Option<u8>,
}

fn read_frequencies(dir: &Path) -> Result<HashMap<String, Vec<Frequency>>, Error> {
    let file = "frequencies.txt";
    let time = |s: &str| GtfsTime::parse(s.as_bytes()).ok_or_else(|| invalid(file, "time", s));
    let mut frequencies: HashMap<String, Vec<Frequency>> = HashMap::new();
    for row in rows::<FrequencyRow>(dir, file, false)? {
        if row.headway_secs == 0 {
            return Err(invalid(file, "headway_secs", "0"));
        }
        let frequency = Frequency {
            start: time(&row.start_time)?,
            end: time(&row.end_time)?,
            headway: row.headway_secs,
            exact: row.exact_times == Some(1),
        };
        frequencies.entry(row.trip_id).or_default().push(frequency);
    }
    Ok(frequencies)
}

/// Parses a GTFS date (`YYYYMMDD`).
pub(crate) fn parse_date(s: &str) -> Option<Date> {
    if s.len() != 8 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Date::new(
        s[..4].parse().ok()?,
        s[4..6].parse().ok()?,
        s[6..].parse().ok()?,
    )
    .ok()
}

/// Parses a GTFS colour (`RRGGBB`). Anything else counts as no colour.
fn parse_color(hex: Option<&str>) -> Option<[u8; 3]> {
    let hex = hex.filter(|hex| hex.len() == 6)?;
    let channel = |i: usize| u8::from_str_radix(hex.get(i..i + 2)?, 16).ok();
    Some([channel(0)?, channel(2)?, channel(4)?])
}
