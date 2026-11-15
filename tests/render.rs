//! End-to-end checks on the PDF bytes, read back with lopdf.

use lifetivation_pdf::{render, Config, Error};
use lopdf::{Document, Object, ObjectId};

const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

fn fixture(name: &str) -> String {
    std::fs::read_to_string(format!("{FIXTURES}/{name}")).expect("fixture")
}

fn config() -> Config {
    Config::new(vec![
        std::fs::read(format!("{FIXTURES}/DejaVuSerif.ttf")).expect("test font")
    ])
}

fn no_images(_: &str) -> Option<Vec<u8>> {
    None
}

fn load(bytes: &[u8]) -> Document {
    assert!(bytes.starts_with(b"%PDF-"), "output is a PDF");
    Document::load_mem(bytes).expect("lopdf can parse the output")
}

/// A generated RGB test image: a diagonal gradient.
fn test_png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, width, height);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    let pixels: Vec<u8> = (0..height)
        .flat_map(|y| {
            (0..width).flat_map(move |x| [(x * 255 / width) as u8, (y * 255 / height) as u8, 160])
        })
        .collect();
    writer.write_image_data(&pixels).unwrap();
    writer.finish().unwrap();
    bytes
}

fn number(object: &Object) -> f32 {
    match object {
        Object::Integer(i) => *i as f32,
        Object::Real(r) => *r,
        other => panic!("expected a number, got {other:?}"),
    }
}

fn numbers(doc: &Document, page_id: ObjectId, key: &[u8]) -> Vec<f32> {
    doc.get_dictionary(page_id)
        .unwrap()
        .get(key)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(number)
        .collect()
}

/// (uri, [x0, y0, x1, y1]) for every link annotation on a page, in PDF space (y up).
fn link_annotations(doc: &Document, page_id: ObjectId) -> Vec<(String, [f32; 4])> {
    let page = doc.get_dictionary(page_id).unwrap();
    let Ok(annots) = page.get(b"Annots") else {
        return Vec::new();
    };
    let annots = match annots {
        Object::Array(items) => items.clone(),
        Object::Reference(id) => doc.get_object(*id).unwrap().as_array().unwrap().clone(),
        other => panic!("unexpected /Annots {other:?}"),
    };
    annots
        .iter()
        .map(|annot| {
            let dict = doc.dereference(annot).unwrap().1.as_dict().unwrap().clone();
            let rect: Vec<f32> = dict
                .get(b"Rect")
                .unwrap()
                .as_array()
                .unwrap()
                .iter()
                .map(number)
                .collect();
            let action = doc
                .dereference(dict.get(b"A").unwrap())
                .unwrap()
                .1
                .as_dict()
                .unwrap()
                .clone();
            let uri = match action.get(b"URI").unwrap() {
                Object::String(bytes, _) => String::from_utf8(bytes.clone()).unwrap(),
                other => panic!("unexpected /URI {other:?}"),
            };
            (uri, [rect[0], rect[1], rect[2], rect[3]])
        })
        .collect()
}

/// Image XObjects a page's content can draw, through its resources and any nested forms.
fn image_count(doc: &Document, page_id: ObjectId) -> usize {
    fn count(doc: &Document, resources: &lopdf::Dictionary) -> usize {
        let Ok(xobjects) = resources
            .get(b"XObject")
            .and_then(|x| doc.dereference(x))
            .and_then(|(_, x)| x.as_dict())
        else {
            return 0;
        };
        xobjects
            .iter()
            .map(|(_, reference)| {
                let stream = doc.dereference(reference).unwrap().1.as_stream().unwrap();
                match stream.dict.get(b"Subtype").unwrap().as_name().unwrap() {
                    b"Image" => 1,
                    _ => stream
                        .dict
                        .get(b"Resources")
                        .ok()
                        .and_then(|r| doc.dereference(r).ok())
                        .and_then(|(_, r)| r.as_dict().ok())
                        .map_or(0, |r| count(doc, r)),
                }
            })
            .sum()
    }
    // A page's resources can be inline or an indirect reference (krilla writes a reference).
    let (inline, referenced) = doc.get_page_resources(page_id).unwrap();
    inline.map_or(0, |r| count(doc, r))
        + referenced
            .iter()
            .map(|id| count(doc, doc.get_dictionary(*id).unwrap()))
            .sum::<usize>()
}

