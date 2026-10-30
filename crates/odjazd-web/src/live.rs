//! Following the live vehicle positions.

use std::{sync::Arc, time::Duration};

use anyhow::{Context, Result};
use odjazd_core::realtime::Positions;
use tokio::time::MissedTickBehavior;
use tracing::{info, warn};

use crate::{USER_AGENT, state::AppState};

/// Warsaw publishes new positions every 10–20 seconds.
const POLL_EVERY: Duration = Duration::from_secs(15);

/// Downloads the vehicle positions at `url` every few seconds, forever, and works out
/// the delays they imply for the current timetable.
pub async fn follow(state: AppState, url: String) {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(10)))
        .build()
        .into();
    let mut etag: Option<String> = None;
    let mut positions: Option<Positions> = None;
    let mut failing = false;
    let mut ticks = tokio::time::interval(POLL_EVERY);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        ticks.tick().await;
        // At startup, begin as soon as the timetable is in.
        let current = loop {
            match state.current() {
                Some(current) => break current,
                None => tokio::time::sleep(Duration::from_secs(1)).await,
            }
        };

        let request = {
            let (agent, url, etag) = (agent.clone(), url.clone(), etag.clone());
            tokio::task::spawn_blocking(move || fetch(&agent, &url, etag.as_deref()))
        };
        match request
            .await
            .context("download task failed")
            .and_then(|result| result)
        {
            Ok(Some((new_positions, new_etag))) => {
                if failing {
                    info!("vehicle positions are back");
                    failing = false;
                }
                positions = Some(new_positions);
                etag = new_etag;
            }
            // Unchanged; work them out again only for a timetable that has just loaded.
            Ok(None) if current.live.published.is_some() => continue,
            Ok(None) => {}
            Err(err) => {
                if !failing {
                    warn!("cannot get vehicle positions: {err:#}");
                    failing = true;
                }
                continue;
            }
        }

        let Some(positions) = positions.clone() else {
            continue;
        };
        let data = Arc::clone(&current.data);
        let tracking = tokio::task::spawn_blocking(move || {
            let live = data.timetable.track(positions, &current.live);
            (data, live)
        });
        if let Ok((data, live)) = tracking.await {
            state.set_live(&data, live);
        }
    }
}

/// The positions at `url`, or `None` when they haven't changed since the version tagged
/// `etag`. Also returns the new tag.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    etag: Option<&str>,
) -> Result<Option<(Positions, Option<String>)>> {
    let mut request = agent.get(url);
    if let Some(etag) = etag {
        request = request.header("If-None-Match", etag);
    }
    let mut response = request.call()?;
    if response.status() == 304 {
        return Ok(None);
    }
    let etag = response
        .headers()
        .get("etag")
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    let bytes = response.body_mut().read_to_vec()?;
    Ok(Some((Positions::decode(&bytes)?, etag)))
}
