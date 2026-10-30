use std::{
    env,
    io::{self, IsTerminal},
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, Parser, Subcommand};
use jiff::{Zoned, civil::Time};
use odjazd_core::{Feed, search};

mod download;
mod render;
mod tui;

/// Warsaw's timetable: ZTM Warszawa data, converted to GTFS by Mikołaj Kuranowski.
const WARSAW_FEED: &str = "https://mkuran.pl/gtfs/warsaw.zip";

/// Past this age, suggest downloading a fresh timetable.
const STALE_AFTER: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// Next departures from any Warsaw public transport stop, right in your terminal.
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
    /// Download the latest timetable
    Update {
        /// GTFS feed to download instead of Warsaw's
        #[arg(long, default_value = WARSAW_FEED)]
        url: String,
    },
    /// List stops matching a name
    Stops {
        /// Stop name, or part of it
        query: String,
    },
}

#[derive(Args)]
struct BoardArgs {
    /// Stop name. Typos, missing Polish letters and "plac" for "Pl." are fine.
    /// Leave it out to search for one on the live board
    stop: Option<String>,

    /// Keep the board open, counting down and refreshing itself
    #[arg(short, long, conflicts_with = "at")]
    live: bool,

    /// How many departures to show
    #[arg(short = 'n', long, default_value_t = 12)]
    limit: usize,

    /// Show departures from this time today instead of now
    #[arg(long, value_name = "HH:MM")]
    at: Option<Time>,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let feed_dir = data_dir()?.join("feed");
    match cli.command {
        Some(Command::Update { url }) => download::update(&url, &feed_dir),
        Some(Command::Stops { query }) => list_stops(&open(&feed_dir)?, &query),
        None => {
            let interactive = io::stdin().is_terminal() && io::stdout().is_terminal();
            match &cli.board.stop {
                Some(stop) if !cli.board.live => show_board(&open(&feed_dir)?, stop, &cli.board),
                stop if interactive => tui::run(&open(&feed_dir)?, stop.as_deref()),
                Some(_) => bail!("the live board needs a terminal; leave out --live"),
                None => Ok(Cli::command().print_help()?),
            }
        }
    }
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

fn show_board(feed: &Feed, query: &str, args: &BoardArgs) -> Result<()> {
    let groups = feed.stop_groups();
    let found = search(&groups, query);
    let Some((group, others)) = found.split_first() else {
        bail!("no stop called anything like \"{query}\"");
    };
    let now = Zoned::now().with_time_zone(feed.timezone().clone());
    let from = match args.at {
        Some(time) => now.with().time(time).build()?,
        None => now,
    };
    let departures = feed.departures(&group.stops, &from, args.limit)?;
    render::board(group, &from, &departures, &others[..others.len().min(3)]);
    Ok(())
}

fn list_stops(feed: &Feed, query: &str) -> Result<()> {
    let groups = feed.stop_groups();
    let found = search(&groups, query);
    if found.is_empty() {
        bail!("no stop called anything like \"{query}\"");
    }
    render::stops(feed, &found[..found.len().min(15)]);
    Ok(())
}
