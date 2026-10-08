//! Which numbers are dates. A cell stores a number and its style says how to
//! show it; only the style tells a date from a quantity. This reads the number
//! format of every cell style (`cellXfs`) and turns a date serial into a date.
//!
//! Decisions.
//! - A number is a date only when its style's format is one: a built-in date or
//!   time format, or a custom code with a date or time letter outside quotes,
//!   brackets and escapes. Anything else stays a number.
//! - A date-formatted serial is a date (`1900` or `1904` system), a timestamp
//!   (a fraction, or a format with a time) or a time of day (a format with a time
//!   and no date, or a fraction on day 0). The seconds are rounded; a fraction of
//!   a second is not kept.
//! - Serials with no date stay numbers: negative, not finite, past 9999-12-31,
//!   day 0 without a time, and 60 in the 1900 system, which Excel counts as a
//!   29 February 1900 that never existed (serials 1 to 59 are shifted one day to
//!   keep the rest right).
//!
//! Bounded: at most [`MAX_XFS`] styles and as many custom formats are read; a
//! part with more is refused, so the table is never larger than 64 KiB.

use crate::tabular_prepare::xlsx_package::{attribute, next_event, Package, MAX_SMALL_PART_BYTES};
use crate::tabular_prepare::xlsx_spool::{Invalid, XlsxError};
use chrono::{Datelike, NaiveDate, Timelike};
use quick_xml::events::Event;
use std::collections::HashMap;

/// Cell styles (and custom number formats) read at most. Excel's own limit is
/// 64,000 styles.
pub const MAX_XFS: usize = 65_536;

/// What a number cell with a given style is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumFmt {
    Plain,
    Date,
    DateTime,
    Time,
}

/// The number format of every cell style, by style index.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Styles {
    xfs: Vec<NumFmt>,
}

impl Styles {
    /// A workbook with no styles part: every number is a number.
    pub fn none() -> Self {
        Self::default()
    }

    /// The format of style `index`; a style that does not exist is plain.
    pub fn format(&self, index: usize) -> NumFmt {
        self.xfs.get(index).copied().unwrap_or(NumFmt::Plain)
    }
}

/// The built-in number formats that are dates or times (ECMA-376 18.8.30);
/// 27 to 36 and 50 to 58 are the East Asian ones.
fn builtin(id: u32) -> NumFmt {
    match id {
        14..=17 | 27..=31 | 34..=36 | 50..=58 => NumFmt::Date,
        22 => NumFmt::DateTime,
        18..=21 | 32 | 33 | 45..=47 => NumFmt::Time,
        _ => NumFmt::Plain,
    }
}

/// A custom format code: a date or time letter (`y d h s`, `m` as a month or
/// minute) in its first section, outside `"quotes"`, `[brackets]` (a colour, a
/// locale, a condition; `[h]` `[m]` `[s]` are elapsed time) and after a
/// backslash or an underscore.
pub fn classify(code: &str) -> NumFmt {
    let (mut date, mut time, mut month) = (false, false, false);
    let mut chars = code.chars();
    while let Some(c) = chars.next() {
        match c {
            ';' => break,
            '"' => {
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                }
            }
            '\\' | '_' | '*' => {
                chars.next();
            }
            '[' => {
                let inner: String = chars.by_ref().take_while(|&b| b != ']').collect();
                // Only the elapsed-time sections count; a colour, a locale or a
                // condition (`[Magenta]`, `[$-409]`, `[>100]`) does not.
                if matches!(
                    inner.to_ascii_lowercase().as_str(),
                    "h" | "hh" | "m" | "mm" | "s" | "ss"
                ) {
                    time = true;
                }
            }
            'y' | 'Y' | 'd' | 'D' => date = true,
            'h' | 'H' | 's' | 'S' => time = true,
            'm' | 'M' => month = true,
            _ => {}
        }
    }
    // `m` beside an hour or a second is a minute; alone it is a month.
    date |= month && !time;
    match (date, time) {
        (true, true) => NumFmt::DateTime,
        (true, false) => NumFmt::Date,
        (false, true) => NumFmt::Time,
        (false, false) => NumFmt::Plain,
    }
}

