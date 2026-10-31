//! A [`Timetable`] in a compact binary form ([rkyv]), which loads in milliseconds instead
//! of the second or two it takes to build one from the GTFS feed: from a cache on disk, or
//! in a browser that downloaded it.
//!
//! [rkyv]: https://rkyv.org

use std::{
    fs, io,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use rkyv::{
    Archive, Archived, Place, Resolver, Serialize,
    rancor::{Fallible, Source},
    ser::Writer,
    string::{ArchivedString, StringResolver},
    util::AlignedVec,
    with::{ArchiveWith, DeserializeWith, SerializeWith},
};

use crate::{Error, Feed, Timetable};

/// The first bytes of [`Timetable::to_bytes`].
const MAGIC: [u8; 8] = *b"odjazd\0\0";

/// Changes whenever the layout of [`Timetable`] does, so that older files are rebuilt
/// instead of misread.
const FORMAT: u32 = 1;

/// Magic and format, padded so that the archive after them stays aligned.
const HEADER_LEN: usize = 16;

/// rkyv reads values in place, so the archive must start at this alignment.
const ALIGN: usize = 16;

impl Timetable {
    /// The timetable in the binary form [`Timetable::from_bytes`] reads: 22 MB for three
    /// days of Warsaw.
    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let archive = rkyv::to_bytes::<rkyv::rancor::Error>(self).map_err(Error::Binary)?;
        let mut bytes = Vec::with_capacity(HEADER_LEN + archive.len());
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&FORMAT.to_le_bytes());
        bytes.resize(HEADER_LEN, 0);
        bytes.extend_from_slice(&archive);
        Ok(bytes)
    }

    /// Reads a timetable written by [`Timetable::to_bytes`] of this version of odjazd. The
    /// bytes are checked first, so a damaged file is an error rather than a crash.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, Error> {
        let archive = bytes
            .split_at_checked(HEADER_LEN)
            .filter(|(header, _)| header[..8] == MAGIC && header[8..12] == FORMAT.to_le_bytes())
            .ok_or(Error::BinaryFormat)?
            .1;
        let timetable = if archive.as_ptr().addr() % ALIGN == 0 {
            rkyv::from_bytes::<Self, rkyv::rancor::Error>(archive)
        } else {
            let mut aligned = AlignedVec::<ALIGN>::with_capacity(archive.len());
            aligned.extend_from_slice(archive);
            rkyv::from_bytes::<Self, rkyv::rancor::Error>(&aligned)
        };
        timetable.map_err(Error::Binary)
    }

    /// The timetable for `today` from the feed unpacked in `feed_dir`. It is read from
    /// `cache_dir` if it was saved there for the same day from the same download, and
    /// otherwise built from the feed and saved there for next time.
    ///
    /// `cache_dir` belongs to odjazd: saving removes the files it saved earlier. A cache
    /// that cannot be read or saved only costs time, so that is not an error.
    pub fn load_or_build(feed_dir: &Path, cache_dir: &Path, today: Date) -> Result<Self, Error> {
        let path = cache_path(feed_dir, cache_dir, today).ok();
        if let Some(path) = &path
            && let Ok(bytes) = read_aligned(path)
            && let Ok(timetable) = Self::from_bytes(&bytes)
            && timetable.days()[1] == today
        {
            return Ok(timetable);
        }
        let timetable = Self::build(&Feed::open(feed_dir)?, today)?;
        if let Some(path) = &path {
            let _ = save(&timetable, path);
        }
        Ok(timetable)
    }
}

/// Saved files start with this.
const CACHE_PREFIX: &str = "timetable-";

/// `timetable-<day>-<key>.bin`, where `<key>` hashes the names, sizes and times of the
/// feed's files and the version of odjazd: a new download or a new odjazd names a new
/// cache.
fn cache_path(feed_dir: &Path, cache_dir: &Path, today: Date) -> io::Result<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(feed_dir)? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        let modified = metadata
            .modified()?
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        files.push((entry.file_name(), metadata.len(), modified.as_nanos()));
    }
    files.sort();
    let mut hash = Fnv::default();
    hash.write(env!("CARGO_PKG_VERSION").as_bytes());
    hash.write(&FORMAT.to_le_bytes());
    for (name, len, modified) in files {
        hash.write(name.as_encoded_bytes());
        hash.write(&len.to_le_bytes());
        hash.write(&modified.to_le_bytes());
    }
    let day = today.strftime("%Y%m%d");
    Ok(cache_dir.join(format!("{CACHE_PREFIX}{day}-{:016x}.bin", hash.0)))
}

/// Reads a whole file into memory aligned for rkyv.
fn read_aligned(path: &Path) -> io::Result<AlignedVec<ALIGN>> {
    let mut file = fs::File::open(path)?;
    let mut bytes = AlignedVec::with_capacity(file.metadata()?.len() as usize);
    bytes.extend_from_reader(&mut file)?;
    Ok(bytes)
}

/// Saves `timetable` as `path`, and removes the files saved for other days and downloads.
fn save(timetable: &Timetable, path: &Path) -> Result<(), Error> {
    let bytes = timetable.to_bytes()?;
    let dir = path.parent().expect("a cache file is in a directory");
    let write_error = |source| Error::Write {
        path: path.to_owned(),
        source,
    };
    fs::create_dir_all(dir).map_err(write_error)?;
    // Written beside its final place, then renamed: an odjazd started meanwhile never
    // reads half a file.
    let partial = path.with_extension("partial");
    fs::write(&partial, bytes).map_err(write_error)?;
    fs::rename(&partial, path).map_err(write_error)?;
    for entry in fs::read_dir(dir).map_err(write_error)?.flatten() {
        let other = entry.path();
        if other != path
            && entry
                .file_name()
                .to_string_lossy()
                .starts_with(CACHE_PREFIX)
        {
            let _ = fs::remove_file(other);
        }
    }
    Ok(())
}

