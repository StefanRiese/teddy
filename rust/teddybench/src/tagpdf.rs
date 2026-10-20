//! Printable PDFs for custom tags: round covers and a backside with series and model number.

use image::RgbaImage;
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, TextRenderingMode};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str};

use crate::covers::cover_for_print;
use crate::export::JobStatus;
use crate::settings::{DataDir, Settings};
use tonie_core::tonies::TonieData;

/// A4 in millimeters
const PAGE_WIDTH: f32 = 210.0;
const PAGE_HEIGHT: f32 = 297.0;
const PT_PER_MM: f32 = 72.0 / 25.4;
/// Outline color of the cutting line (dark blue)
const OUTLINE_RGB: [f32; 3] = [0.0, 0.0, 139.0 / 255.0];

/// Where the tags go on the sheet, from the CustomTag* settings (all values in millimeters).
#[derive(Clone, Debug)]
pub struct TagLayout {
    pub margin: f32,
    pub gap_x: f32,
    pub gap_y: f32,
    pub initial_column: i32,
    pub initial_row: i32,
    pub size: f32,
    pub outline_width: f32,
}

impl TagLayout {
    pub fn from_settings(s: &Settings) -> TagLayout {
        TagLayout {
            margin: s.custom_tag_non_printable_margin as f32,
            gap_x: s.custom_tag_gap_x as f32,
            gap_y: s.custom_tag_gap_y as f32,
            initial_column: s.custom_tag_initial_column.max(0),
            initial_row: s.custom_tag_initial_row.max(0),
            size: s.custom_tag_size.max(1) as f32,
            outline_width: s.custom_tag_outline_width as f32,
        }
    }

    /// Page index and upper left corner (mm from the top left of the page) for `count` tags.
    /// Starts at the initial row/column so a partly used sheet can be printed again.
    pub fn positions(&self, count: usize) -> Vec<(usize, f32, f32)> {
        let (mut page, mut col, mut row) = (0usize, self.initial_column, self.initial_row);
        let corner = |col: i32, row: i32| {
            (
                self.margin + col as f32 * (self.size + self.gap_x),
                self.margin + row as f32 * (self.size + self.gap_y),
            )
        };
        let mut out = Vec::with_capacity(count);

        for _ in 0..count {
            let (mut x, mut y) = corner(col, row);
            if x + self.size + self.margin > PAGE_WIDTH {
                col = 0;
                row += 1;
                (x, y) = corner(col, row);
            }
            if y + self.size + self.margin > PAGE_HEIGHT {
                page += 1;
                col = 0;
                row = 0;
                (x, y) = corner(col, row);
            }
            out.push((page, x, y));
            col += 1;
        }
        out
    }
}

/// Text of a tag backside
pub struct Backside {
    pub series: String,
    pub model: String,
}

struct Writer {
    pdf: Pdf,
    next: i32,
}

impl Writer {
    fn new() -> Writer {
        Writer { pdf: Pdf::new(), next: 1 }
    }

    fn alloc(&mut self) -> Ref {
        let r = Ref::new(self.next);
        self.next += 1;
        r
    }
}

fn deflate(data: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(data, 6)
}

/// PDF y coordinate (points from the bottom) of a position in mm from the top
fn y_pt(mm_from_top: f32) -> f32 {
    (PAGE_HEIGHT - mm_from_top) * PT_PER_MM
}

/// Circle through the square (x, y, size), in mm from the top left
fn circle(content: &mut Content, x: f32, y: f32, size: f32) {
    let r = size / 2.0 * PT_PER_MM;
    let cx = (x + size / 2.0) * PT_PER_MM;
    let cy = y_pt(y + size / 2.0);
    /* four Bézier quarters, k = 4/3 * (sqrt(2) - 1) */
    let k = 0.552_284_8 * r;
    content.move_to(cx + r, cy);
    content.cubic_to(cx + r, cy + k, cx + k, cy + r, cx, cy + r);
    content.cubic_to(cx - k, cy + r, cx - r, cy + k, cx - r, cy);
    content.cubic_to(cx - r, cy - k, cx - k, cy - r, cx, cy - r);
    content.cubic_to(cx + k, cy - r, cx + r, cy - k, cx + r, cy);
    content.close_path();
}

fn outline(content: &mut Content, layout: &TagLayout, x: f32, y: f32) {
    if layout.outline_width > 0.0 {
        content.save_state();
        content.set_stroke_rgb(OUTLINE_RGB[0], OUTLINE_RGB[1], OUTLINE_RGB[2]);
        content.set_line_width(layout.outline_width * PT_PER_MM);
        circle(content, x, y, layout.size);
        content.stroke();
        content.restore_state();
    }
}

