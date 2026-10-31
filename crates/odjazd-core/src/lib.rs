//! The engine behind [odjazd]: reads a [GTFS] timetable, finds stops by name and answers
//! "what leaves from here next?".
//!
//! Two ways in:
//!
//! - [`Feed`] streams the timetable for a single query, which is what the command-line
//!   app does.
//! - [`Timetable`] holds a few days of it in memory for a server: departure boards with
//!   live delays worked out from vehicle positions ([`realtime`]), and journey planning
//!   ([`plan`]).
//!
//! The crate does no networking and no terminal output, so the same code can run behind
//! an HTTP API or, later, in the browser (WebAssembly). [`install`] unpacks a timetable
//! someone else has downloaded.
//!
//! ```no_run
//! use odjazd_core::{Feed, search};
//!
//! let feed = Feed::open("path/to/unpacked/gtfs")?;
//! let groups = feed.stop_groups();
//! let stop = search(&groups, "plac narutowicza")[0];
//! for departure in feed.departures(&stop.stops, &jiff::Zoned::now(), 5)? {
//!     let time = departure.time.strftime("%H:%M");
//!     println!("{time} {} {}", departure.route.name, departure.headsign);
//! }
//! # Ok::<(), odjazd_core::Error>(())
//! ```
//!
//! [odjazd]: https://github.com/bar2sh/odjazd
//! [GTFS]: https://gtfs.org/documentation/schedule/reference/

mod board;
#[cfg(feature = "cache")]
mod cache;
mod calendar;
mod error;
mod feed;
mod geo;
mod install;
pub mod plan;
pub mod realtime;
mod search;
mod time;
pub mod timetable;

pub use board::Departure;
pub use error::Error;
pub use feed::{Feed, Route, RouteKind, Stop, StopIdx};
pub use geo::{LatLon, walking_time};
pub use install::install;
pub use search::{StopGroup, normalize, search};
pub use time::GtfsTime;
pub use timetable::{Live, Timetable, TripIdx};
