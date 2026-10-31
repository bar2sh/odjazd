use std::{
    env,
    io::{self, IsTerminal},
    panic,
    path::{Path, PathBuf},
    thread,
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, Parser, Subcommand};
use jiff::{
    Zoned,
    civil::{Date, Time},
};
use odjazd_core::{Feed, LatLon, StopGroup, Timetable, realtime::Realtime, search};

mod download;
#[cfg(test)]
mod fixtures;
mod live;
mod render;
mod tui;

/// Warsaw's timetable: ZTM Warszawa data, converted to GTFS by Mikołaj Kuranowski.
const WARSAW_FEED: &str = "https://mkuran.pl/gtfs/warsaw.zip";

/// How odjazd introduces itself to the servers it downloads from.
const USER_AGENT: &str = concat!(
    "odjazd/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/bar2sh/odjazd)"
);

/// Past this age, suggest downloading a fresh timetable.
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Next departures from any Warsaw public transport stop, right in your terminal, with
/// live delays.
#[derive(Parser)]
#[command(version, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    #[command(flatten)]
    board: BoardArgs,
}

#[derive(Subcommand)]
enum Command {
    /// Plan a journey: the quickest ways from one stop to another
    Route(RouteArgs),
    /// List stops matching a name
    Stops {
        /// Stop name, or part of it
        query: String,
    },
    /// Download the latest timetable
    Update {
        /// GTFS feed to download instead of Warsaw's
        #[arg(long, default_value = WARSAW_FEED)]
        url: String,
    },
}

#[derive(Args)]
struct BoardArgs {
    /// Stop name. Typos, missing Polish letters and "plac" for "Pl." are fine.
    /// Leave it out to search for one on the live board
    stop: Option<String>,

    /// Keep the board open, counting down and refreshing itself
    #[arg(short, long, conflicts_with = "at")]
    watch: bool,

    /// How many departures to show
    #[arg(short = 'n', long, default_value_t = 12)]
    limit: usize,

    /// Show departures from this time today instead of now
    #[arg(long, value_name = "HH:MM", value_parser = time_of_day)]
    at: Option<Time>,

    #[command(flatten)]
    live: LiveArgs,
}

#[derive(Args)]
struct RouteArgs {
    /// Where from: a stop name, as for the board
    from: String,

    /// Where to
    to: String,

    /// How many ways to show
    #[arg(short = 'n', long, default_value_t = 3)]
    limit: usize,

    /// Set off at this time today instead of now
    #[arg(long, value_name = "HH:MM", value_parser = time_of_day)]
    at: Option<Time>,

    #[command(flatten)]
    live: LiveArgs,
}

#[derive(Args)]
struct LiveArgs {
    /// Go by the timetable alone, without downloading where the vehicles are
    #[arg(long)]
    offline: bool,
}

fn main() -> Result<()> {
    let result = run(Cli::parse());
    // `odjazd centrum | head -3`: what did not fit was not wanted.
    let closed = result
        .as_ref()
        .err()
        .and_then(|error| error.downcast_ref::<io::Error>());
    if closed.is_some_and(|error| error.kind() == io::ErrorKind::BrokenPipe) {
        return Ok(());
    }
    result
}

fn run(cli: Cli) -> Result<()> {
    let feed_dir = data_dir()?.join("feed");
    match cli.command {
        Some(Command::Route(args)) => route(&open(&feed_dir)?, &args),
        Some(Command::Stops { query }) => list_stops(&open(&feed_dir)?, &query),
        Some(Command::Update { url }) => download::update(&url, &feed_dir),
        None => {
            let board = &cli.board;
            let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
            match &board.stop {
                Some(stop) if !board.watch => show_board(&open(&feed_dir)?, stop, board),
                stop if interactive => {
                    tui::run(open(&feed_dir)?, stop.as_deref(), board.live.offline)
                }
                Some(_) => bail!("the live board needs a terminal; leave out --watch"),
                None => Ok(Cli::command().print_help()?),
            }
        }
    }
}

/// A time of day, like `8:15` or `23:50`.
fn time_of_day(text: &str) -> Result<Time, String> {
    let text = match text.find(':') {
        Some(1) => format!("0{text}"),
        _ => text.to_owned(),
    };
    text.parse()
        .map_err(|_| "expected a time of day, like 8:15 or 23:50".to_owned())
}

/// Where the timetable is kept: `$ODJAZD_DATA_DIR`, or the system cache directory.
fn data_dir() -> Result<PathBuf> {
    if let Some(dir) = env::var_os("ODJAZD_DATA_DIR") {
        return Ok(dir.into());
    }
    let cache = dirs::cache_dir().context("cannot find a cache directory; set ODJAZD_DATA_DIR")?;
    Ok(cache.join("odjazd"))
}

