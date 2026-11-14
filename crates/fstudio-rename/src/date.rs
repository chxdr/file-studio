//! Dates for `{date}`, `{created}` and `{exif.date}`: a local date and time
//! and a small `strftime` subset. Pure: the caller works out the local time.

/// A local date and time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DateParts {
    /// The year (2026).
    pub year: i64,
    /// 1–12.
    pub month: u32,
    /// 1–31.
    pub day: u32,
    /// 0–23.
    pub hour: u32,
    /// 0–59.
    pub minute: u32,
    /// 0–60.
    pub second: u32,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// `(year, month, day)` for a day count since 1970-01-01 (proleptic Gregorian).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

fn is_leap(y: i64) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

impl DateParts {
    /// The local date and time of `unix_ns`, given the UTC offset in force then.
    pub fn from_unix_ns(unix_ns: i64, utc_offset_secs: i32) -> DateParts {
        let t = unix_ns.div_euclid(1_000_000_000) + i64::from(utc_offset_secs);
        let (y, m, d) = civil_from_days(t.div_euclid(86_400));
        let s = t.rem_euclid(86_400);
        DateParts {
            year: y,
            month: m,
            day: d,
            hour: (s / 3600) as u32,
            minute: ((s % 3600) / 60) as u32,
            second: (s % 60) as u32,
        }
    }

    /// Parses an EXIF date (`2024:06:01 14:03:22`; dashes are accepted too).
    pub fn parse_exif(s: &str) -> Option<DateParts> {
        let s = s.trim().trim_end_matches('\0');
        let b = s.as_bytes();
        if b.len() < 10 {
            return None;
        }
        let num = |r: std::ops::Range<usize>| -> Option<u32> { s.get(r)?.trim().parse().ok() };
        let p = DateParts {
            year: i64::from(num(0..4)?),
            month: num(5..7)?,
            day: num(8..10)?,
            hour: num(11..13).unwrap_or(0),
            minute: num(14..16).unwrap_or(0),
            second: num(17..19).unwrap_or(0),
        };
        // An all-zero date ("0000:00:00 00:00:00") means "not set".
        if p.year == 0 || !(1..=12).contains(&p.month) || !(1..=31).contains(&p.day) {
            return None;
        }
        Some(p)
    }

    fn day_of_year(&self) -> u32 {
        const CUM: [u32; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
        let leap = u32::from(self.month > 2 && is_leap(self.year));
        CUM[(self.month.clamp(1, 12) - 1) as usize] + self.day + leap
    }

    /// Formats with a `strftime` subset: `%Y %y %m %d %e %H %I %M %S %p %j
    /// %b %B %F %T %%`. Anything else is copied as it is.
    pub fn format(&self, fmt: &str) -> String {
        let mut out = String::with_capacity(fmt.len() + 8);
        let mut it = fmt.chars();
        while let Some(c) = it.next() {
            if c != '%' {
                out.push(c);
                continue;
            }
            let Some(spec) = it.next() else {
                out.push('%');
                break;
            };
            let mon = MONTHS[(self.month.clamp(1, 12) - 1) as usize];
            match spec {
                'Y' => out.push_str(&format!("{:04}", self.year)),
                'y' => out.push_str(&format!("{:02}", self.year.rem_euclid(100))),
                'm' => out.push_str(&format!("{:02}", self.month)),
                'd' => out.push_str(&format!("{:02}", self.day)),
                'e' => out.push_str(&self.day.to_string()),
                'H' => out.push_str(&format!("{:02}", self.hour)),
                'I' => {
                    let h = if self.hour.is_multiple_of(12) {
                        12
                    } else {
                        self.hour % 12
                    };
                    out.push_str(&format!("{h:02}"));
                }
                'M' => out.push_str(&format!("{:02}", self.minute)),
                'S' => out.push_str(&format!("{:02}", self.second)),
                'p' => out.push_str(if self.hour < 12 { "AM" } else { "PM" }),
                'j' => out.push_str(&format!("{:03}", self.day_of_year())),
                'b' => out.push_str(&mon[..3]),
                'B' => out.push_str(mon),
                'F' => out.push_str(&self.format("%Y-%m-%d")),
                'T' => out.push_str(&self.format("%H.%M.%S")),
                '%' => out.push('%'),
                other => {
                    out.push('%');
                    out.push(other);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_unix() {
        // 2024-03-01 12:34:56 UTC
        let ns = 1_709_296_496_000_000_000;
        let p = DateParts::from_unix_ns(ns, 0);
        assert_eq!(
            (p.year, p.month, p.day, p.hour, p.minute, p.second),
            (2024, 3, 1, 12, 34, 56)
        );
        let p = DateParts::from_unix_ns(ns, -13 * 3600);
        assert_eq!((p.month, p.day, p.hour), (2, 29, 23));
    }

    #[test]
    fn formats() {
        let p = DateParts {
            year: 2024,
            month: 3,
            day: 5,
            hour: 14,
            minute: 3,
            second: 9,
        };
        assert_eq!(p.format("%Y-%m-%d"), "2024-03-05");
        assert_eq!(p.format("%y%m%d_%H%M%S"), "240305_140309");
        assert_eq!(p.format("%e %b %B %j %I%p %%"), "5 Mar March 065 02PM %");
        assert_eq!(p.format("%F %T %q"), "2024-03-05 14.03.09 %q");
        assert_eq!(p.format("trailing %"), "trailing %");
    }

    #[test]
    fn exif_dates() {
        let p = DateParts::parse_exif("2023:12:31 23:59:58").unwrap();
        assert_eq!(p.format("%F_%H%M%S"), "2023-12-31_235958");
        assert!(DateParts::parse_exif("0000:00:00 00:00:00").is_none());
        assert!(DateParts::parse_exif("junk").is_none());
        assert_eq!(
            DateParts::parse_exif("2023-01-02").unwrap().format("%F"),
            "2023-01-02"
        );
    }
}