/// Writes the pages; `draw` adds the content of one tag.
fn build(
    layout: &TagLayout,
    count: usize,
    mut setup: impl FnMut(&mut Writer) -> Resources,
    mut draw: impl FnMut(&mut Content, usize, f32, f32, &Resources),
) -> Vec<u8> {
    let mut w = Writer::new();
    let catalog = w.alloc();
    let tree = w.alloc();
    let resources = setup(&mut w);

    let positions = layout.positions(count);
    let pages = positions.last().map(|p| p.0 + 1).unwrap_or(1);
    let mut page_refs = Vec::new();

    for page in 0..pages {
        let mut content = Content::new();
        for (i, &(p, x, y)) in positions.iter().enumerate() {
            if p == page {
                draw(&mut content, i, x, y, &resources);
                outline(&mut content, layout, x, y);
            }
        }
        let content_ref = w.alloc();
        w.pdf.stream(content_ref, &deflate(&content.finish())).filter(Filter::FlateDecode);

        let page_ref = w.alloc();
        let mut pg = w.pdf.page(page_ref);
        pg.media_box(Rect::new(0.0, 0.0, PAGE_WIDTH * PT_PER_MM, PAGE_HEIGHT * PT_PER_MM));
        pg.parent(tree);
        pg.contents(content_ref);
        let mut res = pg.resources();
        if !resources.images.is_empty() {
            let mut xo = res.x_objects();
            for (i, r) in resources.images.iter().enumerate() {
                if let Some(r) = r {
                    xo.pair(Name(format!("Im{i}").as_bytes()), *r);
                }
            }
        }
        if let Some(font) = resources.font {
            res.fonts().pair(Name(b"F1"), font);
        }
        res.finish();
        pg.finish();
        page_refs.push(page_ref);
    }

    w.pdf.pages(tree).kids(page_refs.iter().copied()).count(page_refs.len() as i32);
    w.pdf.catalog(catalog).pages(tree);
    w.pdf.finish()
}

#[derive(Default)]
struct Resources {
    images: Vec<Option<Ref>>,
    font: Option<Ref>,
    font_data: Option<FontInfo>,
}

/// Round covers; `None` leaves the place empty but keeps the outline.
pub fn covers_pdf(layout: &TagLayout, images: &[Option<RgbaImage>]) -> Vec<u8> {
    build(
        layout,
        images.len(),
        |w| {
            let mut res = Resources::default();
            for img in images {
                res.images.push(img.as_ref().map(|img| {
                    let r = w.alloc();
                    /* covers can be transparent, put them on white paper */
                    let rgb: Vec<u8> = img
                        .pixels()
                        .flat_map(|p| {
                            let a = p[3] as u32;
                            [0, 1, 2].map(|c| ((p[c] as u32 * a + 255 * (255 - a)) / 255) as u8)
                        })
                        .collect();
                    let data = deflate(&rgb);
                    let mut x = w.pdf.image_xobject(r, &data);
                    x.filter(Filter::FlateDecode);
                    x.width(img.width() as i32);
                    x.height(img.height() as i32);
                    x.color_space().device_rgb();
                    x.bits_per_component(8);
                    x.finish();
                    r
                }));
            }
            res
        },
        |content, i, x, y, res| {
            if res.images[i].is_some() {
                let s = layout.size * PT_PER_MM;
                content.save_state();
                content.transform([s, 0.0, 0.0, s, x * PT_PER_MM, y_pt(y + layout.size)]);
                content.x_object(Name(format!("Im{i}").as_bytes()));
                content.restore_state();
            }
        },
    )
}

struct FontInfo {
    /// (character, glyph id, advance in 1/1000 em)
    glyphs: std::collections::BTreeMap<char, (u16, f32)>,
}

impl FontInfo {
    fn glyph(&self, c: char) -> Option<(u16, f32)> {
        self.glyphs.get(&c).copied()
    }

    /// Width in units of the font size
    fn width(&self, text: &str) -> f32 {
        text.chars().filter_map(|c| self.glyph(c)).map(|(_, w)| w / 1000.0).sum()
    }
}

