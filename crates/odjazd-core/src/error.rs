use std::path::PathBuf;

/// Errors from reading a GTFS feed.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("cannot read {}", .path.display())]
    Read {
        path: PathBuf,
        #[source]
        source: csv::Error,
    },

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
}

pub(crate) fn invalid(file: &'static str, field: &'static str, value: impl AsRef<[u8]>) -> Error {
    Error::InvalidValue {
        file,
        field,
        value: String::from_utf8_lossy(value.as_ref()).into_owned(),
    }
}