#[test]
fn renders_three_a4_pages() {
    let doc =
        load(&render(&fixture("paragraphs.html"), &config(), no_images).expect("render succeeds"));
    let pages = doc.get_pages();
    assert_eq!(pages.len(), 3);
    for page_id in pages.values() {
        let media_box = numbers(&doc, *page_id, b"MediaBox");
        assert!((media_box[2] - 595.28).abs() < 0.01 && (media_box[3] - 841.89).abs() < 0.01);
    }
}

#[test]
fn links_are_clickable_annotations_inside_the_margins() {
    let doc = load(&render(&fixture("paragraphs.html"), &config(), no_images).unwrap());
    let all: Vec<(String, [f32; 4])> = doc
        .get_pages()
        .values()
        .flat_map(|page_id| link_annotations(&doc, *page_id))
        .collect();

    let wrapped = all
        .iter()
        .filter(|(uri, _)| uri == "https://example.com/wrapped-link")
        .count();
    let short = all
        .iter()
        .filter(|(uri, _)| uri == "https://example.com/short")
        .count();
    assert_eq!(wrapped, 2, "the wrapped link has one annotation per line");
    assert_eq!(short, 1);

    // paragraphs.html: A4 with a 37.5pt margin.
    let (left, right, bottom, top) = (37.5, 595.28 - 37.5, 37.5, 841.89 - 37.5);
    for (uri, [x0, y0, x1, y1]) in &all {
        assert!(x0 < x1 && y0 < y1, "{uri}: empty rect");
        assert!(
            *x0 >= left - 0.01 && *x1 <= right + 0.01,
            "{uri}: outside horizontal margins"
        );
        assert!(
            *y0 >= bottom - 0.01 && *y1 <= top + 0.01,
            "{uri}: outside vertical margins"
        );
    }
}

#[test]
fn a_named_page_gets_its_full_page_background_and_forced_breaks() {
    let mut requested = Vec::new();
    let bytes = render(&fixture("background-page.html"), &config(), |src| {
        requested.push(src.to_string());
        (src == "bg.png").then(|| test_png(120, 160))
    })
    .unwrap();
    assert_eq!(
        requested,
        ["bg.png"],
        "only the referenced background is requested"
    );
    let doc = load(&bytes);
    let pages: Vec<ObjectId> = doc.get_pages().values().copied().collect();
    assert_eq!(pages.len(), 3, "breaks before and after the named page");
    let images: Vec<usize> = pages.iter().map(|id| image_count(&doc, *id)).collect();
    assert_eq!(
        images,
        [0, 1, 0],
        "only the named page has the background image"
    );
}

#[test]
fn the_html_title_becomes_the_pdf_title() {
    let doc = load(
        &render(
            "<title>\n  A  guide &amp; more </title><p>Text</p>",
            &config(),
            no_images,
        )
        .unwrap(),
    );
    let info = doc
        .trailer
        .get(b"Info")
        .and_then(|info| doc.dereference(info))
        .unwrap()
        .1
        .as_dict()
        .unwrap();
    let title = match info.get(b"Title").unwrap() {
        Object::String(bytes, _) => bytes.clone(),
        other => panic!("unexpected /Title {other:?}"),
    };
    let text = match title.strip_prefix(&[0xFE, 0xFF]) {
        Some(utf16) => String::from_utf16(
            &utf16
                .chunks(2)
                .map(|c| u16::from_be_bytes([c[0], c[1]]))
                .collect::<Vec<_>>(),
        )
        .unwrap(),
        None => String::from_utf8(title).unwrap(),
    };
    assert_eq!(text, "A guide & more");
}

