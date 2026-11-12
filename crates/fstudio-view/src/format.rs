//! Display strings for sizes and dates. Pure: the caller supplies the clock and the time zone.

/// How sizes are written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SizeFormat {
    /// The decimal separator (`.` or `,`).
    pub decimal_separator: char,
    /// The word for one byte.
    pub byte: String,
    /// The word for several bytes.
    pub bytes: String,
    /// What zero bytes shows as.
    pub zero: String,
}

impl Default for SizeFormat {
    fn default() -> Self {
        SizeFormat {
            decimal_separator: '.',
            byte: "byte".into(),
            bytes: "bytes".into(),
            zero: "Zero KB".into(),
        }
    }
}

/// Formats a byte count the way Finder does: decimal units (1 KB = 1000 bytes), whole KB, one
/// decimal for MB and up, and `bytes` below 1 KB. `1_234_567` → `"1.2 MB"`.
pub fn format_size(bytes: u64, f: &SizeFormat) -> String {
    if bytes == 0 {
        return f.zero.clone();
    }
    if bytes < 1000 {
        return format!("{bytes} {}", if bytes == 1 { &f.byte } else { &f.bytes });
    }
    const UNITS: [&str; 6] = ["KB", "MB", "GB", "TB", "PB", "EB"];
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        return format!("{} KB", value.round() as u64);
    }
    let tenths = (value * 10.0).round() as u64;
    if tenths.is_multiple_of(10) {
        format!("{} {}", tenths / 10, UNITS[unit])
    } else {
        format!(
            "{}{}{} {}",
            tenths / 10,
            f.decimal_separator,
            tenths % 10,
            UNITS[unit]
        )
    }
}

/// Day-month-year order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateOrder {
    /// 3 Mar 2025
    DayMonthYear,
    /// Mar 3, 2025
    MonthDayYear,
    /// 2025-03-03 style, with the month name
    YearMonthDay,
}

/// How dates are written.
#[derive(Debug, Clone)]
pub struct DateFormat {
    /// Word for the current day.
    pub today: String,
    /// Word for the previous day.
    pub yesterday: String,
    /// Joins the day word and the time: `"Today at 14:32"`.
    pub at: String,
    /// Short month names, January first.
    pub months: [String; 12],
    /// The order of day, month and year for older dates.
    pub order: DateOrder,
    /// 24-hour clock when true, otherwise 12-hour with `am`/`pm`.
    pub hour24: bool,
    /// Text for the morning in 12-hour mode.
    pub am: String,
    /// Text for the afternoon in 12-hour mode.
    pub pm: String,
}

impl Default for DateFormat {
    fn default() -> Self {
        let m = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ];
        DateFormat {
            today: "Today".into(),
            yesterday: "Yesterday".into(),
            at: "at".into(),
            months: m.map(String::from),
            order: DateOrder::DayMonthYear,
            hour24: true,
            am: "AM".into(),
            pm: "PM".into(),
        }
    }
}

/// The reference point for "today": the current time and the zone to show it in.
#[derive(Debug, Clone, Copy)]
pub struct Clock {
    /// Now, in ns since the Unix epoch.
    pub now_ns: i64,
    /// The zone's offset from UTC in seconds, for the instant being formatted. A shell that cares about
    /// daylight-saving changes can build a `Clock` per call.
    pub utc_offset_secs: i32,
}

/// `(year, month 1-12, day 1-31)` for a day count since 1970-01-01 (proleptic Gregorian).
pub fn civil_from_days(z: i64) -> (i64, u32, u32) {
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

/// Formats a modification time: `"Today at 14:32"`, `"Yesterday at 09:05"`, otherwise a short
/// date such as `"3 Mar 2025"`.
pub fn format_date(time_ns: i64, clock: Clock, f: &DateFormat) -> String {
    let local = |ns: i64| ns.div_euclid(1_000_000_000) + i64::from(clock.utc_offset_secs);
    let (t, n) = (local(time_ns), local(clock.now_ns));
    let (day, day_now) = (t.div_euclid(86_400), n.div_euclid(86_400));
    let secs = t.rem_euclid(86_400);
    let (h, min) = ((secs / 3600) as u32, ((secs % 3600) / 60) as u32);
    let time = if f.hour24 {
        format!("{h:02}:{min:02}")
    } else {
        let h12 = if h % 12 == 0 { 12 } else { h % 12 };
        format!("{h12}:{min:02} {}", if h < 12 { &f.am } else { &f.pm })
    };
    let word = if day == day_now {
        Some(&f.today)
    } else if day == day_now - 1 {
        Some(&f.yesterday)
    } else {
        None
    };
    if let Some(w) = word {
        return format!("{w} {} {time}", f.at);
    }
    let (y, m, d) = civil_from_days(day);
    let mon = &f.months[(m - 1) as usize];
    match f.order {
        DateOrder::DayMonthYear => format!("{d} {mon} {y}"),
        DateOrder::MonthDayYear => format!("{mon} {d}, {y}"),
        DateOrder::YearMonthDay => format!("{y} {mon} {d}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        let f = SizeFormat::default();
        assert_eq!(format_size(0, &f), "Zero KB");
        assert_eq!(format_size(1, &f), "1 byte");
        assert_eq!(format_size(532, &f), "532 bytes");
        assert_eq!(format_size(1000, &f), "1 KB");
        assert_eq!(format_size(12_345, &f), "12 KB");
        assert_eq!(format_size(999_999, &f), "1 MB");
        assert_eq!(format_size(1_234_567, &f), "1.2 MB");
        assert_eq!(format_size(5_000_000, &f), "5 MB");
        assert_eq!(format_size(52_428_800, &f), "52.4 MB");
        assert_eq!(format_size(3_400_000_000, &f), "3.4 GB");
        let de = SizeFormat {
            decimal_separator: ',',
            ..SizeFormat::default()
        };
        assert_eq!(format_size(1_234_567, &de), "1,2 MB");
    }

    #[test]
    fn civil() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_723), (2024, 1, 1));
        assert_eq!(civil_from_days(-1), (1969, 12, 31));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }

    #[test]
    fn dates() {
        let f = DateFormat::default();
        // 2025-03-03 14:32:00 UTC
        let t = (20_150i64 * 86_400 + 14 * 3600 + 32 * 60) * 1_000_000_000;
        assert_eq!(civil_from_days(20_150), (2025, 3, 3));
        let clock = |off| Clock {
            now_ns: t + 3600 * 1_000_000_000,
            utc_offset_secs: off,
        };
        assert_eq!(format_date(t, clock(0), &f), "Today at 14:32");
        let yesterday = t - 86_400 * 1_000_000_000;
        assert_eq!(format_date(yesterday, clock(0), &f), "Yesterday at 14:32");
        let old = t - 10 * 86_400 * 1_000_000_000;
        assert_eq!(format_date(old, clock(0), &f), "21 Feb 2025");
        // A zone ahead of UTC moves the same instant to the next day.
        let late = (20_150i64 * 86_400 + 23 * 3600) * 1_000_000_000;
        let c = Clock {
            now_ns: late,
            utc_offset_secs: 2 * 3600,
        };
        assert_eq!(format_date(late, c, &f), "Today at 01:00");
        let us = DateFormat {
            order: DateOrder::MonthDayYear,
            hour24: false,
            ..DateFormat::default()
        };
        assert_eq!(format_date(old, clock(0), &us), "Feb 21, 2025");
        assert_eq!(format_date(t, clock(0), &us), "Today at 2:32 PM");
    }
}
