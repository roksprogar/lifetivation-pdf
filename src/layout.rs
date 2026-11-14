//! Block flow layout, paginated.
//!
//! Blocks stack vertically with collapsing margins. Paragraphs are shaped once by parley, then
//! broken one line at a time, each at the width available on the page it is about to land on: a
//! line that doesn't fit the page's remaining height is undone and broken again on the next page.
//! A box whose `page` name differs from the current page's starts a new page with that name's
//! `@page` geometry and background.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Arc;

use parley::fontique::{Blob, Collection, CollectionOptions, SourceCache};
use parley::{
    Alignment, AlignmentOptions, FontContext, FontFamily, FontFamilyName, FontWeight, Layout,
    LayoutContext, Line, StyleProperty, YieldData,
};

use crate::css::{self, Length, Stylesheet};
use crate::dom::{Block, Link, Paragraph};
use crate::images::Images;
use crate::style::{self, LineHeight, TextStyle};
use crate::{Error, Rgb};

/// Per-span style carried through parley.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Brush {
    pub color: Rgb,
    pub underline: bool,
    pub link: Option<usize>,
}

pub(crate) struct ParagraphLayout {
    pub text: String,
    pub links: Vec<Link>,
    pub layout: Layout<Brush>,
}

/// Page size, margins and background from the `@page` rules for one page name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PageStyle {
    pub width: f32,
    pub height: f32,
    pub margin: style::Sides,
    pub background_color: Option<Rgb>,
    /// `background-image` reference; always painted as `background-size: cover`.
    pub background_image: Option<String>,
}

impl PageStyle {
    fn resolve(sheets: &[&Stylesheet], name: Option<&str>) -> Result<Self, Error> {
        let mut specified = css::Specified::new();
        for sheet in sheets {
            // `@page` applies to every page, `@page name` on top of it to pages of that name.
            let unnamed = sheet.pages.iter().filter(|rule| rule.name.is_none());
            let named = sheet
                .pages
                .iter()
                .filter(|rule| rule.name.is_some() && rule.name.as_deref() == name);
            for rule in unnamed.chain(named) {
                for declaration in &rule.declarations {
                    css::expand(declaration, &mut specified);
                }
            }
        }
        let get = |property: &str| specified.get(property).map(String::as_str);
        let (width, height) = page_size(get("size").unwrap_or("A4"))?;
        let margin =
            |side: &str| match get(&format!("margin-{side}")).and_then(|v| css::length(v, 12.0)) {
                Some(length) => style::resolve(length, width),
                None => 0.0,
            };
        let background_image = get("background-image").filter(|v| *v != "none");
        if background_image.is_some() && get("background-size").unwrap_or("auto") != "cover" {
            return Err(Error::Unsupported(
                "page background images without background-size: cover".into(),
            ));
        }
        let page = PageStyle {
            width,
            height,
            margin: style::Sides {
                top: margin("top"),
                right: margin("right"),
                bottom: margin("bottom"),
                left: margin("left"),
            },
            background_color: get("background-color").and_then(css::color),
            background_image: background_image
                .map(|v| {
                    css::url(v).ok_or(Error::Unsupported(
                        "background-image values other than url()".into(),
                    ))
                })
                .transpose()?,
        };
        if page.content_width() <= 0.0 || page.content_bottom() <= page.margin.top {
            return Err(Error::NoContentArea);
        }
        Ok(page)
    }

    fn content_width(&self) -> f32 {
        self.width - self.margin.left - self.margin.right
    }

    fn content_bottom(&self) -> f32 {
        self.height - self.margin.bottom
    }
}