/// Opens the downloaded timetable, downloading it first on the very first run.
fn open(feed_dir: &Path) -> Result<Feed> {
    if !feed_dir.exists() {
        eprintln!("No timetable yet: downloading Warsaw's (about 110 MB, only this once).");
        download::update(WARSAW_FEED, feed_dir)?;
    }
    let age = feed_dir
        .metadata()
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.elapsed().ok());
    if age.is_some_and(|age| age > STALE_AFTER) {
        eprintln!("The timetable is over a week old; run `odjazd update` for a fresh one.");
    }
    Feed::open(feed_dir).context("cannot read the timetable; try `odjazd update`")
}

/// The timetable of the service days around `day` and, if `live`, how late the vehicles
/// on the road are, from their positions downloaded meanwhile. Without those, the times
/// are the timetable's.
fn load(feed: &Feed, day: Date, live: bool) -> Result<(Timetable, Option<Realtime>)> {
    let (timetable, positions) = thread::scope(|scope| {
        let positions = live.then(|| scope.spawn(live::positions));
        let timetable = Timetable::build(feed, day);
        (timetable, positions.map(|download| download.join()))
    });
    let timetable = timetable.context("cannot read the timetable; try `odjazd update`")?;
    let realtime = match positions {
        None => None,
        Some(Ok(Ok(positions))) => Some(timetable.track(positions, &Realtime::default())),
        Some(Ok(Err(error))) => {
            eprintln!("No live delays ({error:#}), so the times are the timetable's.");
            None
        }
        Some(Err(crash)) => panic::resume_unwind(crash),
    };
    Ok((timetable, realtime))
}

/// The stop best matching `query`, and a few more that also match.
fn find<'g>(groups: &'g [StopGroup], query: &str) -> Result<(&'g StopGroup, Vec<&'g StopGroup>)> {
    let found = search(groups, query);
    let Some((&group, others)) = found.split_first() else {
        bail!("no stop called anything like \"{query}\"");
    };
    Ok((group, others.iter().take(3).copied().collect()))
}

/// `time` today, or now.
fn today_at(feed: &Feed, time: Option<Time>) -> Result<Zoned> {
    let now = Zoned::now().with_time_zone(feed.timezone().clone());
    Ok(match time {
        Some(time) => now.with().time(time).build()?,
        None => now,
    })
}

fn show_board(feed: &Feed, query: &str, args: &BoardArgs) -> Result<()> {
    let groups = feed.stop_groups();
    let (group, others) = find(&groups, query)?;
    let from = today_at(feed, args.at)?;
    // Where the vehicles are now says nothing about another time.
    let live = args.at.is_none() && !args.live.offline;
    let (timetable, realtime) = load(feed, from.date(), live)?;
    let none = Realtime::default();
    let departures = timetable.departures(
        &group.stops,
        from.timestamp(),
        args.limit,
        realtime.as_ref().unwrap_or(&none),
    );
    let out = &mut anstream::stdout().lock();
    render::board(
        out,
        &timetable,
        group,
        &from,
        &departures,
        &others,
        realtime.as_ref(),
    )?;
    Ok(())
}

fn route(feed: &Feed, args: &RouteArgs) -> Result<()> {
    let groups = feed.stop_groups();
    let (from, _) = find(&groups, &args.from)?;
    let (to, _) = find(&groups, &args.to)?;
    if from.name == to.name {
        bail!(
            "{} is both where you are and where you are going",
            from.name
        );
    }
    let start = today_at(feed, args.at)?;
    let live = args.at.is_none() && !args.live.offline;
    let (timetable, realtime) = load(feed, start.date(), live)?;
    let none = Realtime::default();
    let plan = timetable.plan(
        centre(feed, from),
        centre(feed, to),
        start.timestamp(),
        realtime.as_ref().unwrap_or(&none),
    );
    let out = &mut anstream::stdout().lock();
    render::journeys(out, &timetable, from, to, &start, &plan, args.limit)?;
    Ok(())
}

/// The middle of a stop's poles.
fn centre(feed: &Feed, group: &StopGroup) -> LatLon {
    let poles = group
        .stops
        .iter()
        .map(|&stop| feed.stops()[stop].position());
    let (lat, lon) = poles.fold((0.0, 0.0), |(lat, lon), pole| {
        (lat + pole.lat, lon + pole.lon)
    });
    let count = group.stops.len() as f64;
    LatLon::new(lat / count, lon / count)
}

fn list_stops(feed: &Feed, query: &str) -> Result<()> {
    let groups = feed.stop_groups();
    let found = search(&groups, query);
    if found.is_empty() {
        bail!("no stop called anything like \"{query}\"");
    }
    render::stops(
        &mut anstream::stdout().lock(),
        feed,
        &found[..found.len().min(15)],
    )?;
    Ok(())
}