/// Reads the number format of every `cellXfs` style of the part `part`.
pub fn read_styles(pkg: &mut Package, part: &str) -> Result<Styles, XlsxError> {
    let mut custom: HashMap<u32, NumFmt> = HashMap::new();
    let mut xfs: Vec<NumFmt> = Vec::new();
    let mut in_cell_xfs = false;
    let mut reader = pkg.xml(part, MAX_SMALL_PART_BYTES)?;
    let mut buf = Vec::new();
    loop {
        match next_event(&mut reader, &mut buf)? {
            Event::Eof => break,
            Event::Start(e) if e.local_name().as_ref() == b"cellXfs" => in_cell_xfs = true,
            Event::End(e) if e.local_name().as_ref() == b"cellXfs" => in_cell_xfs = false,
            Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                b"numFmt" => {
                    let id = attribute(&e, b"numFmtId")?.and_then(|v| v.parse().ok());
                    if let (Some(id), Some(code)) = (id, attribute(&e, b"formatCode")?) {
                        if custom.len() == MAX_XFS {
                            return Err(XlsxError::Invalid(Invalid::TooManyStyles));
                        }
                        custom.insert(id, classify(&code));
                    }
                }
                b"xf" if in_cell_xfs => {
                    if xfs.len() == MAX_XFS {
                        return Err(XlsxError::Invalid(Invalid::TooManyStyles));
                    }
                    let id: u32 = attribute(&e, b"numFmtId")?
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(0);
                    xfs.push(custom.get(&id).copied().unwrap_or_else(|| builtin(id)));
                }
                _ => {}
            },
            _ => {}
        }
    }
    Ok(Styles { xfs })
}

/// A date-formatted number, as a value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Temporal {
    /// Days since 1970-01-01.
    Date(i32),
    /// Microseconds since 1970-01-01 00:00:00, no zone.
    Timestamp(i64),
    /// Seconds since midnight.
    Time(u32),
}

impl Temporal {
    /// `YYYY-MM-DD`, `YYYY-MM-DD HH:MM:SS` or `HH:MM:SS`.
    pub fn to_text(self) -> String {
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1).expect("a valid date");
        match self {
            Self::Date(days) => {
                let d = epoch + chrono::Duration::days(i64::from(days));
                format!("{:04}-{:02}-{:02}", d.year(), d.month(), d.day())
            }
            Self::Timestamp(micros) => {
                let t = chrono::DateTime::from_timestamp_micros(micros)
                    .expect("a timestamp this reader made")
                    .naive_utc();
                format!(
                    "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
                    t.year(),
                    t.month(),
                    t.day(),
                    t.hour(),
                    t.minute(),
                    t.second()
                )
            }
            Self::Time(secs) => {
                format!("{:02}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
            }
        }
    }
}

/// The date of a day serial, or `None` where there is none (see the module
/// documentation).
fn date_of(days: i64, date1904: bool) -> Option<NaiveDate> {
    let base = if date1904 {
        NaiveDate::from_ymd_opt(1904, 1, 1)?
    } else {
        match days {
            0 | 60 => return None,
            1..=59 => NaiveDate::from_ymd_opt(1899, 12, 31)?,
            _ => NaiveDate::from_ymd_opt(1899, 12, 30)?,
        }
    };
    let date = base.checked_add_signed(chrono::Duration::days(days))?;
    (date.year() <= 9999).then_some(date)
}

