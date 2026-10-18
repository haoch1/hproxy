use crate::Counter;
use chrono::{Datelike, Months, Utc};
use serde::{Deserialize, Serialize};

const DAY: i64 = 86_400_000;
const ZONE: i64 = 8 * 3_600_000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaCycle {
    pub starts_at: i64,
    pub months: u32,
    pub days: u32,
}
impl QuotaCycle {
    pub fn valid(&self) -> bool {
        (946_656_000_000..4_133_952_000_000).contains(&self.starts_at)
            && ((self.months > 0 && self.months <= 60 && self.days == 0)
                || (self.days > 0 && self.days <= 36_500 && self.months == 0))
    }
    // All calendar cycles use Beijing time and the original day, without month-end drift.
    pub fn window(&self, at: i64) -> Option<(i64, i64)> {
        if !self.valid() { return None; }
        if at < self.starts_at { return Some((0, self.starts_at)); }
        if self.days > 0 {
            let width = i64::from(self.days) * DAY;
            let start = self.starts_at + (at - self.starts_at) / width * width;
            return Some((start, start.checked_add(width)?));
        }
        let anchor = chrono::DateTime::<Utc>::from_timestamp_millis(self.starts_at + ZONE)?;
        let current = chrono::DateTime::<Utc>::from_timestamp_millis(at + ZONE)?;
        let elapsed = (current.year() - anchor.year()) * 12 + current.month() as i32 - anchor.month() as i32;
        let mut step = elapsed.max(0) as u32 / self.months;
        let boundary = |n: u32| anchor.checked_add_months(Months::new(n.checked_mul(self.months)?))
            .map(|d| d.timestamp_millis() - ZONE);
        if boundary(step)? > at { step = step.saturating_sub(1); }
        Some((boundary(step)?, boundary(step.checked_add(1)?)?))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaBaseline {
    pub cycle: QuotaCycle,
    pub start: i64,
    pub counter: Counter,
}
impl QuotaBaseline {
    pub fn refresh(&mut self, cycle: &QuotaCycle, total: &Counter, at: i64) {
        let Some((start, _)) = cycle.window(at) else { return; };
        if &self.cycle != cycle {
            // Editing the schedule alone never clears traffic already used.
            self.cycle = cycle.clone();
            self.start = start;
        } else if start > self.start {
            self.start = start;
            self.counter = total.clone();
        }
    }
    pub fn usage(&self, total: &Counter) -> Counter {
        Counter { upload: total.upload.saturating_sub(self.counter.upload),
            download: total.download.saturating_sub(self.counter.download) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn date(s: &str) -> i64 { chrono::DateTime::parse_from_rfc3339(s).unwrap().timestamp_millis() }
    #[test]
    fn calendar_cycles_clamp_month_ends_without_drift_and_preserve_leap_years() {
        let mut cycle = QuotaCycle { starts_at: date("2024-01-31T00:00:00+08:00"), months: 1, days: 0 };
        for (at, start, next) in [
            ("2024-02-28", "2024-01-31", "2024-02-29"),
            ("2024-02-29", "2024-02-29", "2024-03-31"),
            ("2024-04-01", "2024-03-31", "2024-04-30"),
            ("2026-10-17", "2026-09-30", "2026-10-31"),
        ] {
            let parse = |s| date(&format!("{s}T00:00:00+08:00"));
            assert_eq!(cycle.window(parse(at)), Some((parse(start),parse(next))));
        }
        cycle.starts_at = date("2024-02-29T00:00:00+08:00");
        cycle.months = 12;
        assert_eq!(cycle.window(date("2025-02-28T00:00:00+08:00")).unwrap().1,date("2026-02-28T00:00:00+08:00"));
        assert_eq!(cycle.window(date("2028-02-28T00:00:00+08:00")).unwrap().1,date("2028-02-29T00:00:00+08:00"));
    }
    #[test]
    fn custom_days_are_exact_and_invalid_cycles_are_rejected() {
        let mut cycle = QuotaCycle { starts_at: date("2026-03-15T00:00:00+08:00"), months: 0, days: 45 };
        assert_eq!(cycle.window(cycle.starts_at).unwrap().1,date("2026-04-29T00:00:00+08:00"));
        cycle.days = 0; assert!(!cycle.valid());
        cycle.days = 45; cycle.months = 1; assert!(!cycle.valid());
        cycle.months = 0; cycle.days = 36_501; assert!(!cycle.valid());
    }
}
