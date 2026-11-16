use super::{invalid, Block, Document};
use crate::failure::ToolFailure;
use pdf_writer::{
    types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap},
    Content, Finish, Name, Pdf, Rect, Ref, Str,
};
use std::collections::BTreeSet;
use ttf_parser::{Face, GlyphId};
use unicode_normalization::UnicodeNormalization;

const REGULAR: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Regular.ttf");
const BOLD: &[u8] = include_bytes!("../../../assets/fonts/NotoSans-Bold.ttf");
const WIDTH: f32 = 595.0;
const HEIGHT: f32 = 842.0;
const MARGIN: f32 = 54.0;
const BOTTOM: f32 = 64.0;
const MAX_PAGES: usize = 40;

struct Fonts {
    faces: [Face<'static>; 2],
}
impl Fonts {
    fn new() -> Result<Self, ToolFailure> {
        Ok(Self {
            faces: [
                Face::parse(REGULAR, 0).map_err(|_| invalid("invalid bundled regular font"))?,
                Face::parse(BOLD, 0).map_err(|_| invalid("invalid bundled bold font"))?,
            ],
        })
    }
    fn width(&self, c: char, bold: bool, size: f32) -> f32 {
        let face = &self.faces[usize::from(bold)];
        f32::from(
            face.glyph_hor_advance(face.glyph_index(c).unwrap_or(GlyphId(0)))
                .unwrap_or(0),
        ) * size
            / f32::from(face.units_per_em())
    }
    fn text_width(&self, s: &str, bold: bool, size: f32) -> f32 {
        s.chars().map(|c| self.width(c, bold, size)).sum()
    }
    fn validate(&self, s: &str) -> Result<(), ToolFailure> {
        for c in s.chars() {
            if c == '\n' {
                continue;
            }
            // Explicit simple LTR subset. NFC composes common accents first;
            // leftover combining marks, bidi controls and shaping scripts fail.
            let allowed = matches!(c,' '..='~'|'\u{a0}'..='\u{24f}'|'\u{2018}'..='\u{201f}'|'\u{2022}'|'\u{2026}'|'\u{20ac}')
                && c != '\u{ad}';
            if !allowed || self.faces.iter().any(|face| face.glyph_index(c).is_none()) {
                return Err(invalid(format!("unsupported PDF character U+{:04X}; report supports simple Latin text and selected punctuation, not complex shaping or emoji",c as u32)));
            }
        }
        Ok(())
    }
    fn wrap(&self, text: &str, bold: bool, size: f32, width: f32) -> Vec<String> {
        let mut lines = Vec::new();
        for paragraph in text.split('\n') {
            let mut line = String::new();
            let mut used = 0.0;
            for word in paragraph.split_whitespace() {
                let word_width = self.text_width(word, bold, size);
                let space = if line.is_empty() {
                    0.0
                } else {
                    self.width(' ', bold, size)
                };
                if !line.is_empty() && used + space + word_width > width {
                    lines.push(std::mem::take(&mut line));
                    used = 0.0;
                }
                if !line.is_empty() {
                    line.push(' ');
                    used += space;
                }
                for c in word.chars() {
                    let w = self.width(c, bold, size);
                    if used + w > width && !line.is_empty() {
                        lines.push(std::mem::take(&mut line));
                        used = 0.0;
                    }
                    line.push(c);
                    used += w;
                }
            }
            lines.push(line);
        }
        lines
    }
}
#[derive(Debug)]
struct Line {
    text: String,
    x: f32,
    y: f32,
    size: f32,
    bold: bool,
}
struct Layout {
    pages: Vec<Vec<Line>>,
    y: f32,
}
impl Layout {
    fn new() -> Self {
        Self {
            pages: vec![vec![]],
            y: HEIGHT - MARGIN,
        }
    }
    fn page(&mut self) -> Result<(), ToolFailure> {
        if self.pages.len() >= MAX_PAGES {
            return Err(invalid(
                "document exceeds 40-page limit; nothing was written",
            ));
        }
        self.pages.push(vec![]);
        self.y = HEIGHT - MARGIN;
        Ok(())
    }
    fn room(&mut self, height: f32) -> Result<(), ToolFailure> {
        if self.y - height < BOTTOM {
            self.page()?;
        }
        Ok(())
    }
    fn add(
        &mut self,
        lines: Vec<String>,
        x: f32,
        size: f32,
        leading: f32,
        bold: bool,
        bullet: bool,
    ) -> Result<(), ToolFailure> {
        // Keep at least two lines at the start/end of a paragraph where possible.
        self.room(leading * lines.len().min(2) as f32)?;
        let total = lines.len();
        for (i, text) in lines.into_iter().enumerate() {
            self.room(leading * if total - i == 2 { 2.0 } else { 1.0 })?;
            if i == 0 && bullet {
                self.pages.last_mut().unwrap().push(Line {
                    text: "•".into(),
                    x: MARGIN,
                    y: self.y - size,
                    size,
                    bold: false,
                });
            }
            self.pages.last_mut().unwrap().push(Line {
                text,
                x,
                y: self.y - size,
                size,
                bold,
            });
            self.y -= leading;
        }
        self.y -= 9.0;
        Ok(())
    }
}
fn normalize(
    text: &mut String,
    max: usize,
    single: bool,
    fonts: &Fonts,
) -> Result<(), ToolFailure> {
    if text.len() > max || text.trim().is_empty() {
        return Err(invalid(format!(
            "text must be nonempty and at most {max} UTF-8 bytes"
        )));
    }
    *text = text.nfc().collect();
    if single && text.contains('\n') {
        return Err(invalid("title and headings must not contain line breaks"));
    }
    fonts.validate(text)
}
fn layout(doc: &mut Document, fonts: &Fonts) -> Result<Layout, ToolFailure> {
    normalize(&mut doc.title, 240, true, fonts)?;
    if doc.blocks.is_empty() || doc.blocks.len() > 200 {
        return Err(invalid("document needs 1–200 blocks"));
    }
    let mut total = doc.title.len();
    for b in &mut doc.blocks {
        match b {
            Block::Heading { text } => {
                normalize(text, 240, true, fonts)?;
                total += text.len();
            }
            Block::Paragraph { text } | Block::Bullet { text } => {
                normalize(text, 4000, false, fonts)?;
                total += text.len();
            }
            Block::PageBreak => {}
        }
    }
    if total > 24000 {
        return Err(invalid("document text exceeds 24,000 UTF-8 bytes"));
    }
    for (i, b) in doc.blocks.iter().enumerate() {
        if matches!(b, Block::PageBreak)
            && (i == 0
                || i + 1 == doc.blocks.len()
                || matches!(doc.blocks[i - 1], Block::PageBreak | Block::Heading { .. }))
        {
            return Err(invalid("page breaks must separate content; no leading, trailing, repeated breaks or orphan headings"));
        }
        if matches!(b, Block::Heading { .. })
            && (i + 1 == doc.blocks.len()
                || matches!(doc.blocks[i + 1], Block::Heading { .. } | Block::PageBreak))
        {
            return Err(invalid(
                "each heading must be followed by a paragraph or bullet",
            ));
        }
    }
    let mut layout = Layout::new();
    let title = fonts.wrap(&doc.title, true, 26.0, WIDTH - MARGIN * 2.0);
    if title.len() > 3 {
        return Err(invalid("title exceeds three report lines"));
    }
    layout.add(title, MARGIN, 26.0, 35.0, true, false)?;
    layout.y -= 12.0;
    for b in &doc.blocks {
        match b {
            Block::Heading { text } => {
                let lines = fonts.wrap(text, true, 15.0, WIDTH - MARGIN * 2.0);
                // Heading plus two body lines stays on one page.
                layout.room(lines.len() as f32 * 21.0 + 53.0)?;
                layout.y -= 6.0;
                layout.add(lines, MARGIN, 15.0, 21.0, true, false)?;
            }
            Block::Paragraph { text } => layout.add(
                fonts.wrap(text, false, 11.0, WIDTH - MARGIN * 2.0),
                MARGIN,
                11.0,
                16.0,
                false,
                false,
            )?,
            Block::Bullet { text } => layout.add(
                fonts.wrap(text, false, 11.0, WIDTH - MARGIN * 2.0 - 15.0),
                MARGIN + 15.0,
                11.0,
                16.0,
                false,
                true,
            )?,
            Block::PageBreak => layout.page()?,
        }
    }
    Ok(layout)
}
fn system() -> SystemInfo<'static> {
    SystemInfo {
        registry: Str(b"Adobe"),
        ordering: Str(b"Identity"),
        supplement: 0,
    }
}
fn embed(
    pdf: &mut Pdf,
    face: &Face<'_>,
    bytes: &[u8],
    chars: &BTreeSet<char>,
    start: i32,
    name: Name<'_>,
) {
    let id = Ref::new(start);
    let cid = Ref::new(start + 1);
    let desc = Ref::new(start + 2);
    let file = Ref::new(start + 3);
    let unicode = Ref::new(start + 4);
    let map = Ref::new(start + 5);
    pdf.type0_font(id)
        .base_font(name)
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid)
        .to_unicode(unicode);
    let mut font = pdf.cid_font(cid);
    font.subtype(CidFontType::Type2)
        .base_font(name)
        .system_info(system())
        .font_descriptor(desc)
        .cid_to_gid_map_stream(map);
    let scale = 1000.0 / f32::from(face.units_per_em());
    let mut widths = font.widths();
    let mut cmap = UnicodeCmap::new(Name(b"BridgeUnicode"), system());
    let mut glyph_map = vec![0u8; (chars.last().copied().unwrap_or(' ') as usize + 1) * 2];
    for &c in chars {
        let glyph = face.glyph_index(c).unwrap();
        widths.consecutive(
            c as u16,
            [f32::from(face.glyph_hor_advance(glyph).unwrap_or(0)) * scale],
        );
        cmap.pair(c as u16, c);
        glyph_map[c as usize * 2..c as usize * 2 + 2].copy_from_slice(&glyph.0.to_be_bytes());
    }
    widths.finish();
    font.finish();
    let bbox = face.global_bounding_box();
    pdf.font_descriptor(desc)
        .name(name)
        .flags(FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(
            f32::from(bbox.x_min) * scale,
            f32::from(bbox.y_min) * scale,
            f32::from(bbox.x_max) * scale,
            f32::from(bbox.y_max) * scale,
        ))
        .italic_angle(0.0)
        .ascent(f32::from(face.ascender()) * scale)
        .descent(f32::from(face.descender()) * scale)
        .cap_height(f32::from(face.capital_height().unwrap_or(face.ascender())) * scale)
        .stem_v(80.0)
        .font_file2(file);
    pdf.stream(file, bytes)
        .pair(Name(b"Length1"), bytes.len() as i32);
    pdf.stream(map, &glyph_map);
    pdf.cmap(unicode, &cmap.finish());
}
pub(super) fn generate(doc: &mut Document) -> Result<(Vec<u8>, usize), ToolFailure> {
    let fonts = Fonts::new()?;
    let mut layout = layout(doc, &fonts)?;
    let pages = layout.pages.len();
    for (i, page) in layout.pages.iter_mut().enumerate() {
        let footer = format!("{} / {}", i + 1, pages);
        page.push(Line {
            x: WIDTH - MARGIN - fonts.text_width(&footer, false, 9.0),
            y: 35.0,
            text: footer,
            size: 9.0,
            bold: false,
        });
    }
    let chars = layout
        .pages
        .iter()
        .flatten()
        .flat_map(|l| l.text.chars())
        .collect::<BTreeSet<_>>();
    let mut pdf = Pdf::new();
    pdf.catalog(Ref::new(1)).pages(Ref::new(2));
    pdf.pages(Ref::new(2))
        .kids((0..pages).map(|i| Ref::new(20 + i as i32 * 2)))
        .count(pages as i32);
    embed(
        &mut pdf,
        &fonts.faces[0],
        REGULAR,
        &chars,
        3,
        Name(b"NotoSans-Regular"),
    );
    embed(
        &mut pdf,
        &fonts.faces[1],
        BOLD,
        &chars,
        9,
        Name(b"NotoSans-Bold"),
    );
    for (i, lines) in layout.pages.into_iter().enumerate() {
        let page_id = Ref::new(20 + i as i32 * 2);
        let content_id = Ref::new(21 + i as i32 * 2);
        let mut page = pdf.page(page_id);
        page.media_box(Rect::new(0.0, 0.0, WIDTH, HEIGHT))
            .parent(Ref::new(2))
            .contents(content_id);
        page.resources()
            .fonts()
            .pair(Name(b"R"), Ref::new(3))
            .pair(Name(b"B"), Ref::new(9));
        page.finish();
        let mut content = Content::new();
        content.set_fill_rgb(0.10, 0.12, 0.16);
        for line in lines {
            let encoded = line
                .text
                .chars()
                .flat_map(|c| (c as u16).to_be_bytes())
                .collect::<Vec<_>>();
            content
                .begin_text()
                .set_font(Name(if line.bold { b"B" } else { b"R" }), line.size)
                .next_line(line.x, line.y)
                .show(Str(&encoded))
                .end_text();
        }
        pdf.stream(content_id, &content.finish());
    }
    let bytes = pdf.finish();
    if bytes.len() > 4 * 1024 * 1024 {
        return Err(invalid("PDF exceeds 4 MiB output limit"));
    }
    Ok((bytes, pages))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::document_create::{Format, Template};
    fn document(blocks: Vec<Block>) -> Document {
        Document {
            destination: "output/report.pdf".into(),
            format: Format::Pdf,
            template: Template::Report,
            title: "A report".into(),
            blocks,
        }
    }
    #[test]
    fn fonts_layout_and_unicode_are_bounded_and_deterministic() {
        let fonts = Fonts::new().unwrap();
        let mut doc = document(
            (0..60)
                .map(|_| Block::Paragraph {
                    text: format!("Résumé naïve café Ångström {}", "longword".repeat(60)),
                })
                .collect(),
        );
        // Exceeds the overall byte budget before allocating PDF output.
        assert!(generate(&mut doc).is_err());
        doc.blocks.truncate(30);
        let layout = layout(&mut doc, &fonts).unwrap();
        assert!(layout.pages.len() > 2);
        for page in &layout.pages {
            for line in page {
                assert!(line.x >= MARGIN && line.y >= BOTTOM);
                assert!(
                    line.x + fonts.text_width(&line.text, line.bold, line.size)
                        <= WIDTH - MARGIN + 0.01
                );
            }
        }
        let first = generate(&mut doc).unwrap();
        assert_eq!(first, generate(&mut doc).unwrap());
        for text in [
            "emoji 😀",
            "العربية",
            "שלום",
            "汉字",
            "a\u{200f}b",
            "a\tB",
            "a\u{ad}b",
            "a\u{301}\u{301}",
        ] {
            assert!(fonts.validate(text).is_err(), "{text}");
        }
        let mut accent = "Cafe\u{301}".to_string();
        normalize(&mut accent, 240, false, &fonts).unwrap();
        assert_eq!(accent, "Café");
    }
    #[test]
    fn page_breaks_orphan_headings_and_page_exhaustion_are_explicit() {
        for blocks in [
            vec![Block::PageBreak],
            vec![Block::Heading {
                text: "orphan".into(),
            }],
            vec![
                Block::Paragraph {
                    text: "start".into(),
                },
                Block::PageBreak,
            ],
        ] {
            assert!(generate(&mut document(blocks)).is_err());
        }
        let mut blocks = vec![];
        for i in 0..41 {
            if i > 0 {
                blocks.push(Block::PageBreak);
            }
            blocks.push(Block::Paragraph {
                text: format!("Page {i}"),
            });
        }
        assert!(generate(&mut document(blocks))
            .unwrap_err()
            .to_string()
            .contains("40-page"));
    }
}
