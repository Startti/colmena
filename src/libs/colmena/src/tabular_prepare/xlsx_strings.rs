//! The shared-strings table of a workbook, held in memory under a stated cap.
//!
//! Cells point into this table by index and a sheet can use any entry at any
//! row, so it needs random access; it is the one part of a workbook that is
//! read whole. Hence the cap, with a typed refusal rather than a spill:
//!
//! - the part's declared size (the archive pre-check already knows it) must be at
//!   most [`MAX_SHARED_STRINGS_XML_BYTES`] (128 MiB) and the table at most
//!   [`MAX_SHARED_STRINGS`] (10,000,000) entries, else
//!   [`Cap::SharedStrings`], which the driver records as `xlsx_too_large`;
//! - the text goes into one buffer and one `u32` end offset per string, both
//!   reserved up front from the declared size, so the table never grows by
//!   doubling: the text is never larger than the XML it came from (decoding only
//!   shrinks it) and the offsets are at most 40 MiB. The worst case is therefore
//!   about 128 + 40 = 168 MiB, against the 4 GiB of the preparation job, and it is
//!   reached only by a workbook whose sheets are already near the 400 MiB cap.
//!
//! A real workbook near that cap can have more unique text than this holds; it is
//! refused with a message to export as CSV. Spilling the table to the local file
//! is the known way to lift the cap and is not built.
//!
//! A string is the concatenation of the text runs of its `si` element (plain
//! `t`, or the `t` of each rich-text run `r`); phonetic runs (`rPh`) are not
//! part of the value.

use crate::tabular_prepare::xlsx_package::{next_event, text_of, Package};
use crate::tabular_prepare::xlsx_spool::{Cap, XlsxError};
use quick_xml::events::Event;
use std::borrow::Cow;

/// Declared size of the shared-strings part, at most.
pub const MAX_SHARED_STRINGS_XML_BYTES: u64 = 128 * 1024 * 1024;

/// Entries of the table, at most.
pub const MAX_SHARED_STRINGS: usize = 10_000_000;

/// The strings of a workbook, by index.
#[derive(Debug, Default)]
pub struct SharedStrings {
    text: String,
    ends: Vec<u32>,
}

impl SharedStrings {
    /// A workbook with no shared strings.
    pub fn none() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.ends.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ends.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        let end = *self.ends.get(index)? as usize;
        let start = index
            .checked_sub(1)
            .map_or(0, |previous| self.ends[previous] as usize);
        self.text.get(start..end)
    }

    /// Bytes the table holds on the heap (text and offsets, as reserved).
    pub fn heap_bytes(&self) -> usize {
        self.text.capacity() + self.ends.capacity() * std::mem::size_of::<u32>()
    }
}

/// Excel escapes a character that XML cannot hold as `_xHHHH_` (a carriage
/// return is `_x000D_`); `_x005F_` is a literal underscore. Anything that is not
/// exactly that is left as it is.
pub fn unescape_ooxml(s: &str) -> Cow<'_, str> {
    if !s.contains("_x") {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(at) = rest.find("_x") {
        out.push_str(&rest[..at]);
        let tail = &rest[at..];
        let decoded = tail
            .get(2..6)
            .filter(|hex| hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .filter(|_| tail.as_bytes().get(6) == Some(&b'_'))
            .and_then(|hex| u32::from_str_radix(hex, 16).ok())
            .and_then(char::from_u32);
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &tail[7..];
            }
            None => {
                out.push_str("_x");
                rest = &tail[2..];
            }
        }
    }
    out.push_str(rest);
    Cow::Owned(out)
}

/// Reads the table in `part`, under the default caps.
pub fn read_shared_strings(pkg: &mut Package, part: &str) -> Result<SharedStrings, XlsxError> {
    read_shared_strings_with(pkg, part, MAX_SHARED_STRINGS_XML_BYTES, MAX_SHARED_STRINGS)
}

pub(crate) fn read_shared_strings_with(
    pkg: &mut Package,
    part: &str,
    max_xml_bytes: u64,
    max_count: usize,
) -> Result<SharedStrings, XlsxError> {
    let declared = pkg
        .summary()
        .entry(part)
        .map_or(0, |entry| entry.uncompressed);
    if declared > max_xml_bytes {
        return Err(XlsxError::TooLarge(Cap::SharedStrings));
    }
    // A string is at least `<si/>`, five bytes of XML.
    let entries = usize::try_from(declared / 5 + 1)
        .unwrap_or(usize::MAX)
        .min(max_count);
    let mut table = SharedStrings {
        text: String::with_capacity(usize::try_from(declared).unwrap_or(0)),
        ends: Vec::with_capacity(entries),
    };
    let (mut in_si, mut in_t, mut in_phonetic) = (false, false, false);
    let mut reader = pkg.xml(part, MAX_SHARED_STRINGS_XML_BYTES)?;
    let mut buf = Vec::new();
    loop {
        match next_event(&mut reader, &mut buf)? {
            Event::Eof => break,
            Event::Start(e) => match e.local_name().as_ref() {
                b"si" => in_si = true,
                b"t" if in_si => in_t = true,
                b"rPh" => in_phonetic = true,
                _ => {}
            },
            Event::Empty(e) if e.local_name().as_ref() == b"si" => push_end(&mut table, max_count)?,
            Event::End(e) => match e.local_name().as_ref() {
                b"si" => {
                    in_si = false;
                    push_end(&mut table, max_count)?;
                }
                b"t" => in_t = false,
                b"rPh" => in_phonetic = false,
                _ => {}
            },
            Event::Text(t) if in_si && in_t && !in_phonetic => {
                table.text.push_str(&unescape_ooxml(&text_of(&t)?));
            }
            Event::CData(c) if in_si && in_t && !in_phonetic => {
                let raw = std::str::from_utf8(&c).map_err(|_| {
                    XlsxError::Invalid(crate::tabular_prepare::xlsx_spool::Invalid::Xml)
                })?;
                table.text.push_str(&unescape_ooxml(raw));
            }
            _ => {}
        }
    }
    Ok(table)
}

