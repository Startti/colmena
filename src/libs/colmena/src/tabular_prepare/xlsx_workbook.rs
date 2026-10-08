//! The structure of a workbook: which parts hold its sheets, shared strings and
//! styles, and the date system. Only the relationships the workbook names are
//! followed, and every target must resolve to a part the archive has.
//!
//! The workbook part is found through the package relationships
//! (`_rels/.rels`, type `officeDocument`), never by a fixed name. Bounded by
//! construction: at most [`MAX_SHEETS`] `sheet` elements are read (the rest of
//! the part is not), a sheet name is cut at 255 characters, and a relationships
//! part is scanned for the ids the workbook asked for only, so what is kept is a
//! handful of strings whatever the part's size.
//!
//! Decisions. A sheet whose relationship is not a worksheet (a chart sheet, a
//! dialog sheet, a macro sheet) has no cells and is not a table. Hidden and very
//! hidden worksheets are kept: the file is what the user uploaded, the manifest
//! lists every table, and a hidden sheet is not a reason to drop data silently.

use crate::tabular_prepare::xlsx_package::{attribute, next_event, Package, MAX_SMALL_PART_BYTES};
use crate::tabular_prepare::xlsx_spool::{Cap, Invalid, XlsxError};
use quick_xml::events::Event;
use std::collections::HashMap;

/// A name is cut at this many characters when read (Excel allows 31).
const MAX_SHEET_NAME_CHARS: usize = 255;

/// One worksheet of the workbook, in workbook order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetRef {
    pub name: String,
    /// The part holding its cells.
    pub part: String,
    /// `hidden` or `veryHidden` in the workbook.
    pub hidden: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workbook {
    pub sheets: Vec<SheetRef>,
    /// The 1904 date system (`workbookPr date1904`).
    pub date1904: bool,
    pub shared_strings: Option<String>,
    pub styles: Option<String>,
}

/// `target` as a part name, relative to `base_dir` (no trailing slash), or
/// `None` when it climbs out of the package.
fn resolve(base_dir: &str, target: &str) -> Option<String> {
    let joined = match target.strip_prefix('/') {
        Some(absolute) => absolute.to_string(),
        None if base_dir.is_empty() => target.to_string(),
        None => format!("{base_dir}/{target}"),
    };
    let mut parts: Vec<&str> = Vec::new();
    for segment in joined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            other => parts.push(other),
        }
    }
    Some(parts.join("/"))
}

/// The directory of a part name and the name of its relationships part.
fn rels_of(part: &str) -> (String, String) {
    match part.rsplit_once('/') {
        Some((dir, file)) => (dir.to_string(), format!("{dir}/_rels/{file}.rels")),
        None => (String::new(), format!("_rels/{part}.rels")),
    }
}

/// What a relationships part yields: `(Type, Target)` by id, and the first target
/// of each wanted type.
type Relationships = (HashMap<String, (String, String)>, HashMap<String, String>);

/// A relationship: `(Type, Target)` by `Id`, for the ids `wanted` and, whatever
/// the id, the first of each type in `by_type` (given by its last path segment).
fn read_relationships(
    pkg: &mut Package,
    part: &str,
    wanted: &dyn Fn(&str) -> bool,
    by_type: &[&str],
) -> Result<Relationships, XlsxError> {
    let mut by_id = HashMap::new();
    let mut first_of_type: HashMap<String, String> = HashMap::new();
    let mut reader = pkg.xml(part, MAX_SMALL_PART_BYTES)?;
    let mut buf = Vec::new();
    loop {
        match next_event(&mut reader, &mut buf)? {
            Event::Eof => break,
            Event::Start(e) | Event::Empty(e) if e.local_name().as_ref() == b"Relationship" => {
                let (Some(id), Some(kind), Some(target)) = (
                    attribute(&e, b"Id")?,
                    attribute(&e, b"Type")?,
                    attribute(&e, b"Target")?,
                ) else {
                    continue;
                };
                if attribute(&e, b"TargetMode")?.as_deref() == Some("External") {
                    continue;
                }
                let kind = kind.rsplit('/').next().unwrap_or("").to_string();
                if by_type.contains(&kind.as_str()) && !first_of_type.contains_key(&kind) {
                    first_of_type.insert(kind.clone(), target.clone());
                }
                if wanted(&id) {
                    by_id.insert(id, (kind, target));
                }
            }
            _ => {}
        }
    }
    Ok((by_id, first_of_type))
}

