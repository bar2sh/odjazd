use std::fmt;

use jiff::{SignedDuration, Timestamp, civil::Date, tz::TimeZone};

/// A time of day as written in a GTFS feed (`HH:MM:SS`).
///
/// GTFS counts time from "noon minus 12 hours" of the service day, so a night trip that
/// belongs to Friday's timetable but leaves after midnight is written as `24:05:00` or
/// `25:10:00`. [`GtfsTime`] keeps that number of seconds as is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GtfsTime(u32);

impl GtfsTime {
    pub const fn from_secs(secs: u32) -> Self {
        Self(secs)
    }

    pub const fn secs(self) -> u32 {
        self.0
    }

    /// Parses `H:MM:SS` or `HH:MM:SS`. Hours may go past 23.
    pub fn parse(s: &[u8]) -> Option<Self> {
        let mut parts = s.split(|&b| b == b':');
        let (h, m, sec) = (parts.next()?, parts.next()?, parts.next()?);
        if parts.next().is_some() || h.is_empty() || h.len() > 3 || m.len() != 2 || sec.len() != 2 {
            return None;
        }
        let (h, m, sec) = (number(h)?, number(m)?, number(sec)?);
        (m < 60 && sec < 60).then_some(Self(h * 3600 + m * 60 + sec))
    }

    /// The moment this time happens on the service day starting at `origin`.
    pub(crate) fn after(self, origin: Timestamp) -> Result<Timestamp, jiff::Error> {
        origin.checked_add(SignedDuration::from_secs(i64::from(self.0)))
    }
}

impl fmt::Display for GtfsTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let s = self.0;
        write!(f, "{:02}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }
}

/// The instant GTFS times of `date` are counted from: noon minus 12 hours. That is midnight,
/// except on the two days a year when the clocks change.
pub(crate) fn service_day_origin(date: Date, tz: &TimeZone) -> Result<Timestamp, jiff::Error> {
    let noon = date.at(12, 0, 0, 0).to_zoned(tz.clone())?;
    noon.timestamp().checked_sub(SignedDuration::from_hours(12))
}

/// Parses a non-empty string of ASCII digits.
pub(crate) fn number(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u32, |n, &b| {
        let digit = b.is_ascii_digit().then(|| u32::from(b - b'0'))?;
        n.checked_mul(10)?.checked_add(digit)
    })
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    #[test]
    fn parses_times_past_midnight() {
        assert_eq!(
            GtfsTime::parse(b"25:10:00"),
            Some(GtfsTime::from_secs(25 * 3600 + 600))
        );
        assert_eq!(
            GtfsTime::parse(b"7:05:09"),
            Some(GtfsTime::from_secs(7 * 3600 + 309))
        );
        assert_eq!(GtfsTime::from_secs(25 * 3600 + 600).to_string(), "25:10:00");
    }

    #[test]
    fn rejects_malformed_times() {
        for bad in [
            "",
            "12:00",
            "12:60:00",
            "12:00:61",
            "1a:00:00",
            "12:00:00:00",
            "1234:00:00",
        ] {
            assert_eq!(GtfsTime::parse(bad.as_bytes()), None, "{bad}");
        }
    }

    #[test]
    fn service_day_starts_at_noon_minus_twelve_hours() {
        let warsaw = TimeZone::get("Europe/Warsaw").unwrap();
        let wall_clock = |origin: Timestamp, time: &str| {
            let time = GtfsTime::parse(time.as_bytes()).unwrap();
            time.after(origin)
                .unwrap()
                .to_zoned(warsaw.clone())
                .datetime()
        };

        // On an ordinary day that is midnight.
        let origin = service_day_origin(date(2026, 10, 30), &warsaw).unwrap();
        assert_eq!(
            wall_clock(origin, "08:00:00"),
            date(2026, 10, 30).at(8, 0, 0, 0)
        );
        assert_eq!(
            wall_clock(origin, "24:05:00"),
            date(2026, 10, 31).at(0, 5, 0, 0)
        );

        // 25 October 2026 has 25 hours (clocks go back at 03:00), so counting starts at
        // 01:00 and `08:00:00` still means 08:00 on the wall clock.
        let origin = service_day_origin(date(2026, 10, 25), &warsaw).unwrap();
        assert_eq!(
            wall_clock(origin, "08:00:00"),
            date(2026, 10, 25).at(8, 0, 0, 0)
        );
    }
}
