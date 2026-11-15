//! Paints a [`DocumentLayout`] into a PDF with krilla.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use krilla::action::{Action, LinkAction};
use krilla::annotation::{Annotation, LinkAnnotation, Target};
use krilla::color::rgb;
use krilla::geom::{PathBuilder, Point, Rect, Size, Transform};
use krilla::image::Image;
use krilla::metadata::Metadata;
use krilla::page::PageSettings;
use krilla::paint::{Fill, LineJoin, Paint, Stroke};
use krilla::surface::Surface;
use krilla::text::{Font, GlyphId, KrillaGlyph};
use krilla::Document;
use parley::FontData;

use crate::images::Images;
use crate::layout::{DocumentLayout, Page, ParagraphLayout, PlacedLine};
use crate::{Error, Rgb};

pub(crate) fn paint(
    document: &DocumentLayout,
    images: &mut Images<'_>,
    title: Option<&str>,
) -> Result<Vec<u8>, Error> {
    let mut fonts = FontCache::default();
    let mut image_cache = ImageCache {
        images,
        embedded: HashMap::new(),
    };
    let mut pdf = Document::new();
    if let Some(title) = title {
        pdf.set_metadata(Metadata::new().title(title.to_string()));
    }

    for page_layout in &document.pages {
        let style = &page_layout.style;
        let size = Size::from_wh(style.width, style.height).ok_or(Error::NoContentArea)?;
        let mut page = pdf.start_page_with(PageSettings::new(size));
        let mut surface = page.surface();
        paint_background(&mut surface, page_layout, &mut image_cache)?;
        for placed in &page_layout.images {
            let image = image_cache.get(&placed.src)?;
            let size = Size::from_wh(placed.width, placed.height)
                .ok_or(Error::InvalidImage(placed.src.clone()))?;
            surface.push_transform(&Transform::from_translate(placed.left, placed.top));
            surface.draw_image(image, size);
            surface.pop();
        }
        for placed in &page_layout.lines {
            let paragraph = &document.paragraphs[placed.paragraph];
            for text_run in text_runs(paragraph, placed) {
                let font = fonts.get(&text_run.font)?;
                text_run.draw(&mut surface, font, &paragraph.text);
            }
        }
        surface.finish();

        for area in &page_layout.links {
            let Some(rect) = Rect::from_ltrb(area.left, area.top, area.right, area.bottom) else {
                continue;
            };
            let target = Target::Action(Action::Link(LinkAction::new(area.href.clone())));
            page.add_annotation(Annotation::new_link(
                LinkAnnotation::new(rect, target),
                None,
            ));
        }
        page.finish();
    }

    pdf.finish().map_err(|e| Error::Pdf(format!("{e:?}")))
}

/// The page background, across the whole page including its margins: the colour, then the
/// image scaled to cover the page, anchored top left (CSS `background-position: 0 0`).
fn paint_background(
    surface: &mut Surface<'_>,
    page: &Page,
    images: &mut ImageCache<'_, '_>,
) -> Result<(), Error> {
    let style = &page.style;
    if let Some(color) = style.background_color {
        fill_rect(surface, 0.0, 0.0, style.width, style.height, color);
    }
    if let Some(src) = &style.background_image {
        let image = images.get(src)?;
        let (width, height) = image.size();
        let scale = (style.width / width as f32).max(style.height / height as f32);
        let size = Size::from_wh(width as f32 * scale, height as f32 * scale)
            .ok_or(Error::InvalidImage(src.clone()))?;
        surface.draw_image(image, size);
    }
    Ok(())
}

fn fill_rect(surface: &mut Surface<'_>, left: f32, top: f32, width: f32, height: f32, color: Rgb) {
    let mut path = PathBuilder::new();
    let Some(rect) = Rect::from_xywh(left, top, width, height) else {
        return;
    };
    path.push_rect(rect);
    let Some(path) = path.finish() else { return };
    surface.set_fill(Some(fill(color)));
    surface.draw_path(&path);
}

