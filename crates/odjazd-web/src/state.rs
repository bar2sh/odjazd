//! The timetable and live data the API answers from, and keeping the timetable fresh.

use std::{
    collections::HashMap,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{Arc, PoisonError, RwLock},
    time::{Duration, Instant, SystemTime},
};

use anyhow::{Context, Result};
use jiff::{SignedDuration, Zoned, civil::Date};
use odjazd_core::{Feed, StopGroup, StopIdx, Timetable, realtime::Realtime};
use tracing::{error, info};

use crate::USER_AGENT;

/// A timetable on disk older than this is replaced by a fresh download. Warsaw's is
/// rebuilt every night, shortly after midnight.
const REFRESH_AFTER: Duration = Duration::from_secs(20 * 60 * 60);

/// How often to check whether the timetable needs downloading or reloading.
const CHECK_EVERY: Duration = Duration::from_secs(10 * 60);

/// For loading, a new day starts at this hour: until then the night buses of the day
/// before are still running.
const NEW_DAY_AT_HOUR: i64 = 3;

/// Everything the API needs from one timetable.
pub struct Data {
    pub timetable: Timetable,
    /// For finding stops by name.
    pub groups: Vec<StopGroup>,
    pub stop_ids: HashMap<String, StopIdx>,
}

/// The current timetable, and the live data worked out for it.
#[derive(Clone)]
pub struct Current {
    pub data: Arc<Data>,
    pub live: Arc<Realtime>,
}

/// Shared between the request handlers and the background tasks.
#[derive(Clone, Default)]
pub struct AppState(Arc<RwLock<Option<Current>>>);

impl AppState {
    /// `None` until the first timetable has loaded.
    pub fn current(&self) -> Option<Current> {
        self.0
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn set_data(&self, data: Data) {
        let current = Current {
            data: Arc::new(data),
            live: Arc::default(),
        };
        *self.0.write().unwrap_or_else(PoisonError::into_inner) = Some(current);
    }

    /// Stores live data worked out for `data`, unless the timetable was replaced meanwhile.
    pub fn set_live(&self, data: &Arc<Data>, live: Realtime) {
        let mut current = self.0.write().unwrap_or_else(PoisonError::into_inner);
        if let Some(current) = current.as_mut()
            && Arc::ptr_eq(&current.data, data)
        {
            current.live = Arc::new(live);
        }
    }
}

/// Keeps a current timetable loaded, forever: downloads one when there is none on disk or
/// it is a day old, and reloads it when it changes or a new day begins.
pub async fn keep_timetable(state: AppState, feed_dir: PathBuf, url: String) {
    // The day and the download the loaded timetable is for.
    let mut loaded: Option<(Date, SystemTime)> = None;
    loop {
        let task = {
            let (state, feed_dir, url) = (state.clone(), feed_dir.clone(), url.clone());
            tokio::task::spawn_blocking(move || refresh(&state, &feed_dir, &url, loaded))
        };
        match task.await {
            Ok(Ok(now_loaded)) => loaded = now_loaded,
            Ok(Err(err)) => error!("{err:#}"),
            Err(err) => error!("loading the timetable failed: {err}"),
        }
        let wait = if loaded.is_some() {
            CHECK_EVERY
        } else {
            Duration::from_secs(60)
        };
        tokio::time::sleep(wait).await;
    }
}

fn refresh(
    state: &AppState,
    feed_dir: &Path,
    url: &str,
    loaded: Option<(Date, SystemTime)>,
) -> Result<Option<(Date, SystemTime)>> {
    // On the first run, load what is on disk at once and download a fresh one after.
    let stale = modified(feed_dir)
        .is_none_or(|time| time.elapsed().is_ok_and(|age| age > REFRESH_AFTER) && loaded.is_some());
    // On failure, keep serving the old timetable: it covers the next few days.
    if stale && let Err(err) = download(url, feed_dir) {
        error!("{err:#}");
    }
    let modified = modified(feed_dir).context("no timetable to load yet")?;

    let feed = Feed::open(feed_dir).context("cannot read the timetable")?;
    let now = Zoned::now().with_time_zone(feed.timezone().clone());
    let today = now
        .checked_sub(SignedDuration::from_hours(NEW_DAY_AT_HOUR))?
        .date();
    if loaded == Some((today, modified)) {
        return Ok(loaded);
    }

    let started = Instant::now();
    let timetable = Timetable::build(&feed, today).context("cannot load the timetable")?;
    let groups = feed.stop_groups();
    let stop_ids = feed
        .stops()
        .iter()
        .enumerate()
        .map(|(i, stop)| (stop.id.clone(), i))
        .collect();
    info!(
        "loaded the timetable for {today} (version {}) in {:.1} s",
        timetable.version().unwrap_or("unknown"),
        started.elapsed().as_secs_f64()
    );
    state.set_data(Data {
        timetable,
        groups,
        stop_ids,
    });
    Ok(Some((today, modified)))
}

/// When the timetable on disk was unpacked.
fn modified(feed_dir: &Path) -> Option<SystemTime> {
    feed_dir.metadata().and_then(|meta| meta.modified()).ok()
}

/// Downloads the GTFS archive at `url` and installs it in `feed_dir`.
fn download(url: &str, feed_dir: &Path) -> Result<()> {
    info!("downloading the timetable from {url}");
    let data_dir = feed_dir
        .parent()
        .context("the timetable directory has no parent")?;
    fs::create_dir_all(data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;
    let archive = data_dir.join("feed.zip.part");

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .user_agent(USER_AGENT)
        .timeout_global(Some(Duration::from_secs(15 * 60)))
        .build()
        .into();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let mut file =
        File::create(&archive).with_context(|| format!("cannot create {}", archive.display()))?;
    io::copy(&mut response.body_mut().as_reader(), &mut file)
        .with_context(|| format!("download from {url} broke off"))?;

    let installed =
        odjazd_core::install(&archive, feed_dir).context("the download is not a usable GTFS feed");
    fs::remove_file(&archive).ok();
    installed?;
    info!("timetable saved in {}", feed_dir.display());
    Ok(())
}
