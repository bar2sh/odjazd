//! Where Warsaw's buses and trams are right now, and how late that makes them.

use std::time::Duration;

use anyhow::{Context, Result};
use odjazd_core::{
    Timetable,
    realtime::{Positions, Realtime},
};

/// Where every bus and tram is (GTFS-Realtime), from the City Hall's open data, matched to
/// the timetable's trips by Mikołaj Kuranowski. New positions come every 10–20 seconds.
const WARSAW_VEHICLES: &str = "https://mkuran.pl/gtfs/warsaw/vehicles.pb";

/// The positions are a few dozen kilobytes. Without a connection, the board soon makes do
/// with the timetable.
const TIMEOUT: Duration = Duration::from_secs(5);

/// Far more than the positions of every vehicle in Warsaw.
const MAX_SIZE: u64 = 16 * 1024 * 1024;

/// Downloads the vehicle positions.
pub fn positions() -> Result<Positions> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(crate::USER_AGENT)
        .timeout_global(Some(TIMEOUT))
        .build()
        .into();
    let bytes = agent
        .get(WARSAW_VEHICLES)
        .call()
        .and_then(|mut response| {
            response
                .body_mut()
                .with_config()
                .limit(MAX_SIZE)
                .read_to_vec()
        })
        .context("cannot download the vehicle positions")?;
    Positions::decode(&bytes).context("cannot read the vehicle positions")
}

/// Downloads the vehicle positions and works out how late each vehicle is. `previous` is
/// what the last positions gave, as for [`Timetable::track`].
pub fn track(timetable: &Timetable, previous: &Realtime) -> Result<Realtime> {
    Ok(timetable.track(positions()?, previous))
}