fn paint_of(color: Rgb) -> Paint {
    let Rgb(r, g, b) = color;
    rgb::Color::new(r, g, b).into()
}

fn fill(color: Rgb) -> Fill {
    Fill {
        paint: paint_of(color),
        ..Fill::default()
    }
}

/// Glyphs drawn with one call: same font, size and colour, laid out left to right from `origin`.
pub(crate) struct TextRun {
    pub origin: Point,
    pub font: FontData,
    pub font_size: f32,
    pub colour: Rgb,
    /// (offset below the baseline, thickness, width) when underlined.
    pub underline: Option<(f32, f32, f32)>,
    /// fontique found no face of the family close enough to the weight asked for: the nearest is
    /// more than 200 lighter (`bold` on a regular-only family, but not `600`), so the glyphs are
    /// thickened with a stroke, as browsers synthesize bold (`font-synthesis: weight`).
    pub embolden: bool,
    /// Italic or oblique was asked for and the family has no such face, so the glyphs are
    /// slanted by this many degrees, as browsers synthesize italics (`font-synthesis: style`).
    pub skew: Option<f32>,
    pub glyphs: Vec<KrillaGlyph>,
}

impl TextRun {
    /// Stroke width of a synthesized bold, as a fraction of the font size.
    pub(crate) const EMBOLDEN: f32 = 1.0 / 24.0;

    fn draw(&self, surface: &mut Surface<'_>, font: Font, text: &str) {
        surface.set_fill(Some(fill(self.colour)));
        if self.embolden {
            surface.set_stroke(Some(Stroke {
                paint: paint_of(self.colour),
                width: self.font_size * Self::EMBOLDEN,
                line_join: LineJoin::Round,
                ..Stroke::default()
            }));
        }
        if let Some(degrees) = self.skew {
            // Slant around the baseline: a point `h` above it moves `h·tan` to the right.
            let tan = degrees.to_radians().tan();
            surface.push_transform(&Transform::from_row(
                1.0,
                0.0,
                -tan,
                1.0,
                tan * self.origin.y,
                0.0,
            ));
        }
        surface.draw_glyphs(self.origin, &self.glyphs, font, text, self.font_size, false);
        if self.skew.is_some() {
            surface.pop();
        }
        surface.set_stroke(None);
        if let Some((offset, thickness, width)) = self.underline {
            fill_rect(
                surface,
                self.origin.x,
                self.origin.y + offset,
                width,
                thickness,
                self.colour,
            );
        }
    }
}

/// The text runs for one placed line, in page space. A new run starts at every parley run (font
/// or size change) and at every colour or underline change within one.
pub(crate) fn text_runs(paragraph: &ParagraphLayout, placed: &PlacedLine) -> Vec<TextRun> {
    let line = paragraph
        .layout
        .lines()
        .nth(placed.line)
        .expect("placed line exists");
    let baseline = placed.dy + line.metrics().baseline;
    let mut x = placed.left + line.metrics().offset;
    let mut runs = Vec::new();

    for run in line.runs() {
        let font_size = run.font_size();
        let metrics = run.font_metrics();
        // parley's underline offset is y-up from the baseline (negative below it); the top of
        // the stroke sits at that offset.
        let (offset, thickness) = (-metrics.underline_offset, metrics.underline_size.max(0.5));
        let embolden = run.synthesis().embolden();
        let skew = run.synthesis().skew();
        let clusters: Vec<_> = run.visual_clusters().collect();
        let mut current: Option<TextRun> = None;

        for (index, cluster) in clusters.iter().enumerate() {
            let brush = cluster.style().brush;
            if current.as_ref().is_some_and(|run| {
                run.colour != brush.color || run.underline.is_some() != brush.underline
            }) {
                runs.extend(current.take());
            }
            let text_run = current.get_or_insert_with(|| TextRun {
                origin: Point::from_xy(x, baseline),
                font: run.font().clone(),
                font_size,
                colour: brush.color,
                underline: brush.underline.then_some((offset, thickness, 0.0)),
                embolden,
                skew,
                glyphs: Vec::new(),
            });
            let text_range = glyph_text_range(&clusters, index);
            for glyph in cluster.glyphs() {
                text_run.glyphs.push(KrillaGlyph {
                    glyph_id: GlyphId::new(glyph.id),
                    text_range: text_range.clone(),
                    x_advance: glyph.advance / font_size,
                    x_offset: glyph.x / font_size,
                    y_offset: glyph.y / font_size,
                    y_advance: 0.0,
                    location: None,
                });
            }
            // Trailing whitespace at a line end isn't underlined.
            if let Some((_, _, width)) = &mut text_run.underline {
                if !paragraph.text[cluster.text_range()].trim().is_empty() {
                    *width = x + cluster.advance() - text_run.origin.x;
                }
            }
            x += cluster.advance();
        }
        runs.extend(current);
    }
    runs
}

