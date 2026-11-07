//! HTML → a tree of styled block boxes.
//!
//! Block-level elements become containers; runs of inline content (text and inline elements)
//! become paragraphs, anonymous ones when they sit next to blocks. Whitespace collapses as CSS
//! `white-space: normal`, `<a href>` becomes a link, and an `<img>` that is alone on its line
//! becomes an image block.

use std::ops::Range;

use html5ever::tendril::TendrilSink;
use markup5ever_rcdom::{Handle, NodeData, RcDom};

use crate::css::{self, Stylesheet};
use crate::style::{Display, Style, TextStyle};
use crate::Error;

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Link {
    /// Byte range into [`Paragraph::text`].
    pub range: Range<usize>,
    pub href: String,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Span {
    /// Byte range into [`Paragraph::text`].
    pub range: Range<usize>,
    pub style: TextStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Paragraph {
    pub style: Style,
    pub text: String,
    /// Consecutive, non-overlapping and covering all of `text`.
    pub spans: Vec<Span>,
    pub links: Vec<Link>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Block {
    Container { style: Style, children: Vec<Block> },
    Paragraph(Paragraph),
    Image { style: Style, src: String },
}

impl Block {
    pub fn style(&self) -> &Style {
        match self {
            Block::Container { style, .. } | Block::Image { style, .. } => style,
            Block::Paragraph(paragraph) => &paragraph.style,
        }
    }
}

pub(crate) struct Document {
    pub root: Block,
    /// Author styles from every `<style>` element, in document order.
    pub stylesheet: Stylesheet,
    /// The text of the first `<title>`, whitespace collapsed; becomes the PDF title.
    pub title: Option<String>,
}

pub(crate) fn parse(
    html: &str,
    user_agent: &Stylesheet,
    default_font_family: &str,
) -> Result<Document, Error> {
    let dom = html5ever::parse_document(RcDom::default(), Default::default()).one(html);
    let mut css_source = String::new();
    let mut title = None;
    collect_head(&dom.document, &mut css_source, &mut title);
    let stylesheet = Stylesheet::parse(&css_source);

    let html_element = dom
        .document
        .children
        .borrow()
        .iter()
        .find(|child| element_name(child) == Some("html"))
        .cloned()
        .ok_or(Error::Unsupported(
            "documents without an <html> element".into(),
        ))?;
    let builder = TreeBuilder {
        sheets: [user_agent, &stylesheet],
    };
    let root_style = builder.style(&html_element, &Style::initial(default_font_family))?;
    let root = builder.block(&html_element, root_style)?;
    Ok(Document {
        root,
        stylesheet,
        title,
    })
}

fn collect_head(node: &Handle, out: &mut String, title: &mut Option<String>) {
    if element_name(node) == Some("title") && title.is_none() {
        let mut text = String::new();
        for child in node.children.borrow().iter() {
            if let NodeData::Text { contents } = &child.data {
                text.push_str(&contents.borrow());
            }
        }
        let collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
        *title = (!collapsed.is_empty()).then_some(collapsed);
        return;
    }
    if element_name(node) == Some("style") {
        for child in node.children.borrow().iter() {
            if let NodeData::Text { contents } = &child.data {
                out.push_str(&contents.borrow());
                out.push('\n');
            }
        }
        return;
    }
    for child in node.children.borrow().iter() {
        collect_head(child, out, title);
    }
}

fn element_name(node: &Handle) -> Option<&str> {
    match &node.data {
        NodeData::Element { name, .. } => Some(&name.local),
        _ => None,
    }
}

fn attribute(node: &Handle, attribute: &str) -> Option<String> {
    match &node.data {
        NodeData::Element { attrs, .. } => attrs
            .borrow()
            .iter()
            .find(|a| &*a.name.local == attribute)
            .map(|a| a.value.to_string()),
        _ => None,
    }
}

/// Elements this crate renders the way a browser would. Any other element that is displayed fails
/// the render, so the document can't silently come out wrong: `<li>` items would run into one
/// paragraph, `<strong>` would be regular weight. Hidden elements
/// (`display: none`, like `<head>` and `<script>`) are never checked.
const SUPPORTED_ELEMENTS: &[&str] = &[
    "html", "body", "div", "section", "article", "header", "footer", "main", "nav", "aside", "p",
    "h1", "h2", "span", "a", "img", "br",
];

/// Attributes that change how an element renders, which this crate doesn't read.
const UNSUPPORTED_ATTRIBUTES: &[&str] = &["style", "align", "width", "height", "dir"];

fn check_supported(node: &Handle) -> Result<(), Error> {
    let NodeData::Element { name, attrs, .. } = &node.data else {
        return Ok(());
    };
    let tag = &*name.local;
    if !SUPPORTED_ELEMENTS.contains(&tag) {
        return Err(Error::Unsupported(format!("the <{tag}> element")));
    }
    if let Some(attribute) = attrs
        .borrow()
        .iter()
        .find(|a| UNSUPPORTED_ATTRIBUTES.contains(&&*a.name.local))
    {
        return Err(Error::Unsupported(format!(
            "the {} attribute on <{tag}>",
            attribute.name.local
        )));
    }
    Ok(())
}

struct TreeBuilder<'a> {
    sheets: [&'a Stylesheet; 2],
}

impl TreeBuilder<'_> {
    /// The element's computed style; fails if it is displayed but not supported.
    fn style(&self, node: &Handle, parent: &Style) -> Result<Style, Error> {
        let tag = element_name(node).unwrap_or_default();
        let id = attribute(node, "id");
        let class = attribute(node, "class").unwrap_or_default();
        let classes: Vec<&str> = class.split_whitespace().collect();
        let specified = css::cascade(&self.sheets, tag, id.as_deref(), &classes);
        let style = Style::compute(parent, &specified);
        if style.display != Display::None {
            check_supported(node)?;
        }
        Ok(style)
    }

    /// A block-level element. If all its content is inline it becomes one paragraph.
    fn block(&self, node: &Handle, style: Style) -> Result<Block, Error> {
        let mut children = Vec::new();
        let mut inline = ParagraphBuilder::new(Style::anonymous(&style));
        self.block_children(node, &style, &mut children, &mut inline)?;

        if children.is_empty() {
            if let Some(paragraph) = inline.finish() {
                return Ok(Block::Paragraph(Paragraph { style, ..paragraph }));
            }
        } else if let Some(paragraph) = inline.finish() {
            children.push(Block::Paragraph(paragraph));
        }
        Ok(Block::Container { style, children })
    }

    fn block_children(
        &self,
        node: &Handle,
        style: &Style,
        children: &mut Vec<Block>,
        inline: &mut ParagraphBuilder,
    ) -> Result<(), Error> {
        for child in node.children.borrow().iter() {
            match &child.data {
                NodeData::Text { contents } => inline.push_text(&contents.borrow(), &style.text)?,
                NodeData::Element { .. } => {
                    let child_style = self.style(child, style)?;
                    match (child_style.display, element_name(child)) {
                        (Display::None, _) => {}
                        // An image becomes an image block whatever its display; as a Block it
                        // would be an empty container and vanish.
                        (_, Some("img")) => {
                            if inline.ends_with_line_break() {
                                // Text ending in <br> is finished; the image starts a new line.
                                let before = std::mem::replace(
                                    inline,
                                    ParagraphBuilder::new(Style::anonymous(style)),
                                );
                                children.extend(before.finish().map(Block::Paragraph));
                            } else if inline.has_text() {
                                return Err(Error::Unsupported(
                                    "images on a line with text".into(),
                                ));
                            }
                            let src = attribute(child, "src").unwrap_or_default();
                            children.push(Block::Image {
                                style: child_style,
                                src,
                            });
                            inline.after_image = true;
                        }
                        (_, Some("br")) => inline.line_break(&child_style.text),
                        (Display::Block, _) => {
                            let anonymous = std::mem::replace(
                                inline,
                                ParagraphBuilder::new(Style::anonymous(style)),
                            );
                            children.extend(anonymous.finish().map(Block::Paragraph));
                            children.push(self.block(child, child_style)?);
                        }
                        (Display::Inline, _) => self.inline(child, &child_style, inline)?,
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn inline(
        &self,
        node: &Handle,
        style: &Style,
        paragraph: &mut ParagraphBuilder,
    ) -> Result<(), Error> {
        let href = (element_name(node) == Some("a"))
            .then(|| attribute(node, "href"))
            .flatten();
        // A collapsible space before the link belongs outside it.
        if href.is_some() {
            paragraph.flush_pending_space();
        }
        let mut start = paragraph.text.len();

        for child in node.children.borrow().iter() {
            match &child.data {
                NodeData::Text { contents } => {
                    paragraph.push_text(&contents.borrow(), &style.text)?
                }
                NodeData::Element { .. } => {
                    let child_style = self.style(child, style)?;
                    match (child_style.display, element_name(child)) {
                        (Display::None, _) => {}
                        (_, Some("img")) => {
                            return Err(Error::Unsupported("images inside inline elements".into()))
                        }
                        (_, Some("br")) => paragraph.line_break(&child_style.text),
                        (Display::Block, _) => {
                            return Err(Error::Unsupported(
                                "block elements inside inline elements".into(),
                            ))
                        }
                        (Display::Inline, _) => self.inline(child, &child_style, paragraph)?,
                    }
                }
                _ => {}
            }
        }

        if let Some(href) = href {
            let end = paragraph.text.len();
            // Whitespace at the start of the link text stays outside the link.
            while start < end && paragraph.text.as_bytes()[start].is_ascii_whitespace() {
                start += 1;
            }
            if end > start {
                paragraph.links.push(Link {
                    range: start..end,
                    href,
                });
            }
        }
        Ok(())
    }
}

/// Accumulates collapsed text, style spans and link ranges for one paragraph.
struct ParagraphBuilder {
    style: Style,
    text: String,
    spans: Vec<Span>,
    links: Vec<Link>,
    /// A collapsible space is pending; it is written only if more text follows, in the style of
    /// that text.
    pending_space: bool,
    /// An image block was emitted from this inline context; text may not follow on its line.
    after_image: bool,
}

impl ParagraphBuilder {
    fn new(style: Style) -> Self {
        Self {
            style,
            text: String::new(),
            spans: Vec::new(),
            links: Vec::new(),
            pending_space: false,
            after_image: false,
        }
    }

    fn has_text(&self) -> bool {
        !self.text.is_empty()
    }

    fn push_text(&mut self, raw: &str, style: &TextStyle) -> Result<(), Error> {
        for ch in raw.chars() {
            if ch.is_ascii_whitespace() {
                // Whitespace collapses into one space, and disappears at the start of a line.
                self.pending_space = !self.text.is_empty() && !self.text.ends_with([' ', '\n']);
                continue;
            }
            if self.after_image {
                return Err(Error::Unsupported("images on a line with text".into()));
            }
            if self.pending_space {
                self.push_char(' ', style);
                self.pending_space = false;
            }
            self.push_char(ch, style);
        }
        Ok(())
    }

    /// `<br>`: a forced line break. A pending space before it is dropped, as at any line end.
    /// Right after an image block it only ends the image's line, so text may follow.
    fn line_break(&mut self, style: &TextStyle) {
        self.pending_space = false;
        if self.after_image && self.text.is_empty() {
            self.after_image = false;
            return;
        }
        self.push_char('\n', style);
    }

    fn ends_with_line_break(&self) -> bool {
        self.text.ends_with('\n')
    }

    fn flush_pending_space(&mut self) {
        if self.pending_space {
            let style = self
                .spans
                .last()
                .expect("a pending space follows text")
                .style
                .clone();
            self.push_char(' ', &style);
            self.pending_space = false;
        }
    }

    fn push_char(&mut self, ch: char, style: &TextStyle) {
        let start = self.text.len();
        self.text.push(ch);
        match self.spans.last_mut() {
            Some(span) if span.style == *style => span.range.end = self.text.len(),
            _ => self.spans.push(Span {
                range: start..self.text.len(),
                style: style.clone(),
            }),
        }
    }

    fn finish(self) -> Option<Paragraph> {
        (!self.text.is_empty()).then_some(Paragraph {
            style: self.style,
            text: self.text,
            spans: self.spans,
            links: self.links,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_body(html: &str) -> Vec<Block> {
        let ua = Stylesheet::parse(css::USER_AGENT);
        let document = parse(html, &ua, "Test").expect("parses");
        let Block::Container { children, .. } = document.root else {
            panic!("root is a container")
        };
        let body = children.into_iter().last().expect("body");
        match body {
            Block::Container { children, .. } => children,
            paragraph => vec![paragraph],
        }
    }

    fn paragraph(block: &Block) -> &Paragraph {
        match block {
            Block::Paragraph(paragraph) => paragraph,
            other => panic!("expected a paragraph, got {other:?}"),
        }
    }

    #[test]
    fn collapses_whitespace_and_keeps_link_ranges() {
        let blocks = parse_body(
            "<p>  Hello\n   <span>bold</span>  <a href=\"https://x.test\"> the  link </a> end. </p>",
        );
        let p = paragraph(&blocks[0]);
        assert_eq!(p.text, "Hello bold the link end.");
        assert_eq!(p.links.len(), 1);
        assert_eq!(&p.text[p.links[0].range.clone()], "the link");
        assert_eq!(p.links[0].href, "https://x.test");
        let link_span = p
            .spans
            .iter()
            .find(|s| s.range.contains(&p.links[0].range.start))
            .unwrap();
        assert!(
            link_span.style.underline,
            "the user-agent stylesheet underlines links"
        );
    }

    #[test]
    fn anchor_without_href_is_plain_text() {
        let blocks = parse_body("<p>a <a name=\"x\">b</a> c</p>");
        let p = paragraph(&blocks[0]);
        assert_eq!(p.text, "a b c");
        assert!(p.links.is_empty());
    }

    #[test]
    fn mixed_content_gets_anonymous_paragraphs_and_style_is_hidden() {
        let blocks = parse_body(
            "<head><style>h2 { font-size: 30px } .x { color: #ff0000 }</style></head>
             <body>loose <span class=\"x\">red</span><h2>Title</h2>  <p>One</p><p>  </p>tail</body>",
        );
        let texts: Vec<_> = blocks
            .iter()
            .map(|b| match b {
                Block::Paragraph(p) => p.text.clone(),
                Block::Container { children, .. } => format!("<{} children>", children.len()),
                Block::Image { .. } => "<img>".into(),
            })
            .collect();
        assert_eq!(texts, ["loose red", "Title", "One", "<0 children>", "tail"]);
        let loose = paragraph(&blocks[0]);
        assert_eq!(loose.spans.len(), 2, "the red span starts a new style run");
        assert_eq!(loose.spans[1].style.color, crate::Rgb(255, 0, 0));
        assert_eq!(paragraph(&blocks[1]).style.text.font_size, 22.5);
        assert_eq!(
            loose.style.margin.top, 0.0,
            "anonymous paragraphs have no margins"
        );
    }

    #[test]
    fn an_image_alone_on_its_line_is_an_image_block() {
        let blocks =
            parse_body("<p>before</p>\n<img class=\"img-fluid\" src=\"a.png\">\n<p>after</p>");
        assert!(matches!(&blocks[1], Block::Image { src, .. } if src == "a.png"));
        assert_eq!(blocks.len(), 3);
        assert!(matches!(
            parse(
                "<body>text <img src=\"a.png\"></body>",
                &Stylesheet::parse(css::USER_AGENT),
                "Test"
            ),
            Err(Error::Unsupported(_))
        ));
    }

    #[test]
    fn br_is_a_line_break_and_eats_the_spaces_around_it() {
        let blocks = parse_body("<p>one <br>  two<a href=\"https://x.test\">three<br>four</a></p>");
        let p = paragraph(&blocks[0]);
        assert_eq!(p.text, "one\ntwothree\nfour");
        assert_eq!(&p.text[p.links[0].range.clone()], "three\nfour");
    }

    #[test]
    fn br_lets_an_image_and_text_share_a_block() {
        let blocks = parse_body(
            "<div>before<br><img src=\"a.png\"></div><div><img src=\"b.png\"><br>after</div>",
        );
        let [Block::Container {
            children: first, ..
        }, Block::Container {
            children: second, ..
        }] = blocks.as_slice()
        else {
            panic!("two containers: {blocks:?}")
        };
        assert_eq!(paragraph(&first[0]).text, "before\n");
        assert!(matches!(&first[1], Block::Image { src, .. } if src == "a.png"));
        assert!(matches!(&second[0], Block::Image { src, .. } if src == "b.png"));
        assert_eq!(paragraph(&second[1]).text, "after");
    }

    #[test]
    fn an_image_with_display_block_is_still_an_image_block() {
        let blocks = parse_body(
            "<style>img { display: block }</style><p>before</p><img src=\"a.png\"><p>after</p>",
        );
        assert!(matches!(&blocks[1], Block::Image { src, .. } if src == "a.png"));
    }

    fn unsupported(html: &str) -> String {
        match parse(html, &Stylesheet::parse(css::USER_AGENT), "Test") {
            Err(Error::Unsupported(what)) => what,
            other => panic!(
                "expected Unsupported for {html:?}, got {:?}",
                other.map(|_| ())
            ),
        }
    }

    #[test]
    fn elements_and_attributes_it_cannot_render_fail_instead_of_rendering_wrong() {
        assert_eq!(unsupported("<p>one<hr>two</p>"), "the <hr> element");
        assert_eq!(
            unsupported("<ul><li>a</li><li>b</li></ul>"),
            "the <ul> element"
        );
        assert_eq!(
            unsupported("<p>a <strong>b</strong></p>"),
            "the <strong> element"
        );
        assert_eq!(unsupported("<h3>Title</h3>"), "the <h3> element");
        assert_eq!(
            unsupported("<p style=\"text-align: center\">a</p>"),
            "the style attribute on <p>"
        );
        assert_eq!(
            unsupported("<img src=\"a.png\" width=\"300\">"),
            "the width attribute on <img>"
        );
    }

    #[test]
    fn hidden_elements_are_not_checked() {
        let blocks = parse_body(
            "<head><title>T</title><meta charset=\"utf-8\"></head>
             <body><p>shown</p><script>x()</script><ul class=\"hidden\"><li>x</li></ul>
             <style>.hidden { display: none }</style><table class=\"hidden\"><tr><td>x</td></tr></table></body>",
        );
        assert_eq!(blocks.len(), 1);
        assert_eq!(paragraph(&blocks[0]).text, "shown");
    }
}
