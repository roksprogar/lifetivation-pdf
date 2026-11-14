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

/// The text-level part of a style, carried per span into parley.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextStyle {
    /// CSS `font-family` names, in order of preference.
    pub font_family: Vec<String>,
    /// Points.
    pub font_size: f32,
    pub font_weight: f32,
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
        }
    }

    pub fn compute(parent: &Style, specified: &Specified) -> Self {
        let mut style = Style::inherit(parent);
        let get = |name: &str| specified.get(name).map(String::as_str);

        style.display = match get("display") {
            Some("block") => Display::Block,
            Some("none") => Display::None,
            _ => Display::Inline,
        };

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
        if let Some(value) = get("font-weight") {
            style.text.font_weight = match value {
                "normal" => 400.0,
                "bold" => 700.0,
                "bolder" => (parent.text.font_weight + 300.0).min(900.0),
                "lighter" => (parent.text.font_weight - 300.0).max(100.0),
                number => number.parse().unwrap_or(parent.text.font_weight),
            };
        }
        if let Some(color) = get("color").and_then(css::color) {
            style.text.color = color;
        }
        // Underlines propagate to descendants in CSS; `none` can't remove an ancestor's line,
        // only stop this element from adding one.
        if get("text-decoration").is_some_and(|v| v.split_whitespace().any(|w| w == "underline")) {
            style.text.underline = true;
        }
        if let Some(value) = get("line-height") {
            style.text.line_height = match value {
                "normal" => LineHeight::Normal,
                value => match value.parse::<f32>() {
                    Ok(factor) => LineHeight::Factor(factor),
                    Err(_) => match css::length(value, font_size) {
                        Some(Length::Points(points)) => LineHeight::Points(points),
                        Some(Length::Percent(percent)) => {
                            LineHeight::Points(font_size * percent / 100.0)
                        }
                        None => parent.text.line_height,
                    },
                },
            };
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
        style.page = get("page").filter(|v| *v != "auto").map(str::to_string);
        style
    }
}

/// Resolves a length against a containing size, for percentages.
pub(crate) fn resolve(length: Length, containing: f32) -> f32 {
    match length {
        Length::Points(points) => points,
        Length::Percent(percent) => containing * percent / 100.0,
    }
}
