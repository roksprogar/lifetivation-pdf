# lifetivation-pdf

<div align="center">

[![Crates.io](https://img.shields.io/crates/v/lifetivation-pdf.svg)](https://crates.io/crates/lifetivation-pdf)
[![Documentation](https://docs.rs/lifetivation-pdf/badge.svg)](https://docs.rs/lifetivation-pdf)
[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](https://opensource.org/licenses/MIT)

</div>

HTML/CSS to PDF rendering in pure Rust, built for paged media. No browser, no system packages, no
system font scanning: output depends only on the HTML and the fonts and images you pass in.

**Status: early.** CSS support grows from real documents, one feature at a time. Supported so far:

- CSS from `<style>` elements and `style` attributes: element, class, id and compound selectors
  (`img.img-fluid`) with specificity and inheritance; `font-family`, `font-size`, `font-weight`,
  `font-style`, `color`, `text-decoration`, `text-align` (including `justify`), `line-height`,
  `margin`, `padding`, `width`, `height`, `max-width`, `page`.
- Block layout with collapsing vertical margins; paragraphs split across pages between any two
  lines.
- Clickable `<a href>` links, including links that wrap across lines.
- PNG images (with alpha) that sit on their own line, aspect ratio kept.
- `@page` size, margins, `background-color` and `background-image` with `background-size: cover`,
  including named pages: a box with `page: name` starts on a new page with that `@page name`
  rule, and an empty one still creates its page, which makes full-bleed covers easy.

- Elements: `html`, `body`, `div`, `section`, `article`, `header`, `footer`, `main`, `nav`,
  `aside`, `p`, `h1`–`h6`, `span`, `a`, `b`, `strong`, `i`, `em`, `img`, and `br` for line breaks.
- Bold (`font-weight`, `<b>`, `<strong>`, headings) and italic (`font-style: italic | oblique`,
  `<i>`, `<em>`) draw with the matching face of the family when one is configured. Without one,
  they are synthesized as browsers do: bold by thickening the glyphs with a stroke, italic by
  slanting them. Fonts must be static TTF/OTF files, one per weight and style; a variable font
  draws its default instance only.

Any other displayed element (`<ul>`, `<table>`, `<u>`, ...), and the `align`,
`width`, `height` and `dir` attributes, fail the render with `Error::Unsupported` naming
them, so a document never comes out silently wrong. Hidden elements (`display: none`, such as
`<head>` and `<script>`) are skipped. A `style` attribute declaration the crate can't render
(`display: flex`, `background-color`, a percentage margin) fails the render too, naming it;
unsupported CSS in `<style>` elements is ignored.

## Installation

Add this to your `Cargo.toml`:

```toml
[dependencies]
lifetivation-pdf = "0.2"
```

## Usage

```rust
use lifetivation_pdf::{render, Config};

// Build once, reuse for every document.
let config = Config::new(vec![std::fs::read("DejaVuSerif.ttf")?]);
let html = r#"<style>
    @page { margin: 50px }
    @page cover { background-image: url("img/cover.png"); background-size: cover }
    section.cover { page: cover }
</style>
<section class="cover"></section>
<p>Hello, <a href="https://example.com">world</a>.</p>"#;
// Called once for each image the document uses; return None to fail with MissingImage.
// Refuse references you don't trust (absolute paths, `..`) before touching the disk.
let load_image = |src: &str| std::fs::read(std::path::Path::new("assets").join(src)).ok();
let pdf: Vec<u8> = render(html, &config, load_image)?;
```

## How it works

- HTML parsing: [`html5ever`](https://crates.io/crates/html5ever)
- CSS parsing, cascade, block layout, pagination and named pages: this crate
- Text shaping, line breaking and font handling: [`parley`](https://crates.io/crates/parley)
- PDF writing, font subsetting, images and link annotations: [`krilla`](https://crates.io/crates/krilla)

## Testing

Run the test suite with:

```bash
cargo test
```

The test font, `tests/fixtures/DejaVuSerif.ttf`, is DejaVu Serif (Bitstream Vera derived, free licence).

## Contributing

Contributions are welcome! Please feel free to submit a Pull Request. New CSS support comes with a
fixture that needs it.

## License

This project is licensed under the [MIT License](LICENSE).
