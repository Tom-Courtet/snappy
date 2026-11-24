//! Écriture du PDF : pages mises en page par [`super::layout`], images compressées
//! (Flate, transparence en masque doux), police Outfit embarquée en entier (CID,
//! Identity-H) avec sa table Unicode pour copier et chercher le texte.

use std::collections::BTreeMap;

use anyhow::Result;
use image::RgbaImage;
use pdf_writer::types::{CidFontType, FontFlags, SystemInfo, UnicodeCmap};
use pdf_writer::{Content, Filter, Finish, Name, Pdf, Rect, Ref, Str, TextStr};

use super::layout::{self, Face, ITALIC_SKEW, Metrics, OUTFIT, OUTFIT_SEMIBOLD, Op};
use super::{DocItem, DocOptions};

const SYSTEM_INFO: SystemInfo = SystemInfo {
    registry: Str(b"Adobe"),
    ordering: Str(b"Identity"),
    supplement: 0,
};

/// Couleur du texte courant et des titres (violet de Snappy).
const INK: (f32, f32, f32) = (0.11, 0.11, 0.13);
const ACCENT: (f32, f32, f32) = (
    0x7f as f32 / 255.0,
    0x3a as f32 / 255.0,
    0xed as f32 / 255.0,
);

/// Distributeur de numéros d'objets.
struct Ids(i32);

impl Ids {
    fn next(&mut self) -> Ref {
        self.0 += 1;
        Ref::new(self.0)
    }
}

pub fn write(
    items: &[DocItem],
    images: &[RgbaImage],
    options: &DocOptions,
    title: &str,
) -> Result<Vec<u8>> {
    let metrics = Metrics::new();
    let sizes: Vec<_> = images.iter().map(RgbaImage::dimensions).collect();
    let pages = layout::layout(items, &sizes, options, &metrics);

    let mut ids = Ids(0);
    let catalog = ids.next();
    let tree = ids.next();
    let info = ids.next();
    let fonts = [(Face::Regular, ids.next()), (Face::Bold, ids.next())];
    let image_ids: Vec<Ref> = images.iter().map(|_| ids.next()).collect();

    let mut pdf = Pdf::new();
    let mut page_ids = Vec::new();
    // Glyphes utilisés, par graisse : identifiant → caractère (table Unicode, largeurs).
    let mut used: BTreeMap<Face, BTreeMap<u16, char>> = BTreeMap::new();

    for page in &pages {
        let page_id = ids.next();
        let content_id = ids.next();
        page_ids.push(page_id);
        let mut content = Content::new();
        for op in &page.ops {
            match op {
                Op::Text(span) => {
                    let font = metrics.font(span.face);
                    let glyphs = used.entry(span.face).or_default();
                    let mut bytes = Vec::with_capacity(span.text.len() * 2);
                    for c in span.text.chars() {
                        let gid = ab_glyph::Font::glyph_id(font, c).0;
                        glyphs.entry(gid).or_insert(c);
                        bytes.extend_from_slice(&gid.to_be_bytes());
                    }
                    let (r, g, b) = if span.accent { ACCENT } else { INK };
                    let y = page.height - span.y;
                    let skew = if span.italic { ITALIC_SKEW } else { 0.0 };
                    content.set_fill_rgb(r, g, b);
                    content
                        .begin_text()
                        .set_font(font_name(span.face), span.size)
                        .set_text_matrix([1.0, 0.0, skew, 1.0, span.x, y])
                        .show(Str(&bytes))
                        .end_text();
                    if span.underline {
                        let under = y - span.size * 0.15;
                        content
                            .set_stroke_rgb(r, g, b)
                            .set_line_width(span.size * 0.06)
                            .move_to(span.x, under)
                            .line_to(span.x + span.width, under)
                            .stroke();
                    }
                }
                Op::Image { index, x, y, w, h } => {
                    let bottom = page.height - y - h;
                    content
                        .save_state()
                        .transform([*w, 0.0, 0.0, *h, *x, bottom])
                        .x_object(image_name(*index).as_name())
                        .restore_state();
                    // Filet gris clair : les captures à fond blanc restent délimitées.
                    content
                        .set_stroke_gray(0.82)
                        .set_line_width(0.5)
                        .rect(*x, bottom, *w, *h)
                        .stroke();
                }
            }
        }
        pdf.stream(content_id, &content.finish());
        let mut p = pdf.page(page_id);
        p.parent(tree)
            .media_box(Rect::new(0.0, 0.0, page.width, page.height))
            .contents(content_id);
        let mut resources = p.resources();
        {
            let mut x_objects = resources.x_objects();
            for (index, id) in image_ids.iter().enumerate() {
                x_objects.pair(image_name(index).as_name(), *id);
            }
        }
        {
            let mut font_dict = resources.fonts();
            for (face, id) in fonts {
                font_dict.pair(font_name(face), id);
            }
        }
        resources.finish();
        p.finish();
    }

    pdf.catalog(catalog).pages(tree);
    pdf.pages(tree)
        .kids(page_ids.iter().copied())
        .count(page_ids.len() as i32);
    pdf.document_info(info)
        .title(TextStr(title))
        .creator(TextStr("Snappy"))
        .producer(TextStr("Snappy"));

    for (img, id) in images.iter().zip(&image_ids) {
        write_image(&mut pdf, &mut ids, *id, img);
    }
    for (face, id) in fonts {
        let glyphs = used.remove(&face).unwrap_or_default();
        write_font(&mut pdf, &mut ids, id, face, &glyphs, &metrics);
    }
    Ok(pdf.finish())
}

