//! The JSON API, as described in `API.md`.

use axum::{
    Json, Router,
    extract::{Query, State, rejection::QueryRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{any, get},
};
use jiff::Timestamp;
use odjazd_core::{
    LatLon, Route, RouteKind, StopIdx, Timetable,
    plan::{Journey, Leg},
    search,
    timetable::Departure,
    walking_time,
};
use serde::{Deserialize, Serialize};
use tokio::sync::Semaphore;

use crate::state::{AppState, Current};

/// Plans being worked out at once. Each takes a tenth of a second of CPU, so a burst of
/// requests waits its turn instead of starving everything else.
static PLANNING: Semaphore = Semaphore::const_new(4);

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/api/nearby", get(nearby))
        .route("/api/departures", get(departures))
        .route("/api/plan", get(plan))
        .route("/api/stops", get(stops))
        .route("/api/vehicles", get(vehicles))
        .route("/api/status", get(status))
        .route(
            "/api/{*rest}",
            any(|| async { ApiError::NotFound("no such API") }),
        )
        .layer(axum::middleware::map_response(no_store))
        .with_state(state)
}

/// Departures go stale in seconds: browsers and proxies must not keep them.
async fn no_store(mut response: Response) -> Response {
    let no_store = HeaderValue::from_static("no-store");
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, no_store);
    response
}

enum ApiError {
    /// The timetable is still loading.
    Loading,
    BadRequest(String),
    NotFound(&'static str),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::Loading => (
                StatusCode::SERVICE_UNAVAILABLE,
                "the timetable is loading".to_owned(),
            ),
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::NotFound(message) => (StatusCode::NOT_FOUND, message.to_owned()),
        };
        (status, Json(serde_json::json!({ "error": message }))).into_response()
    }
}

impl From<QueryRejection> for ApiError {
    fn from(rejection: QueryRejection) -> Self {
        Self::BadRequest(rejection.body_text())
    }
}

fn current(state: &AppState) -> Result<Current, ApiError> {
    state.current().ok_or(ApiError::Loading)
}

fn point(lat: f64, lon: f64) -> Result<LatLon, ApiError> {
    if (-90.0..=90.0).contains(&lat) && (-180.0..=180.0).contains(&lon) {
        Ok(LatLon::new(lat, lon))
    } else {
        Err(ApiError::BadRequest(format!("no such place: {lat},{lon}")))
    }
}

#[derive(Serialize)]
struct RouteJson<'a> {
    name: &'a str,
    kind: &'static str,
    color: Option<String>,
    text: Option<String>,
}

fn route_json(route: &Route) -> RouteJson<'_> {
    let hex = |[r, g, b]: [u8; 3]| format!("#{r:02X}{g:02X}{b:02X}");
    RouteJson {
        name: &route.name,
        kind: match route.kind {
            RouteKind::Metro => "metro",
            RouteKind::Tram => "tram",
            RouteKind::Rail => "rail",
            RouteKind::Bus | RouteKind::Other => "bus",
        },
        color: route.color.map(hex),
        text: route.text_color.map(hex),
    }
}

#[derive(Serialize)]
struct StopJson<'a> {
    id: &'a str,
    name: &'a str,
    code: Option<&'a str>,
    street: Option<&'a str>,
    lat: f64,
    lon: f64,
}

fn stop_json(timetable: &Timetable, stop: StopIdx) -> StopJson<'_> {
    let pole = &timetable.stops()[stop];
    // Platforms of single-line metro stations have no code: name them after the line.
    let metro_line = || {
        let mut routes = timetable.routes_at(stop);
        let route = routes
            .next()
            .filter(|route| route.kind == RouteKind::Metro)?;
        routes.next().is_none().then_some(route.name.as_str())
    };
    StopJson {
        id: &pole.id,
        name: &pole.name,
        code: pole.code.as_deref().or_else(metro_line),
        street: pole.street.as_deref(),
        lat: pole.lat,
        lon: pole.lon,
    }
}

#[derive(Serialize)]
struct DepartureJson<'a> {
    route: RouteJson<'a>,
    headsign: &'a str,
    stop: &'a str,
    scheduled: i64,
    expected: i64,
    delay: Option<i32>,
    live: bool,
    approximate: bool,
    stops_away: Option<usize>,
}

fn departure_json<'t>(timetable: &'t Timetable, departure: &Departure<'t>) -> DepartureJson<'t> {
    DepartureJson {
        route: route_json(departure.route),
        headsign: departure.headsign,
        stop: &timetable.stops()[departure.stop].id,
        scheduled: departure.scheduled.as_second(),
        expected: departure.expected.as_second(),
        delay: departure.live.map(|live| live.delay),
        live: departure.live.is_some(),
        approximate: departure.approximate,
        stops_away: departure.live.map(|live| live.stops_away),
    }
}

/// A pole near a point.
#[derive(Serialize)]
struct NearStopJson<'a> {
    #[serde(flatten)]
    stop: StopJson<'a>,
    distance: u32,
    walk: u32,
}

