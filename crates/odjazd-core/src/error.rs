use std::{io, path::PathBuf};

/// Errors from reading a GTFS or GTFS-Realtime feed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: csv::Error,
    },

    #[error("cannot write {}", .path.display())]
    Write {
        path: PathBuf,
        #[source]
        source: io::Error,
    },

    #[error("not a GTFS zip archive")]
    Archive(#[from] zip::result::ZipError),

    #[error("{file} has no `{column}` column")]
    MissingColumn {
        file: &'static str,
        column: &'static str,
    },

    #[error("{file}: invalid {field} `{value}`")]
    InvalidValue {
        file: &'static str,
        field: &'static str,
        value: String,
    },

    #[error("unknown time zone `{name}`")]
    TimeZone {
        name: String,
        #[source]
        source: jiff::Error,
    },

    #[error("date out of the supported range")]
    DateRange(#[from] jiff::Error),

    #[error("invalid GTFS-Realtime data")]
    Realtime(#[from] prost::DecodeError),

    #[cfg(feature = "cache")]
    #[error("not a timetable saved by this version of odjazd")]
    BinaryFormat,

    #[cfg(feature = "cache")]
    #[error("cannot convert the timetable to or from its binary form")]
    Binary(#[source] rkyv::rancor::Error),
}

pub(crate) fn invalid(file: &'static str, field: &'static str, value: impl AsRef<[u8]>) -> Error {
    Error::InvalidValue {
        file,
        field,
        value: String::from_utf8_lossy(value.as_ref()).into_owned(),
    }
}