/// Embeds the font (Ubuntu Light, bundled with egui) for the characters used.
fn embed_font(w: &mut Writer, texts: &[&str]) -> (Ref, FontInfo) {
    let data = epaint_default_fonts::UBUNTU_LIGHT;
    let face = ttf_parser::Face::parse(data, 0).expect("bundled font");
    let scale = 1000.0 / face.units_per_em() as f32;

    let mut glyphs = std::collections::BTreeMap::new();
    for c in texts.iter().flat_map(|t| t.chars()).chain(['?']) {
        if let Some(g) = face.glyph_index(c) {
            glyphs.insert(c, (g.0, face.glyph_hor_advance(g).unwrap_or(0) as f32 * scale));
        }
    }

    let type0 = w.alloc();
    let cid = w.alloc();
    let descriptor = w.alloc();
    let file = w.alloc();
    let to_unicode = w.alloc();
    let name = Name(b"Ubuntu-Light");

    w.pdf
        .type0_font(type0)
        .base_font(name)
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid)
        .to_unicode(to_unicode);

    let mut c = w.pdf.cid_font(cid);
    c.subtype(CidFontType::Type2);
    c.base_font(name);
    c.system_info(SystemInfo {
        registry: Str(b"Adobe"),
        ordering: Str(b"Identity"),
        supplement: 0,
    });
    c.font_descriptor(descriptor);
    c.default_width(500.0);
    c.cid_to_gid_map_predefined(Name(b"Identity"));
    {
        let mut widths = c.widths();
        for &(gid, adv) in glyphs.values() {
            widths.consecutive(gid, [adv]);
        }
    }
    c.finish();

    let bbox = face.global_bounding_box();
    w.pdf
        .font_descriptor(descriptor)
        .name(name)
        .flags(FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(
            bbox.x_min as f32 * scale,
            bbox.y_min as f32 * scale,
            bbox.x_max as f32 * scale,
            bbox.y_max as f32 * scale,
        ))
        .italic_angle(0.0)
        .ascent(face.ascender() as f32 * scale)
        .descent(face.descender() as f32 * scale)
        .cap_height(face.capital_height().unwrap_or(face.ascender()) as f32 * scale)
        .stem_v(80.0)
        .font_file2(file);

    let compressed = deflate(data);
    w.pdf
        .stream(file, &compressed)
        .filter(Filter::FlateDecode)
        .pair(Name(b"Length1"), data.len() as i32);

    let mut cmap = pdf_writer::types::UnicodeCmap::new(
        Name(b"Custom"),
        SystemInfo {
            registry: Str(b"Adobe"),
            ordering: Str(b"UCS"),
            supplement: 0,
        },
    );
    for (&ch, &(gid, _)) in &glyphs {
        cmap.pair(gid, ch);
    }
    w.pdf.cmap(to_unicode, &cmap.finish());

    (type0, FontInfo { glyphs })
}

/// Breaks `text` into lines no wider than `max_width` (in units of the font size).
fn wrap(font: &FontInfo, text: &str, max_width: f32) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        let candidate = if line.is_empty() { word.to_string() } else { format!("{line} {word}") };
        if font.width(&candidate) <= max_width || line.is_empty() {
            line = candidate;
        } else {
            lines.push(std::mem::take(&mut line));
            line = word.to_string();
        }
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines
}

/// A line box for text, in millimeters from the top left of the page
struct TextBox {
    left: f32,
    top: f32,
    width: f32,
    font_size: f32,
    bold: bool,
}

/// Draws centered lines starting at the top of the box.
fn centered_text(content: &mut Content, font: &FontInfo, text: &str, b: TextBox) {
    let TextBox {
        left,
        top,
        width,
        font_size: size_mm,
        bold,
    } = b;
    let size_pt = size_mm * PT_PER_MM;
    content.save_state();
    if bold {
        /* fake bold by also stroking the outline */
        content.set_text_rendering_mode(TextRenderingMode::FillStroke);
        content.set_line_width(size_pt * 0.04);
    }
    for (n, line) in wrap(font, text, width / size_mm).iter().enumerate() {
        let line_width = font.width(line) * size_mm;
        let x = left + (width - line_width) / 2.0;
        /* baseline: one ascent below the top of the line */
        let baseline = top + size_mm * (0.85 + 1.15 * n as f32);
        let encoded: Vec<u8> = line
            .chars()
            .filter_map(|c| font.glyph(c).or_else(|| font.glyph('?')))
            .flat_map(|(gid, _)| gid.to_be_bytes())
            .collect();
        content.begin_text();
        content.set_font(Name(b"F1"), size_pt);
        content.next_line(x * PT_PER_MM, y_pt(baseline));
        content.show(Str(&encoded));
        content.end_text();
    }
    content.restore_state();
}

