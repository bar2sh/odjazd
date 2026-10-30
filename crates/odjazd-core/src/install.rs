use std::{
    fs::{self, File},
    io,
    path::Path,
};

use crate::{Error, Feed};

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

/// Unpacks the GTFS archive (zip) at `archive` into `feed_dir`. The timetable already
/// there is replaced only once the new one is unpacked and opens fine.
pub fn install(archive: &Path, feed_dir: &Path) -> Result<(), Error> {
    let staging = feed_dir.with_extension("new");
    unpack(archive, &staging)?;
    Feed::open(&staging)?;
    if feed_dir.exists() {
        fs::remove_dir_all(feed_dir).map_err(write_error(feed_dir))?;
    }
    fs::rename(&staging, feed_dir).map_err(write_error(feed_dir))
}

fn unpack(archive: &Path, dir: &Path) -> Result<(), Error> {
    if dir.exists() {
        fs::remove_dir_all(dir).map_err(write_error(dir))?;
    }
    fs::create_dir_all(dir).map_err(write_error(dir))?;
    let file = File::open(archive).map_err(write_error(archive))?;
    let mut zip = zip::ZipArchive::new(file)?;
    for &table in TABLES {
        let mut entry = match zip.by_name(table) {
            Ok(entry) => entry,
            Err(zip::result::ZipError::FileNotFound) => continue,
            Err(err) => return Err(err.into()),
        };
        let path = dir.join(table);
        let mut out = File::create(&path).map_err(write_error(&path))?;
        io::copy(&mut entry, &mut out).map_err(write_error(&path))?;
    }
    Ok(())
}

fn write_error(path: &Path) -> impl Fn(io::Error) -> Error {
    move |source| Error::Write {
        path: path.to_owned(),
        source,
    }
}