/// A number with a date or time format, as a date, a timestamp or a time; `None`
/// when it is not one (then it stays a number).
pub fn temporal(serial: f64, date1904: bool, format: NumFmt) -> Option<Temporal> {
    if format == NumFmt::Plain || !serial.is_finite() || !(0.0..2_958_466.0).contains(&serial) {
        return None;
    }
    let days = serial.floor() as i64;
    // A value within half a second of midnight shows 23:59:59 of its own day:
    // rolling over would land on the next day, and in the 1900 system possibly on
    // the 29 February 1900 that never existed.
    let secs = (((serial - serial.floor()) * 86_400.0).round() as u32).min(86_399);
    if format == NumFmt::Time {
        return (days == 0).then_some(Temporal::Time(secs));
    }
    let Some(date) = date_of(days, date1904) else {
        // Day 0 with a time of day is a time; day 60 and the rest are numbers.
        return (days == 0 && secs > 0 && !date1904).then_some(Temporal::Time(secs));
    };
    if secs == 0 && format == NumFmt::Date {
        let epoch = NaiveDate::from_ymd_opt(1970, 1, 1)?;
        return i32::try_from(date.signed_duration_since(epoch).num_days())
            .ok()
            .map(Temporal::Date);
    }
    let at = date.and_hms_opt(secs / 3600, secs / 60 % 60, secs % 60)?;
    Some(Temporal::Timestamp(at.and_utc().timestamp_micros()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::xlsx_package::XlsxLimits;
    use crate::tabular_prepare::xlsx_spool::spool_stream;
    use crate::tabular_prepare::xlsxfix::Wb;
    use crate::tabular_prepare::zipfix::{build, Entry};
    use bytes::Bytes;
    use std::sync::atomic::AtomicU64;
    use tokio_util::sync::CancellationToken;

    #[test]
    fn built_in_formats_are_told_dates_from_numbers() {
        for id in [14, 15, 16, 17, 27, 31, 34, 36, 50, 58] {
            assert_eq!(builtin(id), NumFmt::Date, "{id}");
        }
        assert_eq!(builtin(22), NumFmt::DateTime);
        for id in [18, 19, 20, 21, 32, 33, 45, 46, 47] {
            assert_eq!(builtin(id), NumFmt::Time, "{id}");
        }
        for id in [0, 1, 2, 3, 4, 9, 10, 11, 12, 13, 37, 44, 48, 49, 59, 163] {
            assert_eq!(builtin(id), NumFmt::Plain, "{id}");
        }
    }

    #[test]
    fn custom_codes_are_dates_only_by_their_letters_outside_quotes_and_brackets() {
        let cases = [
            ("yyyy-mm-dd", NumFmt::Date),
            ("d/m/yyyy;@", NumFmt::Date),
            ("[$-409]d-mmm-yy", NumFmt::Date),
            ("mmm", NumFmt::Date),
            ("yyyy-mm-dd hh:mm:ss", NumFmt::DateTime),
            ("m/d/yy h:mm", NumFmt::DateTime),
            ("hh:mm", NumFmt::Time),
            ("mm:ss", NumFmt::Time),
            ("[h]:mm:ss", NumFmt::Time),
            ("General", NumFmt::Plain),
            ("0.00", NumFmt::Plain),
            ("#,##0", NumFmt::Plain),
            ("0.0%", NumFmt::Plain),
            ("0.00E+00", NumFmt::Plain),
            ("@", NumFmt::Plain),
            ("[Red]0.0", NumFmt::Plain),
            ("[Magenta]0.00", NumFmt::Plain),
            ("[Green]0", NumFmt::Plain),
            ("[$-409]0.00", NumFmt::Plain),
            ("[>100]0", NumFmt::Plain),
            ("[Red]yyyy-mm-dd", NumFmt::Date),
            ("[HH]:MM:SS", NumFmt::Time),
            ("[mm]:ss", NumFmt::Time),
            ("[hhh]0", NumFmt::Plain),
            ("0 \"days\"", NumFmt::Plain),
            ("\\d0", NumFmt::Plain),
            ("0_)", NumFmt::Plain),
            ("0.00;yyyy", NumFmt::Plain),
        ];
        for (code, want) in cases {
            assert_eq!(classify(code), want, "{code}");
        }
    }

    fn text(serial: f64, date1904: bool, format: NumFmt) -> Option<String> {
        temporal(serial, date1904, format).map(Temporal::to_text)
    }

    #[test]
    fn the_1900_system_keeps_its_leap_year_quirk() {
        let d = |n| text(n, false, NumFmt::Date);
        assert_eq!(d(1.0).as_deref(), Some("1900-01-01"));
        assert_eq!(d(59.0).as_deref(), Some("1900-02-28"));
        assert_eq!(d(60.0), None, "the 29 February 1900 that never existed");
        assert_eq!(d(61.0).as_deref(), Some("1900-03-01"));
        assert_eq!(d(25_569.0).as_deref(), Some("1970-01-01"));
        assert_eq!(d(44_197.0).as_deref(), Some("2021-01-01"));
        assert_eq!(d(2_958_465.0).as_deref(), Some("9999-12-31"));
        assert_eq!(
            temporal(25_569.0, false, NumFmt::Date),
            Some(Temporal::Date(0))
        );
    }

    #[test]
    fn the_1904_system_is_offset_from_the_1900_one_by_1462_days() {
        assert_eq!(text(0.0, true, NumFmt::Date).as_deref(), Some("1904-01-01"));
        assert_eq!(
            text(42_735.0, true, NumFmt::Date),
            text(44_197.0, false, NumFmt::Date)
        );
        assert_eq!(
            text(1461.0, true, NumFmt::Date).as_deref(),
            Some("1908-01-01")
        );
    }

    #[test]
    fn times_fractions_and_roundings_keep_the_value_that_was_stored() {
        let t = |n, f| text(n, false, f);
        assert_eq!(
            t(44_197.5, NumFmt::DateTime).as_deref(),
            Some("2021-01-01 12:00:00")
        );
        // A time format shows midnight too; a date format does not invent one.
        assert_eq!(
            t(44_197.0, NumFmt::DateTime).as_deref(),
            Some("2021-01-01 00:00:00")
        );
        assert_eq!(t(44_197.0, NumFmt::Date).as_deref(), Some("2021-01-01"));
        // A date format over a serial with a time keeps the time.
        assert_eq!(
            t(44_197.25, NumFmt::Date).as_deref(),
            Some("2021-01-01 06:00:00")
        );
        // Seconds are rounded; 23:59:59.9 stays on its own day.
        assert_eq!(
            t(44_197.999_999_9, NumFmt::DateTime).as_deref(),
            Some("2021-01-01 23:59:59")
        );
        // Day 59 of the 1900 system rolling over would be the day that never existed.
        assert_eq!(
            text(59.999_999, false, NumFmt::Date).as_deref(),
            Some("1900-02-28 23:59:59")
        );
        assert_eq!(
            text(0.999_999_9, false, NumFmt::Time).as_deref(),
            Some("23:59:59")
        );
        assert_eq!(t(0.5, NumFmt::Time).as_deref(), Some("12:00:00"));
        assert_eq!(
            t(0.25, NumFmt::Date).as_deref(),
            Some("06:00:00"),
            "day 0 with a time"
        );
        assert_eq!(
            t(1.5, NumFmt::Time),
            None,
            "elapsed time past a day stays a number"
        );
        assert_eq!(t(0.0, NumFmt::Date), None);
    }

    #[test]
    fn what_is_no_date_stays_a_number() {
        for bad in [-1.0, f64::NAN, f64::INFINITY, 2_958_466.0, 1e300] {
            assert_eq!(text(bad, false, NumFmt::Date), None, "{bad}");
        }
        assert_eq!(text(44_197.0, false, NumFmt::Plain), None);
        // 1904 system, past 9999-12-31.
        assert_eq!(text(2_957_004.0, true, NumFmt::Date), None);
    }

    async fn styles_of(bytes: Vec<u8>) -> Result<Styles, XlsxError> {
        let dir = tempfile::tempdir().unwrap();
        let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
        let read = AtomicU64::new(0);
        let cancel = CancellationToken::new();
        let spooled = spool_stream(dir.path(), stream, None, 1 << 30, &cancel, &read)
            .await
            .unwrap();
        let mut pkg = Package::open_with(spooled, &XlsxLimits::default()).unwrap();
        read_styles(&mut pkg, "xl/styles.xml")
    }

    #[tokio::test]
    async fn every_cell_style_gets_the_format_of_its_number_format_id() {
        let bytes = Wb::new()
            .styles(
                &[0, 14, 22, 164, 165, 9],
                &[(164, "yyyy-mm-dd"), (165, "0.00")],
            )
            .build();
        let styles = styles_of(bytes).await.unwrap();
        let formats: Vec<_> = (0..7).map(|i| styles.format(i)).collect();
        assert_eq!(
            formats,
            [
                NumFmt::Plain,
                NumFmt::Date,
                NumFmt::DateTime,
                NumFmt::Date,
                NumFmt::Plain,
                NumFmt::Plain,
                NumFmt::Plain, // a style that does not exist
            ]
        );
        assert_eq!(Styles::none().format(0), NumFmt::Plain);
    }

    #[tokio::test]
    async fn only_cell_xfs_count_and_the_number_of_styles_is_bounded() {
        let xml = "<styleSheet><cellStyleXfs><xf numFmtId=\"14\"/></cellStyleXfs><cellXfs><xf numFmtId=\"0\"><alignment/></xf><xf numFmtId=\"22\"/></cellXfs></styleSheet>";
        let bytes = build(&[Entry::stored("xl/styles.xml", xml.as_bytes())]);
        let styles = styles_of(bytes).await.unwrap();
        assert_eq!(
            (styles.format(0), styles.format(1)),
            (NumFmt::Plain, NumFmt::DateTime)
        );
        // One style more than the limit.
        let many = "<xf numFmtId=\"14\"/>".repeat(MAX_XFS + 1);
        let xml = format!("<styleSheet><cellXfs>{many}</cellXfs></styleSheet>");
        let bytes = build(&[Entry::stored("xl/styles.xml", xml.as_bytes())]);
        assert_eq!(
            styles_of(bytes).await,
            Err(XlsxError::Invalid(Invalid::TooManyStyles))
        );
    }
}
