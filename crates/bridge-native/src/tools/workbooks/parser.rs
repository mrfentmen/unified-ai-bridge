use super::super::archives;
use super::*;
use calamine::{DataRef, Reader, Xlsx, XlsxFormulaMetadata};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Cursor,
};
const S: &str = "http://schemas.openxmlformats.org/spreadsheetml/2006/main";
const R: &str = "http://schemas.openxmlformats.org/officeDocument/2006/relationships";
const REL: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
type Parts = BTreeMap<String, Vec<u8>>;
pub(super) fn position(cell: &str) -> Result<(u32, u32), ToolFailure> {
    if cell.len() > 10 {
        return Err(invalid("use an A1 cell address"));
    }
    let split = cell
        .find(|c: char| c.is_ascii_digit())
        .ok_or_else(|| invalid("use an A1 cell address"))?;
    let (a, b) = cell.split_at(split);
    if a.is_empty()
        || a.len() > 3
        || !a.bytes().all(|c| c.is_ascii_uppercase())
        || b.starts_with('0')
        || !b.bytes().all(|c| c.is_ascii_digit())
    {
        return Err(invalid(
            "use uppercase A1 addresses without sheet prefixes or dollar signs",
        ));
    }
    let column = a
        .bytes()
        .fold(0u32, |n, c| n * 26 + u32::from(c - b'A' + 1));
    let row: u32 = b.parse().map_err(|_| invalid("invalid row"))?;
    if !(1..=1048576).contains(&row) || !(1..=16384).contains(&column) {
        return Err(invalid("cell exceeds XLSX row/column bounds"));
    }
    Ok((row - 1, column - 1))
}
pub(super) fn selection(range: &str) -> Result<((u32, u32), (u32, u32)), ToolFailure> {
    let (a, b) = range.split_once(':').unwrap_or((range, range));
    let first = position(a)?;
    let last = position(b)?;
    if first.0 > last.0
        || first.1 > last.1
        || (last.0 - first.0 + 1) as u64 * (last.1 - first.1 + 1) as u64 > 400
    {
        return Err(invalid(
            "select an ordered rectangular range of at most 400 cells",
        ));
    }
    Ok((first, last))
}
fn address((row, column): (u32, u32)) -> String {
    let mut col = column + 1;
    let mut chars = Vec::new();
    while col > 0 {
        chars.push((b'A' + ((col - 1) % 26) as u8) as char);
        col = (col - 1) / 26;
    }
    format!("{}{}", chars.iter().rev().collect::<String>(), row + 1)
}
fn xml(bytes: &[u8]) -> Result<roxmltree::Document<'_>, ToolFailure> {
    let s = std::str::from_utf8(bytes)
        .map_err(|_| unsupported("XLSX XML must be UTF-8 in this subset"))?;
    if s.contains("<!DOCTYPE") {
        return Err(unsupported("XLSX DTDs are refused"));
    }
    let d = roxmltree::Document::parse_with_options(
        s,
        roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: 150000,
            ..Default::default()
        },
    )
    .map_err(|e| unsupported(format!("invalid or unsupported workbook XML: {e}")))?;
    if d.descendants().any(|n| n.ancestors().take(65).count() > 64) {
        return Err(unsupported("workbook XML nesting exceeds 64 levels"));
    }
    Ok(d)
}
fn part<'a>(parts: &'a Parts, name: &str) -> Result<roxmltree::Document<'a>, ToolFailure> {
    xml(parts
        .get(name)
        .ok_or_else(|| unsupported(format!("missing XLSX part {name}")))?)
}
fn unpack(bytes: &[u8]) -> Result<Parts, ToolFailure> {
    let (mut archive, plan) = archives::inspect(bytes)?;
    if plan.expanded > 32 * 1024 * 1024 {
        return Err(unsupported("XLSX expansion exceeds 32 MiB"));
    }
    let mut parts = Parts::new();
    let mut nodes = 0;
    for entry in plan.entries {
        if entry.directory {
            continue;
        }
        let name = entry.path.to_ascii_lowercase();
        if name.contains("vba") || name.contains("macrosheet") {
            return Err(unsupported("macro-bearing workbooks are refused"));
        }
        let mut data = Vec::new();
        archive
            .by_name(&entry.path)
            .map_err(io)?
            .take(entry.bytes + 1)
            .read_to_end(&mut data)
            .map_err(io)?;
        if data.len() as u64 != entry.bytes {
            return Err(unsupported("XLSX entry size mismatch"));
        }
        if name.ends_with(".xml") || name.ends_with(".rels") {
            if data.len() > 8 * 1024 * 1024 {
                return Err(unsupported("XLSX XML part exceeds 8 MiB"));
            }
            let doc = xml(&data)?;
            nodes += doc.descendants().count();
            if nodes > 300000 {
                return Err(unsupported("XLSX exceeds 300,000 XML nodes"));
            }
            // Calamine reserves shared-string capacity from uniqueCount, and
            // shared formula indices may otherwise grow internal vectors.
            for n in doc.descendants().filter(|n| n.is_element()) {
                for a in n.attributes() {
                    if matches!(a.name(), "count" | "uniqueCount" | "si")
                        && a.value().parse::<usize>().map_or(true, |v| v > 100000)
                    {
                        return Err(unsupported(
                            "XLSX declared count or shared index exceeds 100,000",
                        ));
                    }
                }
            }
        }
        parts.insert(entry.path, data);
    }
    let types = part(&parts, "[Content_Types].xml")?;
    if types
        .descendants()
        .filter_map(|n| n.attribute("ContentType"))
        .any(|s| s.to_ascii_lowercase().contains("macro") || s.to_ascii_lowercase().contains("vba"))
    {
        return Err(unsupported("macro document content types are refused"));
    }
    if !types.descendants().any(|n| {
        n.has_tag_name((TYPES, "Override"))
            && n.attribute("PartName") == Some("/xl/workbook.xml")
            && n.attribute("ContentType")
                == Some(
                    "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
                )
    }) {
        return Err(unsupported(
            "requires a non-macro XLSX workbook at xl/workbook.xml",
        ));
    }
    Ok(parts)
}
#[derive(Clone)]
struct Sheet {
    name: String,
    path: String,
    visibility: String,
}
fn sheets(parts: &Parts) -> Result<Vec<Sheet>, ToolFailure> {
    let workbook = part(parts, "xl/workbook.xml")?;
    if !workbook.root_element().has_tag_name((S, "workbook")) {
        return Err(unsupported("unsupported workbook namespace"));
    }
    let rels = part(parts, "xl/_rels/workbook.xml.rels")?;
    let mut ids = BTreeMap::new();
    for n in rels
        .descendants()
        .filter(|n| n.has_tag_name((REL, "Relationship")))
    {
        let id = n
            .attribute("Id")
            .ok_or_else(|| unsupported("relationship missing Id"))?;
        if ids.insert(id, n).is_some() {
            return Err(unsupported("duplicate relationship Id"));
        }
    }
    let mut result = Vec::new();
    let mut names = BTreeSet::new();
    let mut paths = BTreeSet::new();
    for n in workbook
        .descendants()
        .filter(|n| n.has_tag_name((S, "sheet")))
    {
        let name = n
            .attribute("name")
            .ok_or_else(|| unsupported("sheet missing name"))?;
        if name.is_empty() || name.len() > 128 || !names.insert(name.to_lowercase()) {
            return Err(unsupported(
                "sheet names must be unique and at most 128 bytes",
            ));
        }
        let rel = ids
            .get(
                n.attribute((R, "id"))
                    .ok_or_else(|| unsupported("sheet relationship missing"))?,
            )
            .ok_or_else(|| unsupported("sheet relationship unavailable"))?;
        if rel.attribute("TargetMode") == Some("External")
            || !rel
                .attribute("Type")
                .is_some_and(|t| t == format!("{R}/worksheet"))
        {
            return Err(unsupported(
                "only internal worksheet relationships are supported",
            ));
        }
        let target = rel
            .attribute("Target")
            .ok_or_else(|| unsupported("sheet target missing"))?;
        let path = if target.starts_with("/xl/") {
            target[1..].to_string()
        } else {
            format!("xl/{target}")
        };
        if path.contains('\\')
            || path.contains('%')
            || path
                .split('/')
                .any(|p| p == ".." || p == "." || p.is_empty())
            || !path.starts_with("xl/worksheets/")
            || !path.ends_with(".xml")
            || !parts.contains_key(&path)
            || !paths.insert(path.clone())
        {
            return Err(unsupported("ambiguous or unsupported worksheet target"));
        }
        if !part(parts, &path)?
            .root_element()
            .has_tag_name((S, "worksheet"))
        {
            return Err(unsupported("unsupported worksheet root or namespace"));
        }
        let visibility = n.attribute("state").unwrap_or("visible").to_string();
        if !matches!(visibility.as_str(),"visible"|"hidden"|"veryHidden") {return Err(unsupported("unsupported worksheet visibility"));}
        result.push(Sheet {
            name: name.into(),
            path,
            visibility,
        });
    }
    if result.is_empty() || result.len() > 50 {
        return Err(unsupported("XLSX must contain 1–50 worksheets"));
    }
    Ok(result)
}
fn value(data: DataRef<'_>) -> Result<Value, ToolFailure> {
    Ok(match data {
        DataRef::Int(v) => json!({"type":"number","value":v}),
        DataRef::Float(v) => {
            if !v.is_finite() {
                return Err(unsupported("non-finite worksheet value"));
            }
            json!({"type":"number","value":v})
        }
        DataRef::String(s) => {
            if s.len() > 8192 {
                return Err(unsupported("cell text exceeds 8192 bytes"));
            }
            json!({"type":"string","value":s})
        }
        DataRef::SharedString(s) => {
            if s.len() > 8192 {
                return Err(unsupported("cell text exceeds 8192 bytes"));
            }
            json!({"type":"string","value":s})
        }
        DataRef::Bool(b) => json!({"type":"boolean","value":b}),
        DataRef::Empty => json!({"type":"empty","value":null}),
        DataRef::Error(e) => json!({"type":"error","value":e.to_string()}),
        DataRef::DateTime(d) => {
            json!({"type":if d.is_datetime(){"date_serial"}else{"duration_serial"},"value":d.as_f64(),"date_parts":d.to_ymd_hms_milli(),"note":"Excel serial and interpreted components; no timezone or display-format fidelity"})
        }
        DataRef::DateTimeIso(s) => json!({"type":"date_iso","value":s}),
        DataRef::DurationIso(s) => json!({"type":"duration_iso","value":s}),
    })
}
fn formula(f: Option<XlsxFormulaMetadata>) -> Result<Value, ToolFailure> {
    let v = match f {
        None => Value::Null,
        Some(XlsxFormulaMetadata::Normal { formula }) => json!({"kind":"normal","text":formula}),
        Some(XlsxFormulaMetadata::Shared {
            shared_index,
            range,
            formula,
        }) => {
            json!({"kind":"shared_anchor","index":shared_index,"range":range.map(|r|format!("{}:{}",address(r.start),address(r.end))),"text":formula})
        }
        Some(XlsxFormulaMetadata::SharedDerived { shared_index }) => {
            json!({"kind":"shared_derived","index":shared_index,"text":null})
        }
        Some(_) => return Err(unsupported("unknown formula metadata variant")),
    };
    if v.to_string().len() > 10000 {
        return Err(unsupported("formula metadata exceeds 10,000 bytes"));
    }
    Ok(v)
}
fn read_range(
    book: &mut Xlsx<Cursor<&[u8]>>,
    sheet: &str,
    range: &str,
) -> Result<Value, ToolFailure> {
    let (first, last) = selection(range)?;
    let mut reader = book
        .worksheet_cells_reader(sheet)
        .map_err(|e| unsupported(format!("worksheet unavailable: {e}")))?;
    let mut found = BTreeMap::new();
    let mut count = 0;
    let mut seen = BTreeSet::new();
    while let Some(cell) = reader
        .next_cell_with_formula_metadata()
        .map_err(|e| unsupported(format!("invalid worksheet: {e}")))?
    {
        count += 1;
        if count > 100000 {
            return Err(unsupported("worksheet exceeds 100,000 stored cells"));
        }
        if !seen.insert(cell.pos) {
            return Err(unsupported("duplicate cell address"));
        }
        if cell.pos.0 >= first.0
            && cell.pos.0 <= last.0
            && cell.pos.1 >= first.1
            && cell.pos.1 <= last.1
        {
            let mut v = value(cell.value)?;
            v["formula"] = formula(cell.formula)?;
            v["cached"] = json!(!v["formula"].is_null());
            found.insert(cell.pos, v);
        }
    }
    let mut rows = Vec::new();
    for row in first.0..=last.0 {
        let mut cells = Vec::new();
        for col in first.1..=last.1 {
            let mut v = found.remove(&(row, col)).unwrap_or_else(
                || json!({"type":"empty","value":null,"formula":null,"cached":false}),
            );
            v["cell"] = json!(address((row, col)));
            cells.push(v);
        }
        rows.push(json!({"row":row+1,"cells":cells}));
    }
    let data = json!({"format":"xlsx","sheet":sheet,"range":range,"rows":rows,"formula_evaluation":"unavailable; formula cells expose stored caches, which may be absent or stale; shared-derived formulas are not expanded","formatting":"typed stored values only; no workbook layout or formatted-text reproduction","links":"never opened"});
    if data.to_string().len() > 45000 {
        return Err(unsupported(
            "selected range exceeds 45,000 result bytes; request a smaller range",
        ));
    }
    Ok(data)
}
fn edit_blockers(parts: &Parts) -> Result<Vec<String>, ToolFailure> {
    let mut blockers = BTreeSet::new();
    for (name, bytes) in parts {
        let basic = matches!(
            name.as_str(),
            "[Content_Types].xml"
                | "_rels/.rels"
                | "xl/workbook.xml"
                | "xl/_rels/workbook.xml.rels"
                | "xl/styles.xml"
                | "xl/sharedStrings.xml"
        ) || name.starts_with("docProps/")
            || name.starts_with("xl/theme/")
            || (name.starts_with("xl/worksheets/")
                && !name[14..].contains('/')
                && name.ends_with(".xml"));
        if !basic {
            blockers.insert("additional package parts (drawings, tables, links, signatures or other unsupported features)".to_string());
        }
        if name.ends_with(".rels")
            && xml(bytes)?
                .descendants()
                .any(|n| n.attribute("TargetMode") == Some("External"))
        {
            blockers.insert("external relationships".into());
        }
        if name == "xl/sharedStrings.xml"
            && xml(bytes)?
                .descendants()
                .any(|n| n.has_tag_name((S, "r")) || n.has_tag_name((S, "rPh")))
        {
            blockers.insert("rich or phonetic shared strings".into());
        }
        if name.starts_with("xl/worksheets/") && name.ends_with(".xml") {
            let d = xml(bytes)?;
            for node in d.root_element().children().filter(|n| n.is_element()) {
                if node.tag_name().namespace() != Some(S)
                    || !matches!(
                        node.tag_name().name(),
                        "sheetPr"
                            | "dimension"
                            | "sheetViews"
                            | "sheetFormatPr"
                            | "cols"
                            | "sheetData"
                            | "printOptions"
                            | "pageMargins"
                            | "pageSetup"
                            | "headerFooter"
                    )
                {
                    if blockers.len() < 32 {
                        blockers.insert(format!(
                            "worksheet feature {}",
                            node.tag_name().name().chars().take(64).collect::<String>()
                        ));
                    } else {
                        blockers.insert("additional unsupported worksheet features".into());
                    }
                }
            }
            if d.descendants().any(|n| n.has_tag_name((S, "f"))) {
                blockers.insert("formulas (no recalculation available)".into());
            }
        }
    }
    if part(parts, "xl/workbook.xml")?.descendants().any(|n| {
        n.has_tag_name((S, "definedName"))
            || n.has_tag_name((S, "workbookProtection"))
            || n.has_tag_name((S, "externalReference"))
            || n.has_tag_name((S, "extLst"))
    }) {
        blockers
            .insert("defined names, protection, extensions or external workbook references".into());
    }
    Ok(blockers.into_iter().collect())
}
fn escape(s: &str) -> Result<String, ToolFailure> {
    if s.len() > 8192
        || s.chars().any(|c| {
            !(c == '\t'
                || c == '\n'
                || c == '\r'
                || ('\u{20}'..='\u{d7ff}').contains(&c)
                || ('\u{e000}'..='\u{fffd}').contains(&c)
                || c >= '\u{10000}')
        })
    {
        return Err(invalid(
            "literal text must be XML-safe and at most 8192 bytes",
        ));
    }
    // OOXML's _xHHHH_ escape syntax must remain literal when supplied as text.
    let mut literal = String::new();
    let mut iter = s.char_indices().peekable();
    while let Some((i, c)) = iter.next() {
        if c == '_'
            && s.as_bytes().get(i + 1) == Some(&b'x')
            && s.as_bytes().get(i + 6) == Some(&b'_')
            && s.as_bytes()
                .get(i + 2..i + 6)
                .is_some_and(|h| h.iter().all(u8::is_ascii_hexdigit))
        {
            literal.push_str("_x005F_");
        } else {
            literal.push(c);
        }
    }
    Ok(literal
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\r', "&#13;"))
}
fn edit(
    bytes: &[u8],
    parts: &Parts,
    sheets: &[Sheet],
    sheet: &str,
    range: &str,
    values: Vec<Vec<Value>>,
) -> Result<Value, ToolFailure> {
    let blockers = edit_blockers(parts)?;
    if !blockers.is_empty() {
        return Err(unsupported(format!(
            "XLSX editing refused: {}",
            blockers.join("; ")
        )));
    }
    let (first, last) = selection(range)?;
    if (last.0 - first.0 + 1) as usize != values.len()
        || values
            .iter()
            .any(|r| r.len() != (last.1 - first.1 + 1) as usize)
    {
        return Err(invalid(
            "values must be a rectangular matrix matching the selected range",
        ));
    }
    if values.len() * (last.1 - first.1 + 1) as usize > 100 {
        return Err(invalid("edits support at most 100 cells"));
    }
    let selected = sheets
        .iter()
        .find(|s| s.name == sheet)
        .ok_or_else(|| invalid("sheet name not found"))?;
    let original = parts.get(&selected.path).unwrap();
    let document = xml(original)?;
    if !document.root_element().has_tag_name((S, "worksheet")) {
        return Err(unsupported("unsupported worksheet root"));
    }
    if document.root_element().lookup_prefix(S).is_some() {
        return Err(unsupported(
            "editing requires the standard default worksheet namespace",
        ));
    }
    let mut nodes = BTreeMap::new();
    for cell in document.descendants().filter(|n| n.has_tag_name((S, "c"))) {
        let pos = position(
            cell.attribute("r")
                .ok_or_else(|| unsupported("editing requires explicit cell addresses"))?,
        )?;
        if nodes.insert(pos, cell).is_some() {
            return Err(unsupported("duplicate worksheet cell"));
        }
    }
    let mut book = Xlsx::new(Cursor::new(bytes))
        .map_err(|e| unsupported(format!("unsupported workbook: {e}")))?;
    let before = read_range(&mut book, sheet, range)?;
    let mut changes = Vec::new();
    let mut splices = Vec::new();
    for (row, row_values) in values.iter().enumerate() {
        for (col, v) in row_values.iter().enumerate() {
            let pos = (first.0 + row as u32, first.1 + col as u32);
            let name = address(pos);
            let node = nodes.get(&pos).ok_or_else(|| {
                unsupported(format!(
                    "cell {name} is absent; this editor only changes existing stored cells"
                ))
            })?;
            if node.parent().and_then(|p| p.lookup_namespace_uri(None)) != Some(S) {
                return Err(unsupported(
                    "cell parent must use the default spreadsheet namespace",
                ));
            }
            if node
                .attributes()
                .any(|a| a.namespace().is_some() || !matches!(a.name(), "r" | "s" | "t"))
                || node.children().filter(|n| n.is_element()).any(|n| {
                    n.tag_name().namespace() != Some(S)
                        || !matches!(n.tag_name().name(), "v" | "is")
                })
            {
                return Err(unsupported(format!("cell {name} has unsupported metadata")));
            }
            if node
                .descendants()
                .any(|n| n.has_tag_name((S, "r")) || n.has_tag_name((S, "rPh")))
            {
                return Err(unsupported(format!(
                    "cell {name} has rich or phonetic text; editing would lose it"
                )));
            }
            for child in node.descendants().skip(1).filter(|n| n.is_element()) {
                if child.tag_name().namespace() != Some(S)
                    || !matches!(child.tag_name().name(), "is" | "t" | "v")
                    || child.attributes().any(|a| {
                        !(child.tag_name().name() == "t"
                            && a.namespace() == Some("http://www.w3.org/XML/1998/namespace")
                            && a.name() == "space")
                    })
                {
                    return Err(unsupported(format!(
                        "cell {name} has unsupported nested content"
                    )));
                }
            }
            let style = node
                .attribute("s")
                .map(|s| {
                    s.parse::<u32>()
                        .map_err(|_| unsupported("invalid cell style index"))?;
                    Ok::<_, ToolFailure>(format!(" s=\"{s}\""))
                })
                .transpose()?
                .unwrap_or_default();
            let content=match v{
            Value::Null=>format!("<c r=\"{name}\"{style}/>"),
            Value::Bool(b)=>format!("<c r=\"{name}\"{style} t=\"b\"><v>{}</v></c>",u8::from(*b)),
            Value::Number(n)=>{let f=n.as_f64().ok_or_else(||invalid("invalid numeric value"))?;if !f.is_finite()||f.abs()>9_007_199_254_740_991.0{return Err(invalid("numeric edits must be finite and within exact-integer f64 bounds; use text for large IDs"));}format!("<c r=\"{name}\"{style} t=\"n\"><v>{n}</v></c>")},
            Value::String(s)=>format!("<c r=\"{name}\"{style} t=\"inlineStr\"><is><t xml:space=\"preserve\">{}</t></is></c>",escape(s)?),
            _=>return Err(invalid("cell edits accept only string, number, boolean or null; formulas and objects are refused")),
        };
            changes.push(json!({"cell":name,"before":before["rows"][row]["cells"][col],"after":v}));
            splices.push((node.range(), content));
        }
    }
    splices.sort_by_key(|(r, _)| r.start);
    let mut updated = original.clone();
    for (range, text) in splices.into_iter().rev() {
        updated.splice(range, text.bytes());
    }
    xml(&updated)?;
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(io)?;
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for i in 0..zip.len() {
        let file = zip.by_index(i).map_err(io)?;
        let name = file.name().map_err(io)?.into_owned();
        if name == selected.path {
            writer
                .start_file(
                    name,
                    zip::write::SimpleFileOptions::default()
                        .compression_method(zip::CompressionMethod::Deflated),
                )
                .map_err(io)?;
            writer.write_all(&updated).map_err(io)?;
        } else {
            writer.raw_copy_file(file).map_err(io)?;
        }
    }
    let output = writer.finish().map_err(io)?.into_inner();
    if output.len() > 16 * 1024 * 1024 {
        return Err(unsupported("edited XLSX exceeds 16 MiB"));
    }
    // Independent read path verifies values; verify all other package payloads.
    let checked = unpack(&output)?;
    if checked.len() != parts.len()
        || parts
            .iter()
            .any(|(name, data)| name != &selected.path && checked.get(name) != Some(data))
    {
        return Err(io("untouched workbook parts failed preservation check"));
    }
    let mut reopened = Xlsx::new(Cursor::new(output.as_slice())).map_err(io)?;
    let after = read_range(&mut reopened, sheet, range)?;
    for (row, vals) in values.iter().enumerate() {
        for (col, value) in vals.iter().enumerate() {
            let got = &after["rows"][row]["cells"][col];
            let matches = if value.is_number() {
                got["value"].as_f64() == value.as_f64()
            } else {
                got["value"] == *value
            };
            if !matches || !got["formula"].is_null() {
                return Err(io(format!(
                    "cell verification failed at {}",
                    address((first.0 + row as u32, first.1 + col as u32))
                )));
            }
        }
    }
    Ok(
        json!({"format":"xlsx","sheet":sheet,"range":range,"changes":changes,"preservation":"Only selected existing cell elements changed. Other worksheet XML bytes and all other package payloads preserved; selected style indices retained. ZIP container bytes may differ.","xlsx":STANDARD.encode(output)}),
    )
}
pub(super) fn process(bytes: &[u8], request: Request) -> Result<Value, ToolFailure> {
    let parts = unpack(bytes)?;
    let sheets = sheets(&parts)?;
    match request {
        Request::Edit {
            sheet,
            range,
            values,
        } => edit(bytes, &parts, &sheets, &sheet, &range, values),
        Request::Read { sheet, range } => {
            if !sheets.iter().any(|s| s.name == sheet) {
                return Err(invalid("sheet name not found"));
            }
            let mut book = Xlsx::new(Cursor::new(bytes)).map_err(io)?;
            read_range(&mut book, &sheet, &range)
        }
        Request::Inspect => {
            let blockers = edit_blockers(&parts)?;
            let mut data = Vec::new();
            let mut total = 0;
            for sheet in &sheets {
                let doc = part(&parts, &sheet.path)?;
                let mut count = 0;
                let mut formulas = 0;
                let mut max = (0, 0);
                let mut seen = BTreeSet::new();
                for cell in doc.descendants().filter(|n| n.has_tag_name((S, "c"))) {
                    count += 1;
                    total += 1;
                    if total > 100000 {
                        return Err(unsupported("workbook exceeds 100,000 stored cells"));
                    }
                    let p = position(
                        cell.attribute("r")
                            .ok_or_else(|| unsupported("cell missing explicit address"))?,
                    )?;
                    if !seen.insert(p) {
                        return Err(unsupported("duplicate cell address"));
                    }
                    max.0 = max.0.max(p.0 + 1);
                    max.1 = max.1.max(p.1 + 1);
                    formulas += usize::from(cell.children().any(|n| n.has_tag_name((S, "f"))));
                }
                data.push(json!({"name":sheet.name,"visibility":sheet.visibility,"stored_cells":count,"formula_cells":formulas,"last_row":max.0,"last_column":max.1}));
            }
            Ok(
                json!({"format":"xlsx","sheets":data,"stored_cells":total,"edit_supported":blockers.is_empty(),"edit_limitations":blockers,"edit_subset":"Create-only output; change 1–100 existing plain cells. No formulas, merges, tables, drawings, protection, defined names or external links. Cell metadata may impose further limits.","formula_evaluation":"unavailable; stored caches may be missing or stale","formatting":"not rendered; typed reads may differ from displayed text","links":"never opened"}),
            )
        }
    }
}