/// `A4`, `A5`, `letter`, `legal`, optionally with `portrait`/`landscape`, or two lengths.
fn page_size(value: &str) -> Result<(f32, f32), Error> {
    let words: Vec<String> = value
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect();
    let landscape = words.iter().any(|w| w == "landscape");
    let named = words.iter().find_map(|w| match w.as_str() {
        "a4" => Some((595.28, 841.89)),
        "a5" => Some((419.53, 595.28)),
        "letter" => Some((612.0, 792.0)),
        "legal" => Some((612.0, 1008.0)),
        "auto" | "portrait" | "landscape" => None,
        _ => None,
    });
    let lengths: Vec<f32> = words
        .iter()
        .filter_map(|w| match css::length(w, 12.0) {
            Some(Length::Points(points)) => Some(points),
            _ => None,
        })
        .collect();
    let (width, height) = match (named, lengths.as_slice()) {
        (Some(size), []) => size,
        (None, [side]) => (*side, *side),
        (None, [width, height]) => (*width, *height),
        (None, []) => (595.28, 841.89),
        _ => return Err(Error::Unsupported("this @page size value".into())),
    };
    Ok(if landscape {
        (width.max(height), width.min(height))
    } else {
        (width, height)
    })
}

/// One paragraph line placed on a page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PlacedLine {
    pub paragraph: usize,
    pub line: usize,
    /// Page-space x of the paragraph's content box left edge.
    pub left: f32,
    /// Page-space y of the line box's top edge.
    pub top: f32,
    /// Page-space y of the line box's bottom edge.
    pub bottom: f32,
    /// Add to a layout-space y to get the page-space y.
    pub dy: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PlacedImage {
    pub src: String,
    pub left: f32,
    pub top: f32,
    pub width: f32,
    pub height: f32,
}