/// Reads the structure of the workbook in `pkg`.
pub fn read_workbook(pkg: &mut Package) -> Result<Workbook, XlsxError> {
    let invalid = |i: Invalid| XlsxError::Invalid(i);
    if !pkg.has("_rels/.rels") {
        return Err(invalid(Invalid::NoWorkbook));
    }
    let (_, roots) = read_relationships(pkg, "_rels/.rels", &|_| false, &["officeDocument"])?;
    let workbook_part = roots
        .get("officeDocument")
        .and_then(|t| resolve("", t))
        .filter(|p| pkg.has(p))
        .ok_or(invalid(Invalid::NoWorkbook))?;

    // The sheets and the date system, from the workbook part.
    let mut listed: Vec<(String, String, bool)> = Vec::new();
    let mut date1904 = false;
    {
        let max = pkg.max_sheets();
        let mut reader = pkg.xml(&workbook_part, MAX_SMALL_PART_BYTES)?;
        let mut buf = Vec::new();
        loop {
            match next_event(&mut reader, &mut buf)? {
                Event::Eof => break,
                Event::Start(e) | Event::Empty(e) => match e.local_name().as_ref() {
                    b"workbookPr" => {
                        date1904 =
                            matches!(attribute(&e, b"date1904")?.as_deref(), Some("1" | "true"));
                    }
                    b"sheet" => {
                        if listed.len() == max {
                            return Err(XlsxError::TooLarge(Cap::Sheets));
                        }
                        let name = attribute(&e, b"name")?.unwrap_or_default();
                        let rid = attribute(&e, b"id")?.ok_or(invalid(Invalid::BadRelationship))?;
                        let hidden = matches!(
                            attribute(&e, b"state")?.as_deref(),
                            Some("hidden" | "veryHidden")
                        );
                        let name = name.chars().take(MAX_SHEET_NAME_CHARS).collect();
                        listed.push((name, rid, hidden));
                    }
                    _ => {}
                },
                _ => {}
            }
        }
    }

    let (dir, rels_part) = rels_of(&workbook_part);
    if !pkg.has(&rels_part) {
        return Err(invalid(Invalid::BadRelationship));
    }
    let ids: std::collections::HashSet<&str> =
        listed.iter().map(|(_, id, _)| id.as_str()).collect();
    let (by_id, first) = read_relationships(
        pkg,
        &rels_part,
        &|id| ids.contains(id),
        &["sharedStrings", "styles"],
    )?;
    let part_of = |target: &str| -> Result<String, XlsxError> {
        let part = resolve(&dir, target).ok_or(invalid(Invalid::BadRelationship))?;
        if pkg.has(&part) {
            Ok(part)
        } else {
            Err(invalid(Invalid::MissingPart))
        }
    };
    let mut sheets: Vec<SheetRef> = Vec::new();
    for (name, rid, hidden) in &listed {
        let (kind, target) = by_id.get(rid).ok_or(invalid(Invalid::BadRelationship))?;
        if kind == "worksheet" {
            let part = part_of(target)?;
            // Up to 256 `sheet` elements could name one part, each read (sampled,
            // written, restarted) as if it were another sheet: one part, one sheet.
            if sheets.iter().any(|s| s.part == part) {
                return Err(invalid(Invalid::BadRelationship));
            }
            sheets.push(SheetRef {
                name: name.clone(),
                part,
                hidden: *hidden,
            });
        }
    }
    let optional = |kind: &str| first.get(kind).map(|t| part_of(t)).transpose();
    Ok(Workbook {
        sheets,
        date1904,
        shared_strings: optional("sharedStrings")?,
        styles: optional("styles")?,
    })
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

    async fn open(bytes: Vec<u8>, limits: &XlsxLimits) -> Package {
        let dir = tempfile::tempdir().unwrap();
        let stream = Box::pin(futures::stream::iter(vec![Ok(Bytes::from(bytes))]));
        let read = AtomicU64::new(0);
        let cancel = CancellationToken::new();
        let spooled = spool_stream(dir.path(), stream, None, 1 << 30, &cancel, &read)
            .await
            .unwrap();
        Package::open_with(spooled, limits).unwrap()
    }

    async fn read(bytes: Vec<u8>) -> Result<Workbook, XlsxError> {
        read_workbook(&mut open(bytes, &XlsxLimits::default()).await)
    }

    #[tokio::test]
    async fn the_sheets_come_in_workbook_order_with_their_parts_and_visibility() {
        let bytes = Wb::new()
            .sheet("Sales", "")
            .sheet("Lookup", "")
            .sheet("Totals", "")
            .hidden(1)
            .shared(&["a"])
            .styles(&[0], &[])
            .build();
        let wb = read(bytes).await.unwrap();
        let sheets: Vec<_> = wb
            .sheets
            .iter()
            .map(|s| (s.name.as_str(), s.part.as_str(), s.hidden))
            .collect();
        assert_eq!(
            sheets,
            vec![
                ("Sales", "xl/worksheets/sheet1.xml", false),
                ("Lookup", "xl/worksheets/sheet2.xml", true),
                ("Totals", "xl/worksheets/sheet3.xml", false),
            ]
        );
        assert!(!wb.date1904);
        assert_eq!(wb.shared_strings.as_deref(), Some("xl/sharedStrings.xml"));
        assert_eq!(wb.styles.as_deref(), Some("xl/styles.xml"));
    }

    #[tokio::test]
    async fn a_workbook_written_by_a_real_library_is_read() {
        let mut book = rust_xlsxwriter::Workbook::new();
        book.add_worksheet().set_name("First").unwrap();
        let second = book.add_worksheet();
        second.set_name("Second").unwrap();
        second.write_string(0, 0, "x").unwrap();
        let bytes = book.save_to_buffer().unwrap();
        let wb = read(bytes).await.unwrap();
        let names: Vec<_> = wb.sheets.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["First", "Second"]);
        assert!(wb.shared_strings.is_some() && wb.styles.is_some());
    }

    #[tokio::test]
    async fn the_1904_date_system_is_read_and_a_chart_sheet_is_not_a_table() {
        let wb = read(Wb::new().sheet("A", "").chartsheet().date1904().build())
            .await
            .unwrap();
        assert!(wb.date1904);
        assert_eq!(wb.sheets.len(), 1);
        assert_eq!(wb.shared_strings, None);
    }

    #[tokio::test]
    async fn a_workbook_with_more_sheets_than_the_limit_is_refused_while_listing() {
        let mut wb = Wb::new();
        for i in 0..4 {
            wb = wb.sheet(&format!("S{i}"), "");
        }
        let limits = XlsxLimits {
            max_sheets: 3,
            ..XlsxLimits::default()
        };
        let mut pkg = open(wb.build(), &limits).await;
        assert_eq!(
            read_workbook(&mut pkg),
            Err(XlsxError::TooLarge(Cap::Sheets))
        );
        // At the limit it passes.
        let mut wb = Wb::new();
        for i in 0..3 {
            wb = wb.sheet(&format!("S{i}"), "");
        }
        let mut pkg = open(wb.build(), &limits).await;
        assert_eq!(read_workbook(&mut pkg).unwrap().sheets.len(), 3);
    }

    fn rels(target: &str) -> String {
        format!(
            "<Relationships><Relationship Id=\"rId1\" Type=\"http://x/worksheet\" Target=\"{target}\"/></Relationships>"
        )
    }

    fn package_with(sheet_target: &str, root: Option<&str>) -> Vec<u8> {
        let mut parts = vec![
            Entry::stored("xl/workbook.xml", b"<workbook xmlns:r=\"r\"><sheets><sheet name=\"A\" r:id=\"rId1\"/></sheets></workbook>"),
            Entry::stored("xl/_rels/workbook.xml.rels", rels(sheet_target).as_bytes()),
            Entry::stored("xl/worksheets/sheet1.xml", b"<worksheet/>"),
        ];
        if let Some(root) = root {
            parts.push(Entry::stored("_rels/.rels", root.as_bytes()));
        }
        build(&parts)
    }

    const ROOT: &str = "<Relationships><Relationship Id=\"r\" Type=\"http://x/officeDocument\" Target=\"/xl/workbook.xml\"/></Relationships>";

    #[tokio::test]
    async fn targets_are_resolved_inside_the_package_and_nowhere_else() {
        let ok = |t: &str| read(package_with(t, Some(ROOT)));
        assert_eq!(
            ok("worksheets/sheet1.xml").await.unwrap().sheets[0].part,
            "xl/worksheets/sheet1.xml"
        );
        assert_eq!(
            ok("/xl/worksheets/sheet1.xml").await.unwrap().sheets.len(),
            1
        );
        assert_eq!(
            ok("./worksheets/../worksheets/sheet1.xml")
                .await
                .unwrap()
                .sheets
                .len(),
            1
        );
        for escaping in ["../../../etc/passwd", "../../x.xml"] {
            let r = ok(escaping).await;
            assert_eq!(
                r,
                Err(XlsxError::Invalid(Invalid::BadRelationship)),
                "{escaping}"
            );
        }
        let r = ok("worksheets/nope.xml").await;
        assert_eq!(r, Err(XlsxError::Invalid(Invalid::MissingPart)));
        // A sheet whose id has no relationship.
        let no_rel = build(&[
            Entry::stored("xl/workbook.xml", b"<workbook xmlns:r=\"r\"><sheets><sheet name=\"A\" r:id=\"rId9\"/></sheets></workbook>"),
            Entry::stored("xl/_rels/workbook.xml.rels", rels("worksheets/sheet1.xml").as_bytes()),
            Entry::stored("_rels/.rels", ROOT.as_bytes()),
            Entry::stored("xl/worksheets/sheet1.xml", b"<worksheet/>"),
        ]);
        assert_eq!(
            read(no_rel).await,
            Err(XlsxError::Invalid(Invalid::BadRelationship))
        );
    }

    #[tokio::test]
    async fn a_file_without_a_workbook_is_refused_in_fixed_words() {
        let none = build(&[Entry::stored("hello.txt", b"hi")]);
        assert_eq!(
            read(none).await,
            Err(XlsxError::Invalid(Invalid::NoWorkbook))
        );
        let r = read(package_with("worksheets/sheet1.xml", None)).await;
        assert_eq!(r, Err(XlsxError::Invalid(Invalid::NoWorkbook)));
        let no_office = "<Relationships><Relationship Id=\"r\" Type=\"http://x/thumbnail\" Target=\"t.png\"/></Relationships>";
        let r = read(package_with("worksheets/sheet1.xml", Some(no_office))).await;
        assert_eq!(r, Err(XlsxError::Invalid(Invalid::NoWorkbook)));
    }

    #[tokio::test]
    async fn an_external_relationship_is_ignored_and_a_long_name_is_cut() {
        let long = "n".repeat(300);
        let wb = format!("<workbook xmlns:r=\"r\"><sheets><sheet name=\"{long}\" r:id=\"rId1\"/></sheets></workbook>");
        let rels = "<Relationships><Relationship Id=\"rId1\" Type=\"http://x/worksheet\" Target=\"worksheets/sheet1.xml\"/><Relationship Id=\"rId2\" Type=\"http://x/styles\" Target=\"http://evil/styles.xml\" TargetMode=\"External\"/></Relationships>";
        let bytes = build(&[
            Entry::stored("_rels/.rels", ROOT.as_bytes()),
            Entry::stored("xl/workbook.xml", wb.as_bytes()),
            Entry::stored("xl/_rels/workbook.xml.rels", rels.as_bytes()),
            Entry::stored("xl/worksheets/sheet1.xml", b"<worksheet/>"),
        ]);
        let wb = read(bytes).await.unwrap();
        assert_eq!(wb.sheets[0].name.chars().count(), MAX_SHEET_NAME_CHARS);
        assert_eq!(wb.styles, None);
    }

    #[tokio::test]
    async fn many_sheets_naming_one_part_are_refused() {
        let ns = "xmlns:r=\"r\"";
        let sheets: String = (1..=3)
            .map(|i| format!("<sheet name=\"S{i}\" r:id=\"rId{i}\"/>"))
            .collect();
        let rels: String = (1..=3)
            .map(|i| format!("<Relationship Id=\"rId{i}\" Type=\"http://x/worksheet\" Target=\"worksheets/sheet1.xml\"/>"))
            .collect();
        let bytes = build(&[
            Entry::stored("_rels/.rels", ROOT.as_bytes()),
            Entry::stored(
                "xl/workbook.xml",
                format!("<workbook {ns}><sheets>{sheets}</sheets></workbook>").as_bytes(),
            ),
            Entry::stored(
                "xl/_rels/workbook.xml.rels",
                format!("<Relationships>{rels}</Relationships>").as_bytes(),
            ),
            Entry::stored("xl/worksheets/sheet1.xml", b"<worksheet/>"),
        ]);
        assert_eq!(
            read(bytes).await,
            Err(XlsxError::Invalid(Invalid::BadRelationship))
        );
    }
}
