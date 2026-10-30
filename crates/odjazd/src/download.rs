use std::{
    fs::{self, File},
    io::{self, IsTerminal, Read},
    path::Path,
};

use anyhow::{Context, Result};
use odjazd_core::Feed;

/// The tables odjazd reads; the rest of the archive (shapes, fares) is skipped.
const TABLES: &[&str] = &[
    "agency.txt",
    "stops.txt",
    "routes.txt",
    "trips.txt",
    "stop_times.txt",
    "calendar.txt",
    "calendar_dates.txt",
    "frequencies.txt",
    "feed_info.txt",
    "attributions.txt",
];

/// Downloads the GTFS archive at `url` and unpacks it into `feed_dir`. The previous
/// timetable is replaced only once the new one is unpacked and opens fine.
pub fn update(url: &str, feed_dir: &Path) -> Result<()> {
    let data_dir = feed_dir
        .parent()
        .context("the timetable directory has no parent")?;
    fs::create_dir_all(data_dir)
        .with_context(|| format!("cannot create {}", data_dir.display()))?;

    let archive = data_dir.join("feed.zip.part");
    download(url, &archive)?;
    let staging = data_dir.join("feed.new");
    unpack(&archive, &staging)?;
    fs::remove_file(&archive)?;
    Feed::open(&staging).context("the download is not a usable GTFS feed")?;

    if feed_dir.exists() {
        fs::remove_dir_all(feed_dir)?;
    }
    fs::rename(&staging, feed_dir)?;
    eprintln!("Timetable saved in {}", feed_dir.display());
    Ok(())
}

fn download(url: &str, to: &Path) -> Result<()> {
    let mut response = ureq::get(url)
        .call()
        .with_context(|| format!("cannot download {url}"))?;
    let total = response
        .body()
        .content_length()
        .filter(|_| io::stderr().is_terminal());
    let mut body = Progress {
        inner: response.body_mut().as_reader(),
        read: 0,
        total,
        percent: None,
    };
    let mut file = File::create(to).with_context(|| format!("cannot create {}", to.display()))?;
    eprint!("Downloading timetable…");
    io::copy(&mut body, &mut file).with_context(|| format!("download from {url} broke off"))?;
    eprintln!(" done ({:.0} MB)", body.read as f64 / 1e6);
    Ok(())
}

fn unpack(archive: &Path, dir: &Path) -> Result<()> {
    eprintln!("Unpacking…");
    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::create_dir_all(dir)?;
    let mut zip =
        zip::ZipArchive::new(File::open(archive)?).context("the download is not a zip file")?;
    for &table in TABLES {
        let mut entry = match zip.by_name(table) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => continue,
            Err(err) => return Err(err).with_context(|| format!("cannot unpack {table}")),
        };
        let mut out = File::create(dir.join(table))?;
        io::copy(&mut entry, &mut out).with_context(|| format!("cannot unpack {table}"))?;
    }
    Ok(())
}

/// Counts bytes as they are read and, when `total` is known, shows the percentage done.
struct Progress<R> {
    inner: R,
    read: u64,
    total: Option<u64>,
    percent: Option<u64>,
}

impl<R: Read> Read for Progress<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.read += n as u64;
        if let Some(total) = self.total.filter(|&total| total > 0) {
            let percent = self.read * 100 / total;
            if self.percent != Some(percent) {
                self.percent = Some(percent);
                eprint!("\rDownloading timetable… {percent}%");
            }
        }
        Ok(n)
    }
}