/// Embedded font programs: one per face the document draws with.
fn embedded_fonts(doc: &Document) -> usize {
    doc.objects
        .values()
        .filter_map(|object| object.as_dict().ok())
        .filter(|dict| dict.get(b"Type").and_then(Object::as_name).ok() == Some(b"FontDescriptor"))
        .count()
}

/// Text rendering modes (`Tr`) set anywhere in the page contents; 2 is fill then stroke.
fn text_render_modes(doc: &Document) -> Vec<i64> {
    doc.get_pages()
        .values()
        .flat_map(|page_id| {
            doc.get_and_decode_page_content(*page_id)
                .unwrap()
                .operations
        })
        .filter(|op| op.operator == "Tr")
        .map(|op| op.operands[0].as_i64().unwrap())
        .collect()
}

#[test]
fn bold_text_embeds_the_bold_face() {
    let fonts = Config::new(vec![
        std::fs::read(format!("{FIXTURES}/DejaVuSerif.ttf")).unwrap(),
        std::fs::read(format!("{FIXTURES}/DejaVuSerif-Bold.ttf")).unwrap(),
    ]);
    let doc = load(&render(&fixture("bold.html"), &fonts, no_images).unwrap());
    assert_eq!(embedded_fonts(&doc), 2, "regular and bold");
    assert!(
        !text_render_modes(&doc).contains(&2),
        "real bold is not stroked"
    );
}

#[test]
fn bold_text_is_stroked_without_a_bold_face() {
    let doc = load(&render(&fixture("bold.html"), &config(), no_images).unwrap());
    assert_eq!(embedded_fonts(&doc), 1, "the regular face only");
    assert!(
        text_render_modes(&doc).contains(&2),
        "synthesized bold fills and strokes the glyphs"
    );
}

fn faces(names: &[&str]) -> Config {
    Config::new(
        names
            .iter()
            .map(|name| std::fs::read(format!("{FIXTURES}/{name}")).unwrap())
            .collect(),
    )
}

/// Transforms (`cm`) in the page contents that skew x by y, as a synthesized italic does.
fn skews(doc: &Document) -> usize {
    doc.get_pages()
        .values()
        .flat_map(|page_id| {
            doc.get_and_decode_page_content(*page_id)
                .unwrap()
                .operations
        })
        .filter(|op| op.operator == "cm" && number(&op.operands[2]) != 0.0)
        .count()
}

#[test]
fn italic_text_embeds_the_italic_faces() {
    let config = faces(&[
        "DejaVuSerif.ttf",
        "DejaVuSerif-Bold.ttf",
        "DejaVuSerif-Italic.ttf",
        "DejaVuSerif-BoldItalic.ttf",
    ]);
    let doc = load(&render(&fixture("italic.html"), &config, no_images).unwrap());
    assert_eq!(embedded_fonts(&doc), 3, "regular, italic and bold italic");
    assert_eq!(skews(&doc), 0, "real italics are not slanted");
}

#[test]
fn italic_text_is_slanted_without_italic_faces() {
    let doc = load(&render(&fixture("italic.html"), &config(), no_images).unwrap());
    assert_eq!(embedded_fonts(&doc), 1, "the regular face only");
    assert_eq!(skews(&doc), 4, "emphasis, italic, bold italic and the link");
}

#[test]
fn a_missing_image_is_an_error() {
    let result = render(&fixture("background-page.html"), &config(), no_images);
    assert!(matches!(result, Err(Error::MissingImage(src)) if src == "bg.png"));
}

// Text extraction is verified in `paint::tests` (glyph text ranges cover every line) and with
// MuPDF; lopdf 0.38 cannot parse the ToUnicode CMaps krilla writes, so it can't check it here.