fn push_end(table: &mut SharedStrings, max_count: usize) -> Result<(), XlsxError> {
    if table.ends.len() == max_count {
        return Err(XlsxError::TooLarge(Cap::SharedStrings));
    }
    let end =
        u32::try_from(table.text.len()).map_err(|_| XlsxError::TooLarge(Cap::SharedStrings))?;
    table.ends.push(end);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tabular_prepare::xlsx_package::XlsxLimits;
    use crate::tabular_prepare::xlsx_spool::spool_stream;
    use crate::tabular_prepare::zipfix::{build, Entry};
    use bytes::Bytes;
    use std::sync::atomic::AtomicU64;
    use tokio_util::sync::CancellationToken;

    async fn package_with(xml: &str) -> Package {
        let bytes = build(&[Entry::stored("xl/sharedStrings.xml", xml.as_bytes())]);
        let dir = tempfile::tempdir().unwrap();
        let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
        let read = AtomicU64::new(0);
        let cancel = CancellationToken::new();
        let spooled = spool_stream(dir.path(), stream, None, 1 << 30, &cancel, &read)
            .await
            .unwrap();
        Package::open_with(spooled, &XlsxLimits::default()).unwrap()
    }

    async fn read(xml: &str) -> SharedStrings {
        let mut pkg = package_with(xml).await;
        read_shared_strings(&mut pkg, "xl/sharedStrings.xml").unwrap()
    }

    fn all(t: &SharedStrings) -> Vec<&str> {
        (0..t.len()).map(|i| t.get(i).unwrap()).collect()
    }

    #[tokio::test]
    async fn plain_rich_empty_and_phonetic_strings_have_the_value_a_person_sees() {
        let xml = concat!(
            "<sst>",
            "<si><t>plain</t></si>",
            "<si><r><rPr><b/></rPr><t>ri</t></r><r><t xml:space=\"preserve\">ch </t></r><r><t>text</t></r></si>",
            "<si/>",
            "<si><t></t></si>",
            "<si><t>kanji</t><rPh sb=\"0\" eb=\"1\"><t>yomi</t></rPh><phoneticPr fontId=\"1\"/></si>",
            "<si><t>a &amp; b &lt;c&gt; &#65;</t></si>",
            "<si><t><![CDATA[<raw & text>]]></t></si>",
            "</sst>"
        );
        let t = read(xml).await;
        assert_eq!(
            all(&t),
            [
                "plain",
                "rich text",
                "",
                "",
                "kanji",
                "a & b <c> A",
                "<raw & text>"
            ]
        );
        assert_eq!(t.get(7), None);
        assert!(SharedStrings::none().get(0).is_none() && SharedStrings::none().is_empty());
    }

    #[test]
    fn excel_escapes_are_decoded_only_when_they_are_exactly_escapes() {
        assert_eq!(unescape_ooxml("a_x000D_b"), "a\rb");
        assert_eq!(unescape_ooxml("_x0041__x0042_"), "AB");
        assert_eq!(unescape_ooxml("_x005F_x0041_"), "_x0041_");
        for kept in [
            "plain",
            "snake_x_case",
            "_x00G1_",
            "_x000",
            "_xD800_",
            "tail_x",
        ] {
            assert_eq!(unescape_ooxml(kept), kept, "{kept}");
        }
    }

    #[tokio::test]
    async fn many_strings_are_found_by_index_whatever_their_length() {
        let strings: Vec<String> = (0..20_000)
            .map(|i| format!("{}-{}", i, "x".repeat(i % 37)))
            .collect();
        let body: String = strings
            .iter()
            .map(|s| format!("<si><t>{s}</t></si>"))
            .collect();
        let t = read(&format!("<sst>{body}</sst>")).await;
        assert_eq!(t.len(), 20_000);
        for i in [0, 1, 36, 37, 9_999, 19_999] {
            assert_eq!(t.get(i), Some(strings[i].as_str()));
        }
        // The text and the offsets are all that is held, as reserved up front.
        assert!(t.heap_bytes() >= strings.iter().map(String::len).sum::<usize>());
    }

    #[tokio::test]
    async fn a_table_over_the_size_or_count_cap_is_refused() {
        let xml = format!("<sst>{}</sst>", "<si><t>abc</t></si>".repeat(100));
        let mut pkg = package_with(&xml).await;
        // Over the declared size: refused before the part is read.
        let r = read_shared_strings_with(&mut pkg, "xl/sharedStrings.xml", 500, 1_000);
        assert!(matches!(r, Err(XlsxError::TooLarge(Cap::SharedStrings))));
        // Over the count: refused while reading.
        let r = read_shared_strings_with(&mut pkg, "xl/sharedStrings.xml", 1 << 20, 99);
        assert!(matches!(r, Err(XlsxError::TooLarge(Cap::SharedStrings))));
        // Exactly at the count it passes.
        let t = read_shared_strings_with(&mut pkg, "xl/sharedStrings.xml", 1 << 20, 100).unwrap();
        assert_eq!(t.len(), 100);
    }
}
