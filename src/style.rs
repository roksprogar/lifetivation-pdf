//! Computed styles: the cascade's declared values resolved into the values layout uses, with
//! inheritance from the parent.

use crate::css::{self, Length, Specified};
use crate::Rgb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Display {
    Block,
    Inline,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LineHeight {
    /// From the font's metrics (ascent + descent + line gap).
    Normal,
    /// A multiple of the font size; inherited as the factor, like CSS unitless values.
    Factor(f32),
    Points(f32),
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Sides {
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TextAlign {
    Start,
    End,
    Left,
    Right,
    Center,
    Justify,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum FontStyle {
    Normal,
    Italic,
    /// `oblique` with its angle in degrees, if one was given.
    Oblique(Option<f32>),
}

/// The text-level part of a style, carried per span into parley.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextStyle {
    /// CSS `font-family` names, in order of preference.
    pub font_family: Vec<String>,
    /// Points.
    pub font_size: f32,
    pub font_weight: f32,
    pub font_style: FontStyle,
    pub color: Rgb,
    pub underline: bool,
    pub line_height: LineHeight,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Style {
    pub display: Display,
    pub text: TextStyle,
    pub margin: Sides,
    pub padding: Sides,
    pub width: Option<Length>,
    pub height: Option<Length>,
    pub max_width: Option<Length>,
    /// The `page` property: the named page this box must start on.
    pub page: Option<String>,
    /// Inherited; applies to the lines of the paragraphs this box contains.
    pub text_align: TextAlign,
}

impl Style {
    /// The initial style of the root element's parent.
    pub fn initial(default_font_family: &str) -> Self {
        Style {
            display: Display::Inline,
            text: TextStyle {
                font_family: vec![default_font_family.to_string()],
                font_size: 12.0,
                font_weight: 400.0,
                font_style: FontStyle::Normal,
                color: Rgb(0, 0, 0),
                underline: false,
                line_height: LineHeight::Normal,
            },
            margin: Sides::default(),
            padding: Sides::default(),
            width: None,
            height: None,
            max_width: None,
            page: None,
            text_align: TextAlign::Start,
        }
    }

    /// The style of an anonymous box inside `parent`: inherited properties only.
    pub fn anonymous(parent: &Style) -> Self {
        Style {
            display: Display::Block,
            ..Style::inherit(parent)
        }
    }

    fn inherit(parent: &Style) -> Self {
        Style {
            display: Display::Inline,
            text: parent.text.clone(),
            margin: Sides::default(),
            padding: Sides::default(),
            width: None,
            height: None,
            max_width: None,
            page: None,
            text_align: parent.text_align,
        }
    }

    pub fn compute(parent: &Style, specified: &Specified) -> Self {
        let mut style = Style::inherit(parent);
        let get = |name: &str| specified.get(name).map(String::as_str);

        // Unsupported display values (`flex`, `inline-block`) lay out as inline, like
        // `display: inline`; a `style` attribute refuses them in `renders`.
        style.display = get("display")
            .and_then(parse_display)
            .unwrap_or(Display::Inline);

        // font-size first: em lengths on this element resolve against it.
        let parent_size = parent.text.font_size;
        if let Some(value) = get("font-size") {
            style.text.font_size = match css::length(value, parent_size) {
                Some(Length::Points(points)) => points,
                Some(Length::Percent(percent)) => parent_size * percent / 100.0,
                None => parent_size,
            };
        }
        let font_size = style.text.font_size;

        if let Some(value) = get("font-family") {
            let families: Vec<String> = value
                .split(',')
                .map(|f| f.trim().trim_matches(['"', '\'']).to_string())
                .filter(|f| !f.is_empty())
                .collect();
            if !families.is_empty() {
                style.text.font_family = families;
            }
        }
        // An invalid value leaves the inherited one, as CSS ignores invalid declarations.
        if let Some(weight) =
            get("font-weight").and_then(|v| parse_font_weight(v, parent.text.font_weight))
        {
            style.text.font_weight = weight;
        }
        if let Some(font_style) = get("font-style").and_then(parse_font_style) {
            style.text.font_style = font_style;
        }
        if let Some(color) = get("color").and_then(css::color) {
            style.text.color = color;
        }
        // Underlines propagate to descendants in CSS; `none` can't remove an ancestor's line,
        // only stop this element from adding one.
        if get("text-decoration").and_then(parse_underline) == Some(true) {
            style.text.underline = true;
        }
        if let Some(line_height) = get("line-height").and_then(|v| parse_line_height(v, font_size))
        {
            style.text.line_height = line_height;
        }

        let points = |name: &str| match get(name).and_then(|v| css::length(v, font_size)) {
            Some(Length::Points(points)) => points,
            // Percent margins and padding would resolve against the containing block's width,
            // which isn't known here, so they compute to 0.
            _ => 0.0,
        };
        style.margin = Sides {
            top: points("margin-top"),
            right: points("margin-right"),
            bottom: points("margin-bottom"),
            left: points("margin-left"),
        };
        style.padding = Sides {
            top: points("padding-top"),
            right: points("padding-right"),
            bottom: points("padding-bottom"),
            left: points("padding-left"),
        };
        style.width = get("width").and_then(|v| css::length(v, font_size));
        style.height = get("height").and_then(|v| css::length(v, font_size));
        style.max_width = get("max-width").and_then(|v| css::length(v, font_size));
        // Page names are case-sensitive, like other author-defined names; `auto` is a keyword.
        style.page = get("page")
            .filter(|v| !v.eq_ignore_ascii_case("auto"))
            .map(str::to_string);
        style.text_align = get("text-align")
            .and_then(parse_text_align)
            .unwrap_or(parent.text_align);
        style
    }
}

// Value parsers shared by `Style::compute` and `renders`, so what a `style` attribute accepts is
// exactly what renders. CSS keywords match in any case (`Center`, `ITALIC`).

fn parse_display(value: &str) -> Option<Display> {
    match value.to_ascii_lowercase().as_str() {
        "block" => Some(Display::Block),
        "inline" => Some(Display::Inline),
        "none" => Some(Display::None),
        _ => None,
    }
}

/// `bolder` and `lighter` step from the parent's weight by the CSS Fonts table.
fn parse_font_weight(value: &str, parent: f32) -> Option<f32> {
    match value.to_ascii_lowercase().as_str() {
        "normal" => Some(400.0),
        "bold" => Some(700.0),
        "bolder" => Some(match parent {
            w if w < 350.0 => 400.0,
            w if w < 550.0 => 700.0,
            w if w < 900.0 => 900.0,
            w => w,
        }),
        "lighter" => Some(match parent {
            w if w < 100.0 => w,
            w if w < 550.0 => 100.0,
            w if w < 750.0 => 400.0,
            _ => 700.0,
        }),
        number => number
            .parse::<f32>()
            .ok()
            .filter(|weight| (1.0..=1000.0).contains(weight)),
    }
}

/// `oblique` takes an angle in degrees strictly between -90 and 90: CSS allows ±90 itself,
/// but a 90° slant is infinitely wide, so it is refused rather than drawn off the page.
fn parse_font_style(value: &str) -> Option<FontStyle> {
    let value = value.to_ascii_lowercase();
    let words: Vec<&str> = value.split_whitespace().collect();
    match words.as_slice() {
        ["normal"] => Some(FontStyle::Normal),
        ["italic"] => Some(FontStyle::Italic),
        ["oblique"] => Some(FontStyle::Oblique(None)),
        ["oblique", angle] => angle
            .strip_suffix("deg")
            .and_then(|degrees| degrees.parse::<f32>().ok())
            .filter(|degrees| degrees.abs() < 90.0)
            .map(|degrees| FontStyle::Oblique(Some(degrees))),
        _ => None,
    }
}

/// Whether `text-decoration` underlines; `None` for values other than `none` and `underline`.
fn parse_underline(value: &str) -> Option<bool> {
    let value = value.to_ascii_lowercase();
    let words: Vec<&str> = value.split_whitespace().collect();
    (!words.is_empty() && words.iter().all(|w| matches!(*w, "none" | "underline")))
        .then(|| words.contains(&"underline"))
}

fn parse_line_height(value: &str, font_size: f32) -> Option<LineHeight> {
    if value.eq_ignore_ascii_case("normal") {
        return Some(LineHeight::Normal);
    }
    if let Ok(factor) = value.trim().parse::<f32>() {
        return Some(LineHeight::Factor(factor));
    }
    match css::length(value, font_size)? {
        Length::Points(points) => Some(LineHeight::Points(points)),
        Length::Percent(percent) => Some(LineHeight::Points(font_size * percent / 100.0)),
    }
}

fn parse_text_align(value: &str) -> Option<TextAlign> {
    match value.to_ascii_lowercase().as_str() {
        "start" => Some(TextAlign::Start),
        "end" => Some(TextAlign::End),
        "left" => Some(TextAlign::Left),
        "right" => Some(TextAlign::Right),
        "center" => Some(TextAlign::Center),
        "justify" => Some(TextAlign::Justify),
        _ => None,
    }
}

/// Whether this declaration renders as a browser would on a `tag` element displayed as `display`.
/// Keep in step with `compute` and layout: anything they would drop or misread is `false`.
pub(crate) fn renders(tag: &str, display: Display, name: &str, value: &str) -> bool {
    // Layout spaces block boxes and images; inline boxes (`<span>`, `<a>`) get no margins or
    // padding, and images no padding.
    let margins = display == Display::Block || tag == "img";
    let padding = display == Display::Block && tag != "img";
    let words: Vec<&str> = value.split_whitespace().collect();
    // Percent margins and padding compute to 0, so only absolute lengths render.
    let absolute = |v: &str| matches!(css::length(v, 12.0), Some(Length::Points(_)));
    let length = |v: &str| css::length(v, 12.0).is_some();
    let side = |prefix: &str| {
        name.strip_prefix(prefix)
            .is_some_and(|side| matches!(side, "top" | "right" | "bottom" | "left"))
    };
    match name {
        "display" => parse_display(value).is_some(),
        "font-family" => value
            .split(',')
            .any(|f| !f.trim().trim_matches(['"', '\'']).is_empty()),
        "font-size" => length(value),
        "font-weight" => parse_font_weight(value, 400.0).is_some(),
        "font-style" => parse_font_style(value).is_some(),
        "color" => css::color(value).is_some(),
        "text-decoration" => parse_underline(value).is_some(),
        "line-height" => parse_line_height(value, 12.0).is_some(),
        "margin" | "padding" => {
            (if name == "margin" { margins } else { padding })
                && (1..=4).contains(&words.len())
                && words.iter().all(|w| absolute(w))
        }
        _ if side("margin-") => margins && absolute(value),
        _ if side("padding-") => padding && absolute(value),
        // Layout sizes images only; other boxes take the width of their container.
        "width" | "height" => tag == "img" && (value.eq_ignore_ascii_case("auto") || length(value)),
        "max-width" => tag == "img" && (value.eq_ignore_ascii_case("none") || length(value)),
        "page" => css::is_ident(value),
        "text-align" => parse_text_align(value).is_some(),
        _ => false,
    }
}

/// Resolves a length against a containing size, for percentages.
pub(crate) fn resolve(length: Length, containing: f32) -> f32 {
    match length {
        Length::Points(points) => points,
        Length::Percent(percent) => containing * percent / 100.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bolder_and_lighter_follow_the_css_fonts_table() {
        let step = |keyword: &str, parent: f32| parse_font_weight(keyword, parent).unwrap();
        let parents = [
            50.0, 100.0, 300.0, 400.0, 500.0, 600.0, 700.0, 800.0, 900.0, 950.0,
        ];
        let bolder: Vec<f32> = parents.iter().map(|w| step("bolder", *w)).collect();
        let lighter: Vec<f32> = parents.iter().map(|w| step("lighter", *w)).collect();
        assert_eq!(
            bolder,
            [400.0, 400.0, 400.0, 700.0, 700.0, 900.0, 900.0, 900.0, 900.0, 950.0]
        );
        assert_eq!(
            lighter,
            [50.0, 100.0, 100.0, 100.0, 100.0, 400.0, 400.0, 700.0, 700.0, 700.0]
        );
        assert_eq!(parse_font_weight("1001", 400.0), None);
        assert_eq!(parse_font_weight("0", 400.0), None);
    }

    #[test]
    fn keywords_match_in_any_case_and_names_keep_theirs() {
        assert_eq!(parse_display("BLOCK"), Some(Display::Block));
        assert_eq!(parse_font_weight("Bold", 400.0), Some(700.0));
        assert_eq!(
            parse_font_style("Oblique 10DEG"),
            Some(FontStyle::Oblique(Some(10.0)))
        );
        assert_eq!(parse_underline("UnderLine"), Some(true));
        assert_eq!(parse_line_height("NORMAL", 12.0), Some(LineHeight::Normal));
        assert_eq!(parse_text_align("Center"), Some(TextAlign::Center));
        let mut specified = Specified::new();
        specified.insert("page".into(), "Cover".into());
        let style = Style::compute(&Style::initial("Test"), &specified);
        assert_eq!(
            style.page.as_deref(),
            Some("Cover"),
            "page names are case-sensitive"
        );
        specified.insert("page".into(), "AUTO".into());
        assert_eq!(
            Style::compute(&Style::initial("Test"), &specified).page,
            None
        );
    }

    #[test]
    fn oblique_angles_must_be_strictly_within_90_degrees() {
        assert_eq!(
            parse_font_style("oblique -89.5deg"),
            Some(FontStyle::Oblique(Some(-89.5)))
        );
        for invalid in [
            "oblique 90deg",
            "oblique -90deg",
            "oblique 200deg",
            "oblique 10rad",
        ] {
            assert_eq!(parse_font_style(invalid), None, "{invalid}");
            assert!(
                !renders("p", Display::Block, "font-style", invalid),
                "{invalid}"
            );
        }
        let mut specified = Specified::new();
        specified.insert("font-style".into(), "oblique 120deg".into());
        let mut parent = Style::initial("Test");
        parent.text.font_style = FontStyle::Italic;
        assert_eq!(
            Style::compute(&parent, &specified).text.font_style,
            FontStyle::Italic,
            "an invalid angle keeps the inherited style"
        );
    }
}