/// A clickable area for one line fragment of a link, in page space (y down).
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct LinkArea {
    pub href: String,
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

pub(crate) struct Page {
    pub name: Option<String>,
    pub style: Arc<PageStyle>,
    pub lines: Vec<PlacedLine>,
    pub images: Vec<PlacedImage>,
    pub links: Vec<LinkArea>,
    /// Content (or an empty box) has started on this page, so a different page name breaks.
    used: bool,
}

pub(crate) struct DocumentLayout {
    pub paragraphs: Vec<ParagraphLayout>,
    pub pages: Vec<Page>,
}

/// A cluster positioned along its line, in layout space (x from the paragraph's left edge).
pub(crate) struct PositionedCluster {
    pub x: f32,
    pub advance: f32,
    pub text_range: Range<usize>,
    pub link: Option<usize>,
}

/// The fonts available to a document: registered from bytes only, never from the system, so
/// output doesn't depend on what the host has installed.
pub(crate) struct Fonts {
    context: FontContext,
    /// Registered family names, in registration order; the first is the default.
    families: Vec<String>,
}

impl Fonts {
    pub fn new(fonts: &[Vec<u8>]) -> Result<Self, Error> {
        let mut context = FontContext {
            collection: Collection::new(CollectionOptions {
                shared: false,
                system_fonts: false,
            }),
            source_cache: SourceCache::default(),
        };
        let mut families: Vec<String> = Vec::new();
        for font in fonts {
            let registered = context
                .collection
                .register_fonts(Blob::new(Arc::new(font.clone())), None);
            let (family_id, _) = registered.into_iter().next().ok_or(Error::InvalidFont)?;
            let name = context
                .collection
                .family_name(family_id)
                .ok_or(Error::InvalidFont)?
                .to_owned();
            if !families.contains(&name) {
                families.push(name);
            }
        }
        if families.is_empty() {
            return Err(Error::InvalidFont);
        }
        Ok(Self { context, families })
    }

    pub fn default_family(&self) -> &str {
        &self.families[0]
    }

    /// The first CSS family that names a registered font, else the default. Generic families
    /// (`serif`, `sans-serif`) have no system fonts to map to and fall through to the default.
    fn resolve(families: &[String], css_families: &[String]) -> String {
        css_families
            .iter()
            .find_map(|wanted| families.iter().find(|f| f.eq_ignore_ascii_case(wanted)))
            .unwrap_or(&families[0])
            .clone()
    }
}

/// The horizontal band a block's content occupies, relative to the page content box.
#[derive(Clone, Copy, Default)]
struct Inset {
    left: f32,
    right: f32,
}

/// Adjoining vertical margins waiting to collapse into one.
#[derive(Clone, Copy, Default)]
struct PendingMargin {
    positive: f32,
    negative: f32,
}

impl PendingMargin {
    fn add(&mut self, margin: f32) {
        self.positive = self.positive.max(margin);
        self.negative = self.negative.min(margin);
    }

    fn take(&mut self) -> f32 {
        std::mem::take(self).positive + self.negative
    }
}

struct Flow<'a, 'b> {
    fonts: &'a mut Fonts,
    layout_cx: LayoutContext<Brush>,
    images: &'a mut Images<'b>,
    sheets: [&'a Stylesheet; 2],
    page_styles: HashMap<Option<String>, Arc<PageStyle>>,
    pages: Vec<Page>,
    paragraphs: Vec<ParagraphLayout>,
    cursor: f32,
    margin: PendingMargin,
}

pub(crate) fn layout(
    root: &Block,
    user_agent: &Stylesheet,
    author: &Stylesheet,
    fonts: &mut Fonts,
    images: &mut Images<'_>,
) -> Result<DocumentLayout, Error> {
    let mut flow = Flow {
        fonts,
        layout_cx: LayoutContext::new(),
        images,
        sheets: [user_agent, author],
        page_styles: HashMap::new(),
        pages: Vec::new(),
        paragraphs: Vec::new(),
        cursor: 0.0,
        margin: PendingMargin::default(),
    };
    flow.new_page(None)?;
    flow.block(root, Inset::default(), None)?;
    Ok(DocumentLayout {
        paragraphs: flow.paragraphs,
        pages: flow.pages,
    })
}

impl Flow<'_, '_> {
    fn page(&self) -> &Page {
        self.pages.last().expect("there is always a current page")
    }

    fn page_style(&mut self, name: Option<&str>) -> Result<Arc<PageStyle>, Error> {
        let key = name.map(str::to_string);
        if let Some(style) = self.page_styles.get(&key) {
            return Ok(style.clone());
        }
        let style = Arc::new(PageStyle::resolve(&self.sheets, name)?);
        self.page_styles.insert(key, style.clone());
        Ok(style)
    }

    fn new_page(&mut self, name: Option<&str>) -> Result<(), Error> {
        let style = self.page_style(name)?;
        self.cursor = style.margin.top;
        self.margin = PendingMargin::default();
        self.pages.push(Page {
            name: name.map(str::to_string),
            style,
            lines: Vec::new(),
            images: Vec::new(),
            links: Vec::new(),
            used: false,
        });
        Ok(())
    }

    /// Makes sure content named `name` lands on a page of that name: a used page of another name
    /// is left for a new one; an unused one is renamed in place.
    fn enter_page(&mut self, name: Option<&str>) -> Result<(), Error> {
        if self.page().name.as_deref() != name {
            if self.page().used {
                self.new_page(name)?;
            } else {
                let style = self.page_style(name)?;
                let page = self.pages.last_mut().expect("current page");
                page.name = name.map(str::to_string);
                self.cursor = style.margin.top;
                page.style = style;
            }
        }
        self.pages.last_mut().expect("current page").used = true;
        Ok(())
    }

    /// Settles pending margins before content. At the top of a page they are truncated, as CSS
    /// does for margins adjoining a page break.
    fn settle_margin(&mut self) {
        let collapsed = self.margin.take();
        if self.cursor > self.page().style.margin.top {
            self.cursor += collapsed;
        }
    }

    fn block(
        &mut self,
        block: &Block,
        inset: Inset,
        parent_page: Option<&str>,
    ) -> Result<(), Error> {
        let style = block.style();
        let page_name = style.page.as_deref().or(parent_page);
        let page_name = page_name.map(str::to_string);
        let page_name = page_name.as_deref();
        self.margin.add(style.margin.top);

        let inner = Inset {
            left: inset.left + style.margin.left + style.padding.left,
            right: inset.right + style.margin.right + style.padding.right,
        };
        let empty = matches!(block, Block::Container { children, .. } if children.is_empty());
        if !matches!(block, Block::Container { .. }) || empty {
            self.enter_page(page_name)?;
        }
        if style.padding.top > 0.0 {
            self.enter_page(page_name)?;
            self.settle_margin();
            self.cursor += style.padding.top;
        }

        match block {
            Block::Container { children, .. } => {
                for child in children {
                    self.block(child, inner, page_name)?;
                }
            }
            Block::Paragraph(paragraph) => self.paragraph(paragraph, inner)?,
            Block::Image { src, style } => self.image(src, style, inset)?,
        }

        if style.padding.bottom > 0.0 {
            self.settle_margin();
            self.cursor += style.padding.bottom;
        }
        self.margin.add(style.margin.bottom);
        Ok(())
    }

    fn paragraph(&mut self, paragraph: &Paragraph, inset: Inset) -> Result<(), Error> {
        let Fonts { context, families } = &mut *self.fonts;
        let mut builder = self
            .layout_cx
            .ranged_builder(context, &paragraph.text, 1.0, false);
        push_text_style(&mut builder, families, &paragraph.style.text, None);
        for span in &paragraph.spans {
            push_text_style(
                &mut builder,
                families,
                &span.style,
                Some(span.range.clone()),
            );
        }
        for (index, link) in paragraph.links.iter().enumerate() {
            // Links keep the colour and underline of their spans; the brush only gains the link.
            for span in &paragraph.spans {
                let start = span.range.start.max(link.range.start);
                let end = span.range.end.min(link.range.end);
                if start < end {
                    builder.push(
                        StyleProperty::Brush(brush(&span.style, Some(index))),
                        start..end,
                    );
                }
            }
        }
        let mut layout = builder.build(&paragraph.text);

        self.settle_margin();
        let index = self.paragraphs.len();
        let mut slots: Vec<(usize, f32, f32, f32)> = Vec::new();
        let mut breaker = layout.break_lines();
        breaker
            .state_mut()
            .set_layout_max_advance(self.page().style.content_width() - inset.left - inset.right);
        loop {
            let page_style = self.page().style.clone();
            let width = page_style.content_width() - inset.left - inset.right;
            if width <= 0.0 {
                return Err(Error::NoContentArea);
            }
            breaker.state_mut().set_line_max_advance(width);
            let line = match breaker.break_next() {
                None => break,
                Some(YieldData::LineBreak(line)) => line,
                Some(_) => {
                    return Err(Error::Unsupported(
                        "inline boxes and line height limits".into(),
                    ))
                }
            };
            let height = (line.line_y_end - line.line_y_start) as f32;
            // A line that doesn't fit is undone and broken again on a new page. On an empty page
            // it is placed anyway, so an oversized line can't loop forever.
            if self.cursor + height > page_style.content_bottom()
                && self.cursor > page_style.margin.top
            {
                breaker.revert();
                let name = self.page().name.clone();
                self.new_page(name.as_deref())?;
                self.pages.last_mut().expect("current page").used = true;
                continue;
            }
            slots.push((
                self.pages.len() - 1,
                page_style.margin.left + inset.left,
                self.cursor,
                height,
            ));
            self.cursor += height;
        }
        // A trailing <br> ends its line without opening an empty one after it, as in browsers
        // (`<p>a<br></p>` is one line, `<p><br></p>` one blank line). parley lays that empty
        // line out anyway, so it is dropped, along with the page it alone started.
        if paragraph.text.ends_with('\n') {
            if let (Some((phantom_page, ..)), Some(&(page, _, top, height))) =
                (slots.pop(), slots.last())
            {
                if phantom_page != page {
                    self.pages.pop();
                }
                self.cursor = top + height;
            }
        }
        breaker.finish();
        layout.align(Alignment::Start, AlignmentOptions::default());

        let laid_out = ParagraphLayout {
            text: paragraph.text.clone(),
            links: paragraph.links.clone(),
            layout,
        };
        for (line_index, (line, &(page, left, top, height))) in
            laid_out.layout.lines().zip(&slots).enumerate()
        {
            let placed = PlacedLine {
                paragraph: index,
                line: line_index,
                left,
                top,
                bottom: top + height,
                dy: top - line.metrics().block_min_coord,
            };
            self.pages[page]
                .links
                .extend(link_areas(&line, &placed, &laid_out));
            self.pages[page].lines.push(placed);
        }
        self.paragraphs.push(laid_out);
        Ok(())
    }

    fn image(&mut self, src: &str, style: &style::Style, inset: Inset) -> Result<(), Error> {
        let image = self.images.get(src)?;
        let (px_width, px_height) = (image.width, image.height);
        let available = self.page().style.content_width()
            - inset.left
            - inset.right
            - style.margin.left
            - style.margin.right;
        let ratio = px_height as f32 / px_width.max(1) as f32;
        let (mut width, mut height) = match (style.width, style.height) {
            (Some(w), Some(h)) => (style::resolve(w, available), style::resolve(h, available)),
            (Some(w), None) => (
                style::resolve(w, available),
                style::resolve(w, available) * ratio,
            ),
            (None, Some(h)) => (
                style::resolve(h, available) / ratio,
                style::resolve(h, available),
            ),
            (None, None) => (px_width as f32 * 0.75, px_height as f32 * 0.75),
        };
        if let Some(max) = style.max_width.map(|m| style::resolve(m, available)) {
            if width > max {
                if style.height.is_none() {
                    height *= max / width;
                }
                width = max;
            }
        }

        self.settle_margin();
        let page_style = self.page().style.clone();
        if self.cursor + height > page_style.content_bottom() && self.cursor > page_style.margin.top
        {
            let name = self.page().name.clone();
            self.new_page(name.as_deref())?;
            self.pages.last_mut().expect("current page").used = true;
        }
        let left = self.page().style.margin.left + inset.left + style.margin.left;
        let top = self.cursor;
        self.pages
            .last_mut()
            .expect("current page")
            .images
            .push(PlacedImage {
                src: src.to_string(),
                left,
                top,
                width,
                height,
            });
        self.cursor += height;
        Ok(())
    }
}

fn brush(style: &TextStyle, link: Option<usize>) -> Brush {
    Brush {
        color: style.color,
        underline: style.underline,
        link,
    }
}

fn push_text_style(
    builder: &mut parley::RangedBuilder<'_, Brush>,
    families: &[String],
    style: &TextStyle,
    range: Option<Range<usize>>,
) {
    let family = Fonts::resolve(families, &style.font_family);
    let line_height = match style.line_height {
        LineHeight::Normal => parley::LineHeight::MetricsRelative(1.0),
        LineHeight::Factor(factor) => parley::LineHeight::FontSizeRelative(factor),
        LineHeight::Points(points) => parley::LineHeight::Absolute(points),
    };
    let properties = [
        StyleProperty::FontFamily(FontFamily::from(FontFamilyName::Named(family.into()))),
        StyleProperty::FontSize(style.font_size),
        StyleProperty::FontWeight(FontWeight::new(style.font_weight)),
        StyleProperty::LineHeight(line_height),
        StyleProperty::Brush(brush(style, None)),
    ];
    for property in properties {
        match &range {
            None => builder.push_default(property),
            Some(range) => builder.push(property, range.clone()),
        }
    }
}

/// Clusters of a line in visual order, with x positions in layout space.
pub(crate) fn positioned_clusters(line: &Line<'_, Brush>) -> Vec<PositionedCluster> {
    let mut x = line.metrics().offset;
    let mut clusters = Vec::new();
    for run in line.runs() {
        for cluster in run.visual_clusters() {
            let advance = cluster.advance();
            clusters.push(PositionedCluster {
                x,
                advance,
                text_range: cluster.text_range(),
                link: cluster.style().brush.link,
            });
            x += advance;
        }
    }
    clusters
}

/// One clickable area per stretch of the same link on this line. An area spans from the link's
/// first to its last visible cluster on the line, so whitespace at either end is not clickable.
fn link_areas(
    line: &Line<'_, Brush>,
    placed: &PlacedLine,
    paragraph: &ParagraphLayout,
) -> Vec<LinkArea> {
    let mut areas: Vec<(usize, f32, f32)> = Vec::new();
    for cluster in positioned_clusters(line) {
        let Some(link) = cluster.link else { continue };
        if paragraph.text[cluster.text_range.clone()].trim().is_empty() {
            continue;
        }
        let (left, right) = (cluster.x, cluster.x + cluster.advance);
        match areas.last_mut() {
            Some((last, _, last_right)) if *last == link => *last_right = right,
            _ => areas.push((link, left, right)),
        }
    }
    areas
        .into_iter()
        .map(|(link, left, right)| LinkArea {
            href: paragraph.links[link].href.clone(),
            left: placed.left + left,
            top: placed.top,
            right: placed.left + right,
            bottom: placed.bottom,
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::images::png_header;
    use crate::{css, dom};

    pub(crate) fn font() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/DejaVuSerif.ttf"
        ))
        .expect("test font")
    }

    pub(crate) fn fixture(name: &str) -> String {
        std::fs::read_to_string(format!(
            "{}/tests/fixtures/{name}",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture")
    }

    /// Lays out `html` with images of the given (src, width, height) sizes in px.
    pub(crate) fn lay_out_with(html: &str, images: &[(&str, u32, u32)]) -> DocumentLayout {
        let mut fonts = Fonts::new(&[font()]).unwrap();
        let ua = Stylesheet::parse(css::USER_AGENT);
        let document = dom::parse(html, &ua, fonts.default_family()).unwrap();
        let mut load = |src: &str| {
            images
                .iter()
                .find(|(s, _, _)| *s == src)
                .map(|&(_, width, height)| png_header(width, height))
        };
        let mut images = Images::new(&mut load);
        layout(
            &document.root,
            &ua,
            &document.stylesheet,
            &mut fonts,
            &mut images,
        )
        .unwrap()
    }

    pub(crate) fn lay_out(html: &str) -> DocumentLayout {
        lay_out_with(html, &[])
    }

    fn line_text(document: &DocumentLayout, placed: &PlacedLine) -> String {
        let paragraph = &document.paragraphs[placed.paragraph];
        let line = paragraph
            .layout
            .lines()
            .nth(placed.line)
            .expect("placed line exists");
        paragraph.text[line.text_range()].to_string()
    }

    /// Visible clusters of a placed line, as page-space (left, right) extents.
    fn ink_extent(document: &DocumentLayout, placed: &PlacedLine) -> (f32, f32) {
        let paragraph = &document.paragraphs[placed.paragraph];
        let line = paragraph.layout.lines().nth(placed.line).unwrap();
        let visible: Vec<_> = positioned_clusters(&line)
            .into_iter()
            .filter(|c| !paragraph.text[c.text_range.clone()].trim().is_empty())
            .collect();
        let left = visible.iter().map(|c| c.x).fold(f32::MAX, f32::min);
        let right = visible.iter().map(|c| c.x + c.advance).fold(0.0, f32::max);
        (placed.left + left, placed.left + right)
    }

    #[test]
    fn spans_three_pages_and_keeps_every_line_in_order() {
        let html = fixture("paragraphs.html");
        let document = lay_out(&html);
        assert_eq!(document.pages.len(), 3);

        let mut rebuilt = vec![String::new(); document.paragraphs.len()];
        let mut previous = None;
        for placed in document.pages.iter().flat_map(|page| &page.lines) {
            let key = (placed.paragraph, placed.line);
            assert!(
                previous < Some(key),
                "lines out of order or duplicated at {key:?}"
            );
            previous = Some(key);
            rebuilt[placed.paragraph].push_str(&line_text(&document, placed));
        }
        let expected: Vec<String> = document.paragraphs.iter().map(|p| p.text.clone()).collect();
        assert_eq!(rebuilt, expected);
        assert_eq!(expected.len(), 11);
    }

    #[test]
    fn a_paragraph_splits_between_lines_across_a_page_break() {
        let document = lay_out(&fixture("paragraphs.html"));
        let split = document.pages.windows(2).any(|pair| {
            let last = pair[0].lines.last().unwrap();
            let first = pair[1].lines.first().unwrap();
            last.paragraph == first.paragraph && first.line == last.line + 1
        });
        assert!(
            split,
            "expected at least one paragraph to continue on the next page"
        );
    }

    #[test]
    fn nothing_crosses_the_margins() {
        let document = lay_out(&fixture("paragraphs.html"));
        for page in &document.pages {
            let style = &page.style;
            let (left, right) = (style.margin.left, style.width - style.margin.right);
            let (top, bottom) = (style.margin.top, style.content_bottom());
            assert_eq!(
                (left, top),
                (37.5, 37.5),
                "the fixture's @page margin applies"
            );
            for placed in &page.lines {
                assert!(placed.top >= top - 0.01 && placed.bottom <= bottom + 0.01);
                let (ink_left, ink_right) = ink_extent(&document, placed);
                assert!(
                    ink_left >= left - 0.01 && ink_right <= right + 0.01,
                    "line overflows the margins"
                );
            }
            for area in &page.links {
                assert!(area.left >= left - 0.01 && area.right <= right + 0.01);
                assert!(area.top >= top - 0.01 && area.bottom <= bottom + 0.01);
            }
        }
    }

    #[test]
    fn a_wrapped_link_gets_one_area_per_line() {
        let document = lay_out(&fixture("paragraphs.html"));
        let areas: Vec<&LinkArea> = document
            .pages
            .iter()
            .flat_map(|page| &page.links)
            .filter(|area| area.href == "https://example.com/wrapped-link")
            .collect();
        assert_eq!(
            areas.len(),
            2,
            "the long link should wrap onto exactly two lines"
        );
        assert!(
            areas[1].top >= areas[0].bottom - 0.01,
            "second area is on a later line"
        );
    }

    #[test]
    fn adjoining_margins_collapse_and_are_truncated_at_the_page_top() {
        let document = lay_out(
            "<style>@page { margin: 20pt } body { margin: 0; line-height: 20pt }
             h1 { margin: 30pt 0 10pt } p { margin: 12pt 0 }</style>
             <h1>Title</h1><p>One</p><div><p>Two</p></div>",
        );
        let lines = &document.pages[0].lines;
        let tops: Vec<f32> = lines.iter().map(|l| l.top).collect();
        // h1's 30pt top margin is truncated at the page top; 10pt and 12pt collapse to 12pt;
        // p's 12pt bottom and the next p's 12pt top (through the div) collapse to 12pt.
        assert_eq!(tops, [20.0, 20.0 + 20.0 + 12.0, 52.0 + 20.0 + 12.0]);
    }

    fn line_texts(html: &str) -> Vec<String> {
        let document = lay_out(&format!(
            "<style>body {{ margin: 0 }} p {{ margin: 0; line-height: 20pt }}</style>{html}"
        ));
        assert_eq!(document.pages.len(), 1);
        let lines = &document.pages[0].lines;
        for (index, line) in lines.iter().enumerate() {
            let expected = 72.0 / 2.54 + 20.0 * index as f32; // the default 1cm page margin
            assert!(
                (line.top - expected).abs() < 0.01,
                "every line is a full line high"
            );
        }
        lines
            .iter()
            .map(|placed| line_text(&document, placed))
            .collect()
    }

    #[test]
    fn br_breaks_lines_like_a_browser() {
        assert_eq!(line_texts("<p>a<br>b</p>"), ["a\n", "b"]);
        assert_eq!(
            line_texts("<p>a<br></p>"),
            ["a\n"],
            "a trailing <br> adds no line"
        );
        assert_eq!(line_texts("<p>a<br><br>b</p>"), ["a\n", "\n", "b"]);
        assert_eq!(
            line_texts("<p><br></p>"),
            ["\n"],
            "a lone <br> is one blank line"
        );
        assert_eq!(line_texts("<p>a<br><br></p>"), ["a\n", "\n"]);
    }

    #[test]
    fn a_link_across_a_br_gets_one_area_per_line() {
        let document = lay_out("<p><a href=\"https://x.test\">one<br>two</a></p>");
        let page = &document.pages[0];
        assert_eq!(page.links.len(), 2);
        for (area, line) in page.links.iter().zip(&page.lines) {
            assert_eq!((area.top, area.bottom), (line.top, line.bottom));
            let (left, right) = ink_extent(&document, line);
            assert!((area.left - left).abs() < 0.01 && (area.right - right).abs() < 0.01);
        }
    }

    #[test]
    fn a_trailing_br_does_not_start_an_empty_page() {
        let html = "<style>@page { size: 100pt 60pt; margin: 0 } body { margin: 0 }
                    p { margin: 0; line-height: 20pt }</style><p>a<br>b<br>c<br></p>";
        assert_eq!(
            lay_out(html).pages.len(),
            1,
            "three lines fill the page exactly"
        );

        let document = lay_out(&format!("{html}<p>d</p>"));
        assert_eq!(document.pages.len(), 2);
        assert_eq!(
            document.pages[1].lines[0].top, 0.0,
            "the next paragraph starts page 2"
        );
    }

    #[test]
    fn images_keep_their_aspect_ratio_within_max_width() {
        let document = lay_out_with(
            "<style>@page { size: A4; margin: 50pt } body { margin: 0 } img.fluid { max-width: 100% }</style>
             <img class=\"fluid\" src=\"wide.png\"><img class=\"fluid\" src=\"small.png\">",
            &[("wide.png", 1600, 400), ("small.png", 40, 20)],
        );
        let placed = &document.pages[0].images;
        let content_width = 595.28 - 100.0;
        assert!((placed[0].width - content_width).abs() < 0.01);
        assert!((placed[0].height - content_width / 4.0).abs() < 0.01);
        assert_eq!(
            (placed[1].width, placed[1].height),
            (30.0, 15.0),
            "40×20 px is 30×15 pt"
        );
        assert!((placed[1].top - (placed[0].top + placed[0].height)).abs() < 0.01);
    }

    #[test]
    fn text_on_a_background_page_flows_like_the_pages_around_it() {
        let document = lay_out(&fixture("background-page.html"));
        let names: Vec<Option<&str>> = document.pages.iter().map(|p| p.name.as_deref()).collect();
        assert_eq!(names, [None, Some("full"), None]);
        assert_eq!(
            document.pages[1].style.background_image.as_deref(),
            Some("bg.png")
        );
        assert_eq!(document.pages[0].style.background_image, None);

        for page in &document.pages {
            assert_eq!(
                page.style.margin, document.pages[0].style.margin,
                "same margins on every page"
            );
            let first = page.lines[0];
            assert_eq!(
                (first.left, first.top),
                (50.0, 50.0),
                "content starts at the page's margins"
            );
        }
        // Each page opens with a paragraph of about the same length; on equal widths they wrap
        // into the same number of lines.
        let line_counts: Vec<usize> = document
            .pages
            .iter()
            .map(|page| {
                page.lines
                    .iter()
                    .filter(|l| l.paragraph == page.lines[0].paragraph)
                    .count()
            })
            .collect();
        assert!(
            line_counts[0] > 1 && line_counts.iter().all(|c| *c == line_counts[0]),
            "{line_counts:?}"
        );
        for page in &document.pages {
            for placed in &page.lines {
                let (left, right) = ink_extent(&document, placed);
                assert!(left >= 50.0 - 0.01 && right <= 595.28 - 50.0 + 0.01);
            }
        }
    }

    #[test]
    fn named_pages_force_breaks_and_an_empty_named_box_still_gets_its_page() {
        let document = lay_out(
            "<style>@page { margin: 40pt } @page cover { margin: 0 } section.cover { page: cover }
             body { margin: 0 }</style>
             <section class=\"cover\"></section><p>After the cover.</p>
             <section class=\"cover\"><p>Second cover</p></section><p>End</p>",
        );
        let names: Vec<Option<&str>> = document.pages.iter().map(|p| p.name.as_deref()).collect();
        assert_eq!(names, [Some("cover"), None, Some("cover"), None]);
        assert!(
            document.pages[0].lines.is_empty(),
            "the empty cover section still made its page"
        );
        assert_eq!(
            document.pages[1].lines[0].top, 40.0,
            "the margin truncates at the forced break"
        );
        assert_eq!(document.pages[2].style.margin.left, 0.0);
        assert_eq!(document.pages[3].lines.len(), 1);
    }
}