/// Backsides with the series name and the model number, laid out like the covers.
pub fn backsides_pdf(layout: &TagLayout, backs: &[Backside]) -> Vec<u8> {
    let texts: Vec<&str> = backs.iter().flat_map(|b| [b.series.as_str(), b.model.as_str()]).collect();
    let s = layout.size;
    build(
        layout,
        backs.len(),
        |w| {
            let (font, info) = embed_font(w, &texts);
            Resources {
                font: Some(font),
                font_data: Some(info),
                ..Default::default()
            }
        },
        |content, i, x, y, res| {
            let font = res.font_data.as_ref().unwrap();
            /* positions of the original, made for 40 mm tags, scaled to the tag size */
            let series = TextBox {
                left: x + 1.0,
                top: y + s * 15.0 / 40.0,
                width: s - 2.0,
                font_size: s * 3.5 / 40.0,
                bold: true,
            };
            centered_text(content, font, &backs[i].series, series);
            let model = TextBox {
                left: x,
                top: y + s * 33.0 / 40.0,
                width: s,
                font_size: s * 3.0 / 40.0,
                bold: false,
            };
            centered_text(content, font, &backs[i].model, model);
        },
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TagSide {
    Cover,
    Backside,
}

/// Writes the cover or backside PDF for `tonies` (tonies.json entry and audio hash) to `out_file`.
pub fn create(
    side: TagSide,
    tonies: &[(TonieData, String)],
    layout: &TagLayout,
    data_dir: &DataDir,
    out_file: &std::path::Path,
    status: &JobStatus,
) -> Result<(), String> {
    let pdf = match side {
        TagSide::Cover => {
            let mut images = Vec::new();
            for (info, hash) in tonies {
                if status.cancelled() {
                    return Err("cancelled".into());
                }
                status.set_current(info.title());
                let image = match info.pic.as_deref().filter(|p| !p.is_empty()) {
                    Some(pic) => cover_for_print(data_dir, pic, hash)
                        .map_err(|e| status.log(format!("No cover for '{}': {e}", info.title())))
                        .ok(),
                    None => {
                        status.log(format!("No cover URL for '{}'", info.title()));
                        None
                    }
                };
                images.push(image);
                status.finished_items.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }
            covers_pdf(layout, &images)
        }
        TagSide::Backside => {
            let backs: Vec<Backside> = tonies
                .iter()
                .map(|(info, _)| Backside {
                    series: info.series.clone().unwrap_or_default(),
                    model: info.model.clone().unwrap_or_default(),
                })
                .collect();
            status.finished_items.store(tonies.len(), std::sync::atomic::Ordering::Relaxed);
            backsides_pdf(layout, &backs)
        }
    };
    std::fs::write(out_file, pdf).map_err(|e| format!("Cannot write '{}': {e}", out_file.display()))?;
    status.log(format!("Written {}", out_file.display()));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layout() -> TagLayout {
        TagLayout {
            margin: 15.0,
            gap_x: 5.0,
            gap_y: 5.0,
            initial_column: 0,
            initial_row: 0,
            size: 40.0,
            outline_width: 0.5,
        }
    }

    #[test]
    fn positions_fill_rows_and_pages() {
        /* (210 - 2 * 15 + 5) / 45 = 4 columns, (297 - 30 + 5) / 45 = 6 rows */
        let p = layout().positions(25);
        assert_eq!(p[0], (0, 15.0, 15.0));
        assert_eq!(p[3], (0, 150.0, 15.0));
        assert_eq!(p[4], (0, 15.0, 60.0));
        assert_eq!(p[23], (0, 150.0, 240.0));
        assert_eq!(p[24], (1, 15.0, 15.0));
    }

    #[test]
    fn positions_start_on_a_partly_used_sheet() {
        let l = TagLayout {
            initial_column: 2,
            initial_row: 5,
            ..layout()
        };
        let p = l.positions(4);
        assert_eq!(p[0], (0, 105.0, 240.0));
        assert_eq!(p[1], (0, 150.0, 240.0));
        assert_eq!(p[2], (1, 15.0, 15.0));
    }

    #[test]
    fn writes_valid_pdfs() {
        let img = RgbaImage::from_pixel(64, 64, image::Rgba([200, 30, 30, 255]));
        let pdf = covers_pdf(&layout(), &[Some(img.clone()), None, Some(img)]);
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 1"));

        let backs: Vec<Backside> = (0..30)
            .map(|i| Backside {
                series: format!("Die Sendung mit der Maus {i}"),
                model: "10000123".into(),
            })
            .collect();
        let pdf = backsides_pdf(&layout(), &backs);
        assert!(pdf.starts_with(b"%PDF-"));
        assert!(String::from_utf8_lossy(&pdf).contains("/Count 2"));
    }

    #[test]
    fn wraps_long_series_names() {
        let mut w = Writer::new();
        let (_, font) = embed_font(&mut w, &["Die Sendung mit der Maus"]);
        let lines = wrap(&font, "Die Sendung mit der Maus", 38.0 / 3.5);
        assert!(lines.len() >= 2, "{lines:?}");
        assert!(lines.iter().all(|l| font.width(l) * 3.5 <= 38.0), "{lines:?}");
    }
}