fn near_stop_json(timetable: &Timetable, stop: StopIdx, distance: f64) -> NearStopJson<'_> {
    NearStopJson {
        stop: stop_json(timetable, stop),
        distance: distance.round() as u32,
        walk: walking_time(distance),
    }
}

#[derive(Deserialize)]
struct NearbyQuery {
    lat: f64,
    lon: f64,
    radius: Option<f64>,
    limit: Option<usize>,
    departures: Option<usize>,
}

async fn nearby(
    State(state): State<AppState>,
    query: Result<Query<NearbyQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    #[derive(Serialize)]
    struct Pole<'a> {
        #[serde(flatten)]
        stop: NearStopJson<'a>,
        routes: Vec<RouteJson<'a>>,
        departures: Vec<DepartureJson<'a>>,
    }

    let Query(query) = query?;
    let current = current(&state)?;
    let timetable = &current.data.timetable;
    let here = point(query.lat, query.lon)?;
    let radius = query.radius.unwrap_or(600.0).clamp(1.0, 2_000.0);
    let per_stop = query.departures.unwrap_or(4).min(20);
    let now = Timestamp::now();
    let stops: Vec<Pole> = timetable
        .nearby(here, radius)
        .into_iter()
        .take(query.limit.unwrap_or(12).min(40))
        .map(|(stop, distance)| Pole {
            stop: near_stop_json(timetable, stop, distance),
            routes: timetable.routes_at(stop).map(route_json).collect(),
            departures: timetable
                .departures(&[stop], now, per_stop, &current.live)
                .iter()
                .map(|departure| departure_json(timetable, departure))
                .collect(),
        })
        .collect();
    let body = serde_json::json!({ "now": now.as_second(), "stops": stops });
    Ok(Json(body).into_response())
}

#[derive(Deserialize)]
struct DeparturesQuery {
    stops: String,
    limit: Option<usize>,
}

async fn departures(
    State(state): State<AppState>,
    query: Result<Query<DeparturesQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let current = current(&state)?;
    let timetable = &current.data.timetable;
    let stops: Vec<StopIdx> = query
        .stops
        .split(',')
        .filter_map(|id| current.data.stop_ids.get(id.trim()).copied())
        .collect();
    if stops.is_empty() {
        return Err(ApiError::NotFound("no such stop"));
    }
    let now = Timestamp::now();
    let departures: Vec<DepartureJson> = timetable
        .departures(
            &stops,
            now,
            query.limit.unwrap_or(12).min(50),
            &current.live,
        )
        .iter()
        .map(|departure| departure_json(timetable, departure))
        .collect();
    let body = serde_json::json!({ "now": now.as_second(), "departures": departures });
    Ok(Json(body).into_response())
}

#[derive(Deserialize)]
struct PlanQuery {
    from: String,
    to: String,
    at: Option<i64>,
}

/// Parses `LAT,LON`.
fn parse_point(text: &str) -> Result<LatLon, ApiError> {
    let invalid = || ApiError::BadRequest(format!("expected LAT,LON, got `{text}`"));
    let (lat, lon) = text.split_once(',').ok_or_else(invalid)?;
    let lat = lat.trim().parse().map_err(|_| invalid())?;
    let lon = lon.trim().parse().map_err(|_| invalid())?;
    point(lat, lon)
}

async fn plan(
    State(state): State<AppState>,
    query: Result<Query<PlanQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    let Query(query) = query?;
    let current = current(&state)?;
    let (from, to) = (parse_point(&query.from)?, parse_point(&query.to)?);
    let now = Timestamp::now();
    let at = match query.at {
        Some(at) => Timestamp::from_second(at)
            .map_err(|_| ApiError::BadRequest(format!("no such time: {at}")))?,
        None => now,
    };
    let _turn = PLANNING
        .acquire()
        .await
        .expect("the semaphore is never closed");
    // Keep the CPU-heavy part off the async workers.
    let response = tokio::task::spawn_blocking(move || {
        let timetable = &current.data.timetable;
        let plan = timetable.plan(from, to, at, &current.live);
        let options: Vec<_> = plan
            .options
            .iter()
            .map(|journey| journey_json(timetable, journey))
            .collect();
        let walk = plan.walk.map(|walk| {
            serde_json::json!({ "seconds": walk.seconds, "distance": walk.distance.round() })
        });
        let body = serde_json::json!({ "now": now.as_second(), "options": options, "walk": walk });
        Json(body).into_response()
    });
    response
        .await
        .map_err(|_| ApiError::BadRequest("could not plan that journey".to_owned()))
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LegJson<'a> {
    Walk {
        from: Option<StopJson<'a>>,
        to: Option<StopJson<'a>>,
        seconds: u32,
        distance: u32,
    },
    Ride {
        route: RouteJson<'a>,
        headsign: &'a str,
        from: StopJson<'a>,
        to: StopJson<'a>,
        departure: i64,
        arrival: i64,
        scheduled: i64,
        delay: Option<i32>,
        live: bool,
        approximate: bool,
        stops: usize,
    },
}

