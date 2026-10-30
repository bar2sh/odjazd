//! The engine behind [odjazd]: reads a [GTFS] timetable, finds stops by name and answers
//! "what leaves from here next?".
//!
//! The crate does no networking and no terminal output, so the same code can later run in
//! the browser (WebAssembly) or behind an HTTP API.
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
mod calendar;
mod error;
mod feed;
mod search;
mod time;

pub use board::Departure;
pub use error::Error;
pub use feed::{Feed, Route, Stop, StopIdx};
pub use search::{StopGroup, normalize, search};
pub use time::GtfsTime;
