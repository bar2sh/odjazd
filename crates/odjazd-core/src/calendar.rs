use std::collections::HashMap;

use jiff::civil::Date;

/// Which days each service (`service_id`) runs on, combined from `calendar.txt` (weekly
/// patterns) and `calendar_dates.txt` (single days added or removed).
#[derive(Debug, Default)]
pub(crate) struct Calendar {
    services: HashMap<String, Service>,
}

#[derive(Debug, Default)]
struct Service {
    weekly: Option<Weekly>,
    added: Vec<Date>,
    removed: Vec<Date>,
}

#[derive(Debug)]
struct Weekly {
    /// Monday first.
    days: [bool; 7],
    start: Date,
    end: Date,
}

impl Calendar {
    pub(crate) fn add_weekly(
        &mut self,
        service_id: String,
        days: [bool; 7],
        start: Date,
        end: Date,
    ) {
        self.services.entry(service_id).or_default().weekly = Some(Weekly { days, start, end });
    }

    pub(crate) fn add_exception(&mut self, service_id: String, date: Date, runs: bool) {
        let service = self.services.entry(service_id).or_default();
        if runs {
            service.added.push(date);
        } else {
            service.removed.push(date);
        }
    }

    /// Every service that runs on at least one of `days`, with a bit mask where bit `i` is
    /// set when it runs on `days[i]`.
    pub(crate) fn running_on(&self, days: &[Date]) -> HashMap<&str, u8> {
        self.services
            .iter()
            .filter_map(|(id, service)| {
                let mask = days
                    .iter()
                    .enumerate()
                    .filter(|&(_, &day)| service.runs_on(day))
                    .fold(0u8, |mask, (i, _)| mask | (1 << i));
                (mask != 0).then_some((id.as_str(), mask))
            })
            .collect()
    }
}

impl Service {
    fn runs_on(&self, day: Date) -> bool {
        if self.removed.contains(&day) {
            return false;
        }
        self.added.contains(&day)
            || self.weekly.as_ref().is_some_and(|weekly| {
                let weekday = day.weekday().to_monday_zero_offset() as usize;
                weekly.start <= day && day <= weekly.end && weekly.days[weekday]
            })
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::date;

    use super::*;

    #[test]
    fn exceptions_override_the_weekly_pattern() {
        let mut calendar = Calendar::default();
        // Saturdays in November 2026, except the 7th, plus one extra Sunday.
        let saturdays = [false, false, false, false, false, true, false];
        calendar.add_weekly(
            "SAT".into(),
            saturdays,
            date(2026, 11, 1),
            date(2026, 11, 30),
        );
        calendar.add_exception("SAT".into(), date(2026, 11, 7), false);
        calendar.add_exception("SAT".into(), date(2026, 11, 8), true);

        let days = [
            date(2026, 11, 7),
            date(2026, 11, 8),
            date(2026, 11, 14),
            date(2026, 12, 5),
        ];
        assert_eq!(calendar.running_on(&days)["SAT"], 0b0110);
    }
}