#[derive(Serialize)]
struct JourneyJson<'a> {
    start: NearStopJson<'a>,
    leave: i64,
    arrive: i64,
    transfers: usize,
    legs: Vec<LegJson<'a>>,
}

fn journey_json<'t>(timetable: &'t Timetable, journey: &Journey) -> JourneyJson<'t> {
    let stop = |stop: Option<StopIdx>| stop.map(|stop| stop_json(timetable, stop));
    let start = match journey.legs.first() {
        Some(Leg::Walk(walk)) => near_stop_json(timetable, journey.start(), walk.distance),
        _ => near_stop_json(timetable, journey.start(), 0.0),
    };
    let legs = journey
        .legs
        .iter()
        .map(|leg| match leg {
            Leg::Walk(walk) => LegJson::Walk {
                from: stop(walk.from),
                to: stop(walk.to),
                seconds: walk.seconds,
                distance: walk.distance.round() as u32,
            },
            Leg::Ride(ride) => LegJson::Ride {
                route: route_json(timetable.route_of(ride.trip)),
                headsign: timetable.headsign_of(ride.trip),
                from: stop_json(timetable, ride.from),
                to: stop_json(timetable, ride.to),
                departure: ride.departure.as_second(),
                arrival: ride.arrival.as_second(),
                scheduled: ride.scheduled.as_second(),
                delay: ride.live.map(|live| live.delay),
                live: ride.live.is_some(),
                approximate: timetable.is_approximate(ride.trip),
                stops: ride.stops,
            },
        })
        .collect();
    JourneyJson {
        start,
        leave: journey.leave.as_second(),
        arrive: journey.arrive.as_second(),
        transfers: journey.transfers(),
        legs,
    }
}

#[derive(Deserialize)]
struct StopsQuery {
    q: String,
}

async fn stops(
    State(state): State<AppState>,
    query: Result<Query<StopsQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    #[derive(Serialize)]
    struct Group<'a> {
        name: &'a str,
        lat: f64,
        lon: f64,
        poles: Vec<StopJson<'a>>,
    }

    let Query(query) = query?;
    let current = current(&state)?;
    let timetable = &current.data.timetable;
    let groups: Vec<Group> = search(&current.data.groups, &query.q)
        .into_iter()
        .take(8)
        .map(|group| {
            let poles: Vec<StopJson> = group
                .stops
                .iter()
                .map(|&stop| stop_json(timetable, stop))
                .collect();
            let count = poles.len().max(1) as f64;
            Group {
                name: &group.name,
                lat: poles.iter().map(|pole| pole.lat).sum::<f64>() / count,
                lon: poles.iter().map(|pole| pole.lon).sum::<f64>() / count,
                poles,
            }
        })
        .collect();
    Ok(Json(serde_json::json!({ "stops": groups })).into_response())
}

#[derive(Deserialize)]
struct VehiclesQuery {
    lat: f64,
    lon: f64,
    radius: Option<f64>,
}

async fn vehicles(
    State(state): State<AppState>,
    query: Result<Query<VehiclesQuery>, QueryRejection>,
) -> Result<Response, ApiError> {
    #[derive(Serialize)]
    struct VehicleJson<'a> {
        route: RouteJson<'a>,
        headsign: &'a str,
        lat: f64,
        lon: f64,
        bearing: Option<f32>,
        delay: Option<i32>,
        number: Option<&'a str>,
        updated: Option<i64>,
    }

    let Query(query) = query?;
    let current = current(&state)?;
    let timetable = &current.data.timetable;
    let here = point(query.lat, query.lon)?;
    let radius = query.radius.unwrap_or(1_500.0).clamp(1.0, 5_000.0);
    let vehicles: Vec<VehicleJson> = current
        .live
        .vehicles
        .iter()
        .filter(|vehicle| here.distance(vehicle.position.position) <= radius)
        .filter_map(|vehicle| {
            let trip = vehicle.trip?;
            Some(VehicleJson {
                route: route_json(timetable.route_of(trip)),
                headsign: timetable.headsign_of(trip),
                lat: vehicle.position.position.lat,
                lon: vehicle.position.position.lon,
                bearing: vehicle.position.bearing,
                delay: vehicle.progress.map(|progress| progress.delay),
                number: vehicle.position.label.as_deref(),
                updated: vehicle.position.timestamp.map(Timestamp::as_second),
            })
        })
        .collect();
    let body = serde_json::json!({ "now": Timestamp::now().as_second(), "vehicles": vehicles });
    Ok(Json(body).into_response())
}

async fn status(State(state): State<AppState>) -> Result<Response, ApiError> {
    let current = current(&state)?;
    let timetable = &current.data.timetable;
    let body = serde_json::json!({
        "now": Timestamp::now().as_second(),
        "timetable": {
            "version": timetable.version(),
            "days": timetable.days().map(|day| day.to_string()),
        },
        "realtime": {
            "updated": current.live.published.map(Timestamp::as_second),
            "vehicles": current.live.vehicles.len(),
            "tracked": current.live.tracked(),
        },
    });
    Ok(Json(body).into_response())
}
