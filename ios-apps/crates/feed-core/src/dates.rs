//! The two date formats of feeds, as Unix seconds: RFC 822 (RSS) and ISO 8601 (Atom).
//! Small on purpose: no calendar crate for two formats.

/// `None` for what is neither. A date with no zone is read as UTC.
pub fn parse_date(text: &str) -> Option<i64> {
    let text = text.trim();
    rfc822(text).or_else(|| iso8601(text))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(year: i64, month: i64) -> i64 {
    match month {
        2 if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// `offset` is the zone's, in seconds east of UTC.
fn timestamp(date: (i64, i64, i64), time: (i64, i64, i64), offset: i64) -> Option<i64> {
    let (year, month, day) = date;
    let (hour, minute, second) = time;
    let valid = (1..=9999).contains(&year)
        && (1..=12).contains(&month)
        && (1..=days_in_month(year, month)).contains(&day)
        && (0..24).contains(&hour)
        && (0..60).contains(&minute)
        && (0..=60).contains(&second); // 60: a leap second
    valid.then(|| days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// All of `text` as a number of at most `max` digits.
fn digits(text: &str, max: usize) -> Option<i64> {
    let ok = !text.is_empty() && text.len() <= max && text.bytes().all(|b| b.is_ascii_digit());
    ok.then(|| text.parse().ok()).flatten()
}

/// `HH:MM` or `HH:MM:SS`.
fn clock(text: &str) -> Option<(i64, i64, i64)> {
    let mut parts = text.split(':');
    let hour = digits(parts.next()?, 2)?;
    let minute = digits(parts.next()?, 2)?;
    let second = match parts.next() {
        Some(second) => digits(second, 2)?,
        None => 0,
    };
    parts.next().is_none().then_some((hour, minute, second))
}

/// `+0200`, `-05:30`, `+02`: seconds east of UTC.
fn numeric_zone(text: &str) -> Option<i64> {
    let sign = match text.as_bytes().first()? {
        b'+' => 1,
        b'-' => -1,
        _ => return None,
    };
    let rest = text[1..].replace(':', "");
    let (hours, minutes) = match rest.len() {
        2 => (digits(&rest, 2)?, 0),
        4 => (digits(rest.get(..2)?, 2)?, digits(rest.get(2..)?, 2)?),
        _ => return None,
    };
    (hours < 24 && minutes < 60).then_some(sign * (hours * 3600 + minutes * 60))
}

/// `Mon, 01 Jan 2024 12:00:00 +0000`: the day name, the seconds and the zone are optional.
fn rfc822(text: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let mut words = text.split([' ', '\t', ',']).filter(|w| !w.is_empty()).peekable();
    if words.peek()?.bytes().all(|b| b.is_ascii_alphabetic()) {
        words.next(); // Mon
    }
    let day = digits(words.next()?, 2)?;
    let name = words.next()?.to_ascii_lowercase();
    let month = MONTHS.iter().position(|m| name.get(..3) == Some(*m))? as i64 + 1;
    let year = words.next()?;
    let year = match year.len() {
        // RFC 2822, 4.3: two digits are 20xx below 50, 19xx from there.
        2 => digits(year, 2).map(|y| if y < 50 { 2000 + y } else { 1900 + y })?,
        4 => digits(year, 4)?,
        _ => return None,
    };
    let time = clock(words.next()?)?;
    let offset = match words.next() {
        None => 0,
        Some(zone) => numeric_zone(zone).unwrap_or(match zone.to_ascii_uppercase().as_str() {
            "EDT" => -4 * 3600,
            "EST" | "CDT" => -5 * 3600,
            "CST" | "MDT" => -6 * 3600,
            "MST" | "PDT" => -7 * 3600,
            "PST" => -8 * 3600,
            _ => 0, // GMT, UT, Z, and what nobody can tell
        }),
    };
    timestamp((year, month, day), time, offset)
}

/// `2024-02-03T10:00:00Z`, `2024-02-03 10:00:00.123+01:00`, `2024-02-03T10:00`, `2024-02-03`.
fn iso8601(text: &str) -> Option<i64> {
    let date = text.get(..10)?;
    let rest = &text[10..];
    let mut parts = date.split('-');
    let date = (
        digits(parts.next()?, 4)?,
        digits(parts.next()?, 2)?,
        digits(parts.next()?, 2)?,
    );
    if parts.next().is_some() {
        return None;
    }
    if rest.is_empty() {
        return timestamp(date, (0, 0, 0), 0);
    }
    let rest = rest.strip_prefix(['T', 't', ' '])?;
    let zone_at = rest.find(['Z', 'z', '+', '-']).unwrap_or(rest.len());
    let (time, zone) = rest.split_at(zone_at);
    // Fractions of a second do not matter for sorting articles.
    let time = clock(time.split(['.', ',']).next()?)?;
    let offset = match zone {
        "" | "Z" | "z" => 0,
        zone => numeric_zone(zone)?,
    };
    timestamp(date, time, offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc822_dates() {
        // feed.py's test_rfc822
        assert_eq!(parse_date("Mon, 01 Jan 2024 12:00:00 +0000"), Some(1_704_110_400));
        assert_eq!(parse_date("Mon, 01 Jan 2024 12:00:00 GMT"), Some(1_704_110_400));
        assert_eq!(parse_date("01 Jan 2024 12:00 UT"), Some(1_704_110_400));
        assert_eq!(parse_date("Mon, 1 January 2024 14:00:00 +0200"), Some(1_704_110_400));
        assert_eq!(parse_date("Mon, 01 Jan 2024 07:00:00 EST"), Some(1_704_110_400));
        assert_eq!(parse_date("Mon, 01 Jan 2024 06:30:00 -0530"), Some(1_704_110_400));
        assert_eq!(parse_date("  Mon,  01 Jan 2024 12:00:00  "), Some(1_704_110_400));
        assert_eq!(parse_date("Mon, 01 Jan 24 12:00:00 +0000"), Some(1_704_110_400));
        assert_eq!(parse_date("Thu, 01 Jan 70 00:00:00 +0000"), Some(0));
        assert_eq!(parse_date("Wed, 31 Dec 1969 23:59:59 +0000"), Some(-1));
    }

    #[test]
    fn iso8601_dates() {
        // feed.py's test_iso8601_with_z
        assert_eq!(parse_date("2024-02-03T10:00:00Z"), Some(1_706_954_400));
        assert_eq!(parse_date("2024-02-03T11:00:00+01:00"), Some(1_706_954_400));
        assert_eq!(parse_date("2024-02-03T05:00:00.123456-0500"), Some(1_706_954_400));
        assert_eq!(parse_date("2024-02-03 10:00:00"), Some(1_706_954_400));
        assert_eq!(parse_date("2024-02-03T10:00"), Some(1_706_954_400));
        assert_eq!(parse_date("2024-02-03"), Some(1_706_918_400));
        assert_eq!(parse_date("2024-02-29T00:00:00Z"), Some(1_709_164_800));
    }

    #[test]
    fn what_is_not_a_date() {
        // feed.py's test_empty_returns_none and test_garbage_returns_none
        for text in [
            "",
            "not a date",
            "Mon, 32 Jan 2024 12:00:00 +0000",
            "Mon, 01 Foo 2024 12:00:00 +0000",
            "Mon, 01 Jan 2024 25:00:00 +0000",
            "Mon, 01 Jan 2024",
            "2023-02-29T00:00:00Z",
            "2024-13-01",
            "2024-02-03T10:00:00+25:00",
            "2024-02-03X10:00:00",
            "2024-02-03T10:00:00Zjunk",
            "2024-02-03Té",
            "１２ Jan 2024 12:00:00",
        ] {
            assert_eq!(parse_date(text), None, "{text:?}");
        }
    }
}