/// The text a cluster's glyphs stand for. A ligature glyph ("fi") sits in its first cluster and
/// the clusters it absorbed carry no glyphs, so its range must span them too, or the absorbed
/// letters vanish from the PDF's extractable text.
fn glyph_text_range<B: parley::Brush>(
    clusters: &[parley::Cluster<'_, B>],
    index: usize,
) -> Range<usize> {
    let start = clusters[index].text_range().start;
    let end = clusters[index + 1..]
        .iter()
        .take_while(|next| next.is_ligature_continuation())
        .last()
        .map_or(clusters[index].text_range().end, |last| {
            last.text_range().end
        });
    start..end
}

/// krilla fonts keyed by the parley font blob they came from, so each font is embedded once.
#[derive(Default)]
struct FontCache {
    fonts: HashMap<(u64, u32), Font>,
}

impl FontCache {
    fn get(&mut self, data: &FontData) -> Result<Font, Error> {
        let key = (data.data.id(), data.index);
        if let Some(font) = self.fonts.get(&key) {
            return Ok(font.clone());
        }
        let bytes: Arc<dyn AsRef<[u8]> + Send + Sync> = Arc::new(data.data.clone());
        let font = Font::new(bytes.into(), data.index).ok_or(Error::InvalidFont)?;
        self.fonts.insert(key, font.clone());
        Ok(font)
    }
}

/// krilla images by reference, so an image used on several pages is embedded once.
struct ImageCache<'a, 'b> {
    images: &'a mut Images<'b>,
    embedded: HashMap<String, Image>,
}

impl ImageCache<'_, '_> {
    fn get(&mut self, src: &str) -> Result<Image, Error> {
        if let Some(image) = self.embedded.get(src) {
            return Ok(image.clone());
        }
        let bytes = self.images.get(src)?.bytes.clone();
        let data: Arc<dyn AsRef<[u8]> + Send + Sync> = bytes;
        let image =
            Image::from_png(data.into(), true).map_err(|_| Error::InvalidImage(src.to_string()))?;
        self.embedded.insert(src.to_string(), image.clone());
        Ok(image)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout::tests::{fixture, font, font_file, lay_out, lay_out_with_fonts};

    const FACES: [&str; 4] = ["Regular", "Bold", "Italic", "BoldItalic"];

    fn faces(names: &[&str]) -> Vec<Vec<u8>> {
        names
            .iter()
            .map(|name| match *name {
                "Regular" => font(),
                name => font_file(&format!("DejaVuSerif-{name}.ttf")),
            })
            .collect()
    }

    /// (text, face it is drawn with, emboldened, slanted) for every run of a fixture laid out
    /// with the given faces.
    fn face_runs(
        fixture_name: &str,
        registered: &[&str],
    ) -> Vec<(String, &'static str, bool, bool)> {
        let all = faces(&FACES);
        let document = lay_out_with_fonts(&fixture(fixture_name), &[], &faces(registered));
        document
            .pages
            .iter()
            .flat_map(|page| &page.lines)
            .flat_map(|placed| {
                let paragraph = &document.paragraphs[placed.paragraph];
                text_runs(paragraph, placed).into_iter().map(|run| {
                    let start = run.glyphs.first().unwrap().text_range.start;
                    let end = run.glyphs.last().unwrap().text_range.end;
                    let text = paragraph.text[start..end].trim().to_string();
                    let face = FACES[all
                        .iter()
                        .position(|bytes| run.font.data.as_ref() == bytes.as_slice())
                        .expect("a registered face")];
                    (text, face, run.embolden, run.skew.is_some())
                })
            })
            .filter(|(text, ..)| !text.is_empty())
            .collect()
    }

    fn as_strs<'a>(
        runs: &'a [(String, &'static str, bool, bool)],
    ) -> Vec<(&'a str, &'a str, bool, bool)> {
        runs.iter()
            .map(|(t, f, e, s)| (t.as_str(), *f, *e, *s))
            .collect()
    }

    #[test]
    fn bold_text_uses_the_bold_face_when_it_is_registered() {
        let runs = face_runs("bold.html", &["Regular", "Bold"]);
        assert_eq!(
            as_strs(&runs),
            [
                ("A bold heading", "Bold", false, false),
                ("Plain text with", "Regular", false, false),
                ("bold", "Bold", false, false),
                ("and", "Regular", false, false),
                ("strong", "Bold", false, false),
                ("words.", "Regular", false, false),
            ]
        );
    }

    #[test]
    fn bold_text_is_synthesized_without_a_bold_face() {
        let runs = face_runs("bold.html", &["Regular"]);
        assert_eq!(
            as_strs(&runs),
            [
                ("A bold heading", "Regular", true, false),
                ("Plain text with", "Regular", false, false),
                ("bold", "Regular", true, false),
                ("and", "Regular", false, false),
                ("strong", "Regular", true, false),
                ("words.", "Regular", false, false),
            ]
        );
    }

    #[test]
    fn italic_text_uses_the_italic_faces_when_they_are_registered() {
        let runs = face_runs("italic.html", &FACES);
        assert_eq!(
            as_strs(&runs),
            [
                ("Plain", "Regular", false, false),
                ("emphasis", "Italic", false, false),
                ("and", "Regular", false, false),
                ("italic", "Italic", false, false),
                ("and", "Regular", false, false),
                ("bold italic", "BoldItalic", false, false),
                ("with an", "Regular", false, false),
                ("underlined", "Italic", false, false),
                ("link.", "Regular", false, false),
            ]
        );
    }

    #[test]
    fn italic_text_is_synthesized_without_italic_faces() {
        let runs = face_runs("italic.html", &["Regular", "Bold"]);
        assert_eq!(
            as_strs(&runs),
            [
                ("Plain", "Regular", false, false),
                ("emphasis", "Regular", false, true),
                ("and", "Regular", false, false),
                ("italic", "Regular", false, true),
                ("and", "Regular", false, false),
                ("bold italic", "Bold", false, true),
                ("with an", "Regular", false, false),
                ("underlined", "Regular", false, true),
                ("link.", "Regular", false, false),
            ]
        );
        let runs = face_runs("italic.html", &["Regular"]);
        assert!(
            runs.iter()
                .any(|(t, _, embolden, skewed)| t == "bold italic" && *embolden && *skewed),
            "bold italic with the regular face only is both emboldened and slanted"
        );
    }

    #[test]
    fn a_synthesized_italic_keeps_its_underline_straight_under_the_text() {
        let document = lay_out_with_fonts(&fixture("italic.html"), &[], &faces(&["Regular"]));
        let page = &document.pages[0];
        let area = &page.links[0];
        let placed = page.lines.iter().find(|l| l.top == area.top).unwrap();
        let runs = text_runs(&document.paragraphs[placed.paragraph], placed);
        let link = runs.iter().find(|r| r.underline.is_some()).unwrap();
        assert!(link.skew.is_some());
        let (_, _, width) = link.underline.unwrap();
        assert!((link.origin.x - area.left).abs() < 0.01);
        assert!((link.origin.x + width - area.right).abs() < 0.01);
    }

    /// Every byte of every line's text must be claimed by some glyph, in order, with no gaps or
    /// overlaps, so PDF text extraction reproduces the source text exactly.
    #[test]
    fn glyph_text_ranges_cover_each_line_exactly() {
        let document = lay_out(&fixture("paragraphs.html"));
        let mut ligatures = 0;

        for placed in document.pages.iter().flat_map(|page| &page.lines) {
            let paragraph = &document.paragraphs[placed.paragraph];
            let line_range = paragraph
                .layout
                .lines()
                .nth(placed.line)
                .unwrap()
                .text_range();
            let mut ranges: Vec<Range<usize>> = text_runs(paragraph, placed)
                .iter()
                .flat_map(|run| run.glyphs.iter().map(|g| g.text_range.clone()))
                .collect();
            ranges.dedup();
            ligatures += ranges
                .iter()
                .filter(|r| paragraph.text[(*r).clone()].chars().count() > 1)
                .count();

            let mut covered = line_range.start;
            for range in &ranges {
                assert_eq!(
                    range.start,
                    covered,
                    "gap or overlap in line {:?}",
                    &paragraph.text[line_range.clone()]
                );
                covered = range.end;
            }
            assert_eq!(covered, line_range.end, "line end not covered");
        }
        assert!(
            ligatures > 0,
            "the fixture should exercise at least one ligature"
        );
    }

    /// The PDF places glyphs by their advances alone, so those must carry the justified spacing:
    /// runs meet end to end, and a stretched line's last letter lands on the right margin.
    #[test]
    fn justified_glyphs_reach_the_right_margin_in_the_pdf() {
        let document = lay_out(&fixture("text-align.html"));
        let right_margin = 419.53 - 40.0;
        let mut at_margin = 0;
        for placed in &document.pages[0].lines {
            let paragraph = &document.paragraphs[placed.paragraph];
            let runs = text_runs(paragraph, placed);
            let mut last_ink = 0.0;
            for (index, run) in runs.iter().enumerate() {
                let mut x = run.origin.x;
                for glyph in &run.glyphs {
                    x += glyph.x_advance * run.font_size;
                    if !paragraph.text[glyph.text_range.clone()].trim().is_empty() {
                        last_ink = x;
                    }
                }
                if let Some(next) = runs.get(index + 1) {
                    assert!((x - next.origin.x).abs() < 0.01, "runs meet end to end");
                }
            }
            if (last_ink - right_margin).abs() < 0.01 {
                at_margin += 1;
            }
        }
        // Two right-aligned lines plus the stretched lines of the justified paragraphs.
        assert!(at_margin >= 6, "{at_margin} lines end at the right margin");
    }

    #[test]
    fn a_line_break_draws_no_glyph() {
        let document = lay_out("<p>one<br>two</p>");
        let paragraph = &document.paragraphs[0];
        let drawn: Vec<String> = document.pages[0]
            .lines
            .iter()
            .flat_map(|placed| text_runs(paragraph, placed))
            .flat_map(|run| run.glyphs)
            .map(|glyph| paragraph.text[glyph.text_range].to_string())
            .collect();
        assert_eq!(drawn, ["o", "n", "e", "t", "w", "o"]);
    }

    #[test]
    fn underlines_span_the_visible_link_text_only() {
        let document = lay_out("<style>body { margin: 0 }</style><p>Read <a href=\"https://x.test\">the link</a> now.</p>");
        let placed = &document.pages[0].lines[0];
        let runs = text_runs(&document.paragraphs[0], placed);
        let underlined: Vec<_> = runs.iter().filter(|r| r.underline.is_some()).collect();
        assert_eq!(underlined.len(), 1);
        let area = &document.pages[0].links[0];
        let (offset, thickness, width) = underlined[0].underline.unwrap();
        assert!((underlined[0].origin.x - area.left).abs() < 0.01);
        assert!((underlined[0].origin.x + width - area.right).abs() < 0.01);
        assert!(
            offset > 0.0 && thickness > 0.0,
            "the underline sits below the baseline"
        );
    }
}
