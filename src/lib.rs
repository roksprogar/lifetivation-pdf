//! HTML/CSS to PDF rendering in pure Rust, built for paged media.
//!
//! Supported so far: block layout with collapsing margins and padding; paragraphs with styled
//! text, `<br>` line breaks and clickable `<a href>` links; PNG images; CSS from `<style>`
//! elements and `style` attributes, with compound selectors; `@page` size, margins and
//! backgrounds, including named pages (`page: name`).
//! An element outside that set, or an attribute that would change its rendering (`align`,
//! `width`, ...), is reported as [`Error::Unsupported`] instead of rendering wrong, as is a `style`
//! attribute declaration it can't render. Unsupported CSS in `<style>` elements is ignored.

mod css;
mod dom;
mod images;
mod layout;
mod paint;
mod style;

use std::fmt;

/// An sRGB colour.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

/// Rendering settings that don't change between documents; build it once and reuse it.
#[derive(Clone, Debug, Default)]
#[non_exhaustive]
pub struct Config {
    /// Font files (TTF/OTF). CSS `font-family` names match their family names; the first font's
    /// family is the default for text without a matching family. Faces of one family (regular,
    /// bold, italic) are matched by weight and style; bold or italic text in a family without
    /// that face is synthesized: bold when the nearest face is more than 200 lighter (so `bold` on
    /// a regular face, but not `600`), italic when there is no italic face. Variable fonts draw
    /// their default instance only, so give each weight and style as its own static font file.
    /// No system fonts are used.
    pub fonts: Vec<Vec<u8>>,
}

impl Config {
    pub fn new(fonts: Vec<Vec<u8>>) -> Self {
        Self { fonts }
    }
}

#[derive(Debug)]
#[non_exhaustive]
pub enum Error {
    /// A configured font could not be loaded, or no font was configured.
    InvalidFont,
    /// The image loader returned nothing for this reference.
    MissingImage(String),
    /// An image could not be read; only PNG is supported.
    InvalidImage(String),
    /// The margins leave no room for content on the page.
    NoContentArea,
    /// The document needs a feature this version doesn't support yet, such as an element
    /// (`<ul>`, `<table>`) it can't render correctly.
    Unsupported(String),
    /// The PDF writer rejected the document.
    Pdf(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::InvalidFont => write!(
                f,
                "a configured font is not a valid TTF/OTF font, or none was given"
            ),
            Error::MissingImage(src) => write!(f, "the image {src:?} was not provided"),
            Error::InvalidImage(src) => write!(f, "the image {src:?} is not a valid PNG"),
            Error::NoContentArea => write!(f, "the page margins leave no room for content"),
            Error::Unsupported(what) => write!(f, "not supported yet: {what}"),
            Error::Pdf(reason) => write!(f, "the PDF could not be written: {reason}"),
        }
    }
}

impl std::error::Error for Error {}

/// Renders `html` into PDF bytes.
///
/// `load_image` is called with each image reference the document uses (`<img src>`, `@page`
/// `background-image: url(…)`), once per reference, and returns the file's bytes, or `None`
/// to fail with [`Error::MissingImage`]. It decides what a reference means (a path, an asset
/// key, a URL), so it should also refuse references it doesn't trust, such as paths outside
/// an assets folder.
pub fn render(
    html: &str,
    config: &Config,
    mut load_image: impl FnMut(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>, Error> {
    let mut fonts = layout::Fonts::new(&config.fonts)?;
    let user_agent = css::Stylesheet::parse(css::USER_AGENT);
    let document = dom::parse(html, &user_agent, fonts.default_family())?;
    let mut images = images::Images::new(&mut load_image);
    let layout = layout::layout(
        &document.root,
        &user_agent,
        &document.stylesheet,
        &mut fonts,
        &mut images,
    )?;
    paint::paint(&layout, &mut images, document.title.as_deref())
}