fn font_name(face: Face) -> Name<'static> {
    match face {
        Face::Regular => Name(b"F1"),
        Face::Bold => Name(b"F2"),
    }
}

struct ImageName(String);

impl ImageName {
    fn as_name(&self) -> Name<'_> {
        Name(self.0.as_bytes())
    }
}

fn image_name(index: usize) -> ImageName {
    ImageName(format!("Im{index}"))
}

fn deflate(bytes: &[u8]) -> Vec<u8> {
    miniz_oxide::deflate::compress_to_vec_zlib(bytes, 6)
}

/// Image RVB, plus un masque doux s'il y a de la transparence.
fn write_image(pdf: &mut Pdf, ids: &mut Ids, id: Ref, img: &RgbaImage) {
    let (w, h) = img.dimensions();
    let rgb: Vec<u8> = img
        .pixels()
        .flat_map(|p| [p.0[0], p.0[1], p.0[2]])
        .collect();
    let has_alpha = img.pixels().any(|p| p.0[3] < 255);
    let mask_id = has_alpha.then(|| ids.next());
    let data = deflate(&rgb);
    let mut x = pdf.image_xobject(id, &data);
    x.filter(Filter::FlateDecode);
    x.width(w as i32);
    x.height(h as i32);
    x.color_space().device_rgb();
    x.bits_per_component(8);
    if let Some(mask) = mask_id {
        x.s_mask(mask);
    }
    x.finish();
    if let Some(mask) = mask_id {
        let alpha: Vec<u8> = img.pixels().map(|p| p.0[3]).collect();
        let data = deflate(&alpha);
        let mut m = pdf.image_xobject(mask, &data);
        m.filter(Filter::FlateDecode);
        m.width(w as i32);
        m.height(h as i32);
        m.color_space().device_gray();
        m.bits_per_component(8);
        m.finish();
    }
}

/// Police Type 0 (CID, Identity-H) : le texte est écrit en identifiants de glyphes.
fn write_font(
    pdf: &mut Pdf,
    ids: &mut Ids,
    id: Ref,
    face: Face,
    glyphs: &BTreeMap<u16, char>,
    metrics: &Metrics,
) {
    let (base, data): (Name, &[u8]) = match face {
        Face::Regular => (Name(b"Outfit-Regular"), OUTFIT),
        Face::Bold => (Name(b"Outfit-SemiBold"), OUTFIT_SEMIBOLD),
    };
    let cid = ids.next();
    let descriptor = ids.next();
    let file = ids.next();
    let cmap_id = ids.next();
    pdf.type0_font(id)
        .base_font(base)
        .encoding_predefined(Name(b"Identity-H"))
        .descendant_font(cid)
        .to_unicode(cmap_id);

    let font = metrics.font(face);
    let upem = ab_glyph::Font::units_per_em(font).unwrap_or(1000.0);
    let to_1000 = |v: f32| v * 1000.0 / upem;
    let mut cid_font = pdf.cid_font(cid);
    cid_font
        .subtype(CidFontType::Type2)
        .base_font(base)
        .system_info(SYSTEM_INFO)
        .font_descriptor(descriptor)
        .default_width(0.0)
        .cid_to_gid_map_predefined(Name(b"Identity"));
    {
        let mut widths = cid_font.widths();
        for &gid in glyphs.keys() {
            let w = ab_glyph::Font::h_advance_unscaled(font, ab_glyph::GlyphId(gid));
            widths.consecutive(gid, [to_1000(w)]);
        }
    }
    cid_font.finish();

    let ascent = to_1000(ab_glyph::Font::ascent_unscaled(font));
    let descent = to_1000(ab_glyph::Font::descent_unscaled(font));
    pdf.font_descriptor(descriptor)
        .name(base)
        .flags(FontFlags::NON_SYMBOLIC)
        .bbox(Rect::new(-200.0, descent, 1200.0, ascent))
        .italic_angle(0.0)
        .ascent(ascent)
        .descent(descent)
        .cap_height(700.0)
        .stem_v(if face == Face::Bold { 120.0 } else { 80.0 })
        .font_file2(file);

    let compressed = deflate(data);
    pdf.stream(file, &compressed)
        .filter(Filter::FlateDecode)
        .pair(Name(b"Length1"), data.len() as i32);

    let mut cmap = UnicodeCmap::new(Name(b"Snappy-UCS"), SYSTEM_INFO);
    for (&gid, &c) in glyphs {
        cmap.pair(gid, c);
    }
    let cmap = cmap.finish();
    pdf.cmap(cmap_id, &cmap);
}