/// FNV-1a: tiny, and unlike the standard library's hasher, the same in every Rust release.
struct Fnv(u64);

impl Default for Fnv {
    fn default() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3);
        }
    }
}

// jiff's types have no rkyv support, so `Timetable` stores them through these wrappers.

/// Stores a [`TimeZone`] by its IANA name, e.g. `Europe/Warsaw`.
pub(crate) struct IanaName;

#[derive(Debug, thiserror::Error)]
#[error("the time zone has no IANA name")]
struct Unnamed;

impl ArchiveWith<TimeZone> for IanaName {
    type Archived = ArchivedString;
    type Resolver = StringResolver;

    fn resolve_with(field: &TimeZone, resolver: Self::Resolver, out: Place<Self::Archived>) {
        let name = field.iana_name().unwrap_or_default();
        ArchivedString::resolve_from_str(name, resolver, out);
    }
}

impl<S> SerializeWith<TimeZone, S> for IanaName
where
    S: Fallible + Writer + ?Sized,
    S::Error: Source,
{
    fn serialize_with(field: &TimeZone, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        let name = field.iana_name().ok_or_else(|| S::Error::new(Unnamed))?;
        ArchivedString::serialize_from_str(name, serializer)
    }
}

impl<D> DeserializeWith<ArchivedString, TimeZone, D> for IanaName
where
    D: Fallible + ?Sized,
    D::Error: Source,
{
    fn deserialize_with(field: &ArchivedString, _: &mut D) -> Result<TimeZone, D::Error> {
        TimeZone::get(field.as_str()).map_err(D::Error::new)
    }
}

/// Stores a [`Timestamp`] as nanoseconds since 1970.
pub(crate) struct Nanoseconds;

impl ArchiveWith<Timestamp> for Nanoseconds {
    type Archived = Archived<i128>;
    type Resolver = Resolver<i128>;

    fn resolve_with(field: &Timestamp, resolver: Self::Resolver, out: Place<Self::Archived>) {
        field.as_nanosecond().resolve(resolver, out);
    }
}

impl<S: Fallible + ?Sized> SerializeWith<Timestamp, S> for Nanoseconds {
    fn serialize_with(field: &Timestamp, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        field.as_nanosecond().serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<i128>, Timestamp, D> for Nanoseconds
where
    D: Fallible + ?Sized,
    D::Error: Source,
{
    fn deserialize_with(field: &Archived<i128>, _: &mut D) -> Result<Timestamp, D::Error> {
        Timestamp::from_nanosecond(field.to_native()).map_err(D::Error::new)
    }
}

/// Stores a [`Date`] as the number `YYYYMMDD`.
pub(crate) struct Ymd;

fn ymd(date: Date) -> i32 {
    i32::from(date.year()) * 10_000 + i32::from(date.month()) * 100 + i32::from(date.day())
}

fn date<E: Source>(ymd: i32) -> Result<Date, E> {
    let year = i16::try_from(ymd / 10_000).unwrap_or(i16::MIN);
    let part = |n: i32| i8::try_from(n).unwrap_or(i8::MIN);
    Date::new(year, part(ymd / 100 % 100), part(ymd % 100)).map_err(E::new)
}

impl ArchiveWith<Date> for Ymd {
    type Archived = Archived<i32>;
    type Resolver = Resolver<i32>;

    fn resolve_with(field: &Date, resolver: Self::Resolver, out: Place<Self::Archived>) {
        ymd(*field).resolve(resolver, out);
    }
}

impl<S: Fallible + ?Sized> SerializeWith<Date, S> for Ymd {
    fn serialize_with(field: &Date, serializer: &mut S) -> Result<Self::Resolver, S::Error> {
        ymd(*field).serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<i32>, Date, D> for Ymd
where
    D: Fallible + ?Sized,
    D::Error: Source,
{
    fn deserialize_with(field: &Archived<i32>, _: &mut D) -> Result<Date, D::Error> {
        date(field.to_native())
    }
}

/// Stores a `(service day, trip)` pair of the trip index, the day as [`Ymd`] does.
pub(crate) struct DayTrip;

impl ArchiveWith<(Date, usize)> for DayTrip {
    type Archived = Archived<(i32, usize)>;
    type Resolver = Resolver<(i32, usize)>;

    fn resolve_with(
        &(day, trip): &(Date, usize),
        resolver: Self::Resolver,
        out: Place<Self::Archived>,
    ) {
        (ymd(day), trip).resolve(resolver, out);
    }
}

impl<S: Fallible + ?Sized> SerializeWith<(Date, usize), S> for DayTrip {
    fn serialize_with(
        &(day, trip): &(Date, usize),
        serializer: &mut S,
    ) -> Result<Self::Resolver, S::Error> {
        (ymd(day), trip).serialize(serializer)
    }
}

impl<D> DeserializeWith<Archived<(i32, usize)>, (Date, usize), D> for DayTrip
where
    D: Fallible + ?Sized,
    D::Error: Source,
{
    fn deserialize_with(
        field: &Archived<(i32, usize)>,
        _: &mut D,
    ) -> Result<(Date, usize), D::Error> {
        Ok((date(field.0.to_native())?, field.1.to_native() as usize))
    }
}
