//! A small CSS parser for the subset this crate supports: style rules with compound selectors
//! (`p`, `.cover`, `img.img-fluid`, `#id`, `*`) and `@page` rules, optionally named. Selectors
//! with combinators, attributes or pseudo-classes are dropped, as are other at-rules, so an
//! unsupported rule never matches instead of matching the wrong elements.

use std::collections::HashMap;

/// The user-agent stylesheet: browser defaults for the elements we support.
pub(crate) const USER_AGENT: &str = "
html, body, div, section, article, header, footer, main, nav, aside, p, h1, h2, h3, h4, h5, h6,
blockquote { display: block }
head, style, script, title, meta, link { display: none }
@page { size: A4; margin: 1cm }
body { margin: 8px }
p { margin: 1em 0 }
h1 { font-size: 2em; margin: 0.67em 0; font-weight: bold }
h2 { font-size: 1.5em; margin: 0.83em 0; font-weight: bold }
a { color: #0000ee; text-decoration: underline }
";

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Declaration {
    /// Lowercased property name.
    pub name: String,
    pub value: String,
}

/// A compound selector: optional type, ids and classes, all of which must match.
#[derive(Debug, Clone, PartialEq, Default)]
pub(crate) struct Selector {
    pub tag: Option<String>,
    pub ids: Vec<String>,
    pub classes: Vec<String>,
}

impl Selector {
    fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.is_empty() || text.contains(|c: char| c.is_whitespace() || ">+~[]:()".contains(c)) {
            return None;
        }
        let mut selector = Selector::default();
        let mut rest = text;
        let type_end = rest.find(['.', '#']).unwrap_or(rest.len());
        match &rest[..type_end] {
            "" | "*" => {}
            tag if is_ident(tag) => selector.tag = Some(tag.to_ascii_lowercase()),
            _ => return None,
        }
        rest = &rest[type_end..];
        while let Some(marker) = rest.chars().next() {
            let end = rest[1..].find(['.', '#']).map_or(rest.len(), |i| i + 1);
            let name = &rest[1..end];
            if !is_ident(name) {
                return None;
            }
            match marker {
                '.' => selector.classes.push(name.to_string()),
                _ => selector.ids.push(name.to_string()),
            }
            rest = &rest[end..];
        }
        Some(selector)
    }

    /// (ids, classes, types), compared lexicographically.
    pub fn specificity(&self) -> (usize, usize, usize) {
        (
            self.ids.len(),
            self.classes.len(),
            usize::from(self.tag.is_some()),
        )
    }

    pub fn matches(&self, tag: &str, id: Option<&str>, classes: &[&str]) -> bool {
        self.tag.as_deref().is_none_or(|t| t == tag)
            && self.ids.iter().all(|i| id == Some(i.as_str()))
            && self.classes.iter().all(|c| classes.contains(&c.as_str()))
    }
}

fn is_ident(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

#[derive(Debug, Clone)]
pub(crate) struct StyleRule {
    pub selector: Selector,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone)]
pub(crate) struct PageRule {
    /// `None` for `@page { … }`, the name for `@page name { … }`.
    pub name: Option<String>,
    pub declarations: Vec<Declaration>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Stylesheet {
    /// In source order, one entry per selector of a selector list.
    pub rules: Vec<StyleRule>,
    pub pages: Vec<PageRule>,
}

impl Stylesheet {
    pub fn parse(source: &str) -> Self {
        let source = strip_comments(source);
        let mut sheet = Stylesheet::default();
        let mut rest = source.as_str();
        loop {
            rest = rest.trim_start();
            if rest.is_empty() {
                break;
            }
            if let Some(at_rule) = rest.strip_prefix('@') {
                let keyword_end = at_rule
                    .find(|c: char| !(c.is_alphanumeric() || c == '-'))
                    .unwrap_or(at_rule.len());
                let keyword = at_rule[..keyword_end].to_ascii_lowercase();
                // Statement at-rules (`@import …;`) end at a semicolon, block at-rules at their block.
                let semicolon = at_rule.find(';');
                let brace = at_rule.find('{');
                match (semicolon, brace) {
                    (Some(s), b) if b.is_none_or(|b| s < b) => {
                        rest = &at_rule[s + 1..];
                        continue;
                    }
                    (_, None) => break,
                    (_, Some(open)) => {
                        let close = matching_brace(at_rule, open);
                        let prelude = at_rule[keyword_end..open].trim();
                        let body = &at_rule[open + 1..close.min(at_rule.len())];
                        if keyword == "page"
                            && !prelude.contains(':')
                            && (prelude.is_empty() || is_ident(prelude))
                        {
                            sheet.pages.push(PageRule {
                                name: (!prelude.is_empty()).then(|| prelude.to_string()),
                                declarations: declarations(body),
                            });
                        }
                        rest = at_rule.get(close + 1..).unwrap_or("");
                    }
                }
                continue;
            }
            let Some(open) = rest.find('{') else { break };
            let close = matching_brace(rest, open);
            let body = declarations(&rest[open + 1..close.min(rest.len())]);
            for selector in rest[..open].split(',').filter_map(Selector::parse) {
                sheet.rules.push(StyleRule {
                    selector,
                    declarations: body.clone(),
                });
            }
            rest = rest.get(close + 1..).unwrap_or("");
        }
        sheet
    }
}

fn strip_comments(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut rest = source;
    while let Some(start) = rest.find("/*") {
        out.push_str(&rest[..start]);
        rest = rest[start + 2..]
            .find("*/")
            .map_or("", |end| &rest[start + 2 + end + 2..]);
    }
    out.push_str(rest);
    out
}

/// The index of the `}` closing the block opened at `open`, or `text.len()` if unclosed.
fn matching_brace(text: &str, open: usize) -> usize {
    let mut depth = 0usize;
    let mut quote = None;
    for (i, c) in text[open..].char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '{') => depth += 1,
            (None, '}') => {
                depth -= 1;
                if depth == 0 {
                    return open + i;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// Declarations of a block body. Nested blocks (margin boxes inside `@page`) are skipped;
/// `!important` is accepted and ignored.
pub(crate) fn declarations(body: &str) -> Vec<Declaration> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut depth = 0usize;
    let mut quote = None;
    let mut parens = 0usize;
    let bytes: Vec<(usize, char)> = body.char_indices().collect();
    let push = |text: &str, out: &mut Vec<Declaration>| {
        if let Some((name, value)) = text.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().trim_end_matches("!important").trim();
            if is_ident(&name) && !value.is_empty() {
                out.push(Declaration {
                    name,
                    value: value.to_string(),
                });
            }
        }
    };
    for &(i, c) in &bytes {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => parens += 1,
            (None, ')') => parens = parens.saturating_sub(1),
            (None, '{') => {
                if depth == 0 {
                    start = body.len();
                }
                depth += 1;
            }
            (None, '}') => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    start = i + 1;
                }
            }
            (None, ';') if depth == 0 && parens == 0 => {
                if start <= i {
                    push(&body[start..i], &mut out);
                }
                start = i + 1;
            }
            _ => {}
        }
    }
    if depth == 0 && start < body.len() {
        push(&body[start..], &mut out);
    }
    out
}

/// Declared values for one element, after the cascade: name → value.
pub(crate) type Specified = HashMap<String, String>;

/// Applies the matching rules of each sheet in order (user agent first, then author), each sheet
/// sorted by specificity with source order breaking ties.
pub(crate) fn cascade(
    sheets: &[&Stylesheet],
    tag: &str,
    id: Option<&str>,
    classes: &[&str],
) -> Specified {
    let mut specified = Specified::new();
    for sheet in sheets {
        let mut matching: Vec<(usize, &StyleRule)> = sheet
            .rules
            .iter()
            .enumerate()
            .filter(|(_, rule)| rule.selector.matches(tag, id, classes))
            .collect();
        matching.sort_by_key(|(order, rule)| (rule.selector.specificity(), *order));
        for (_, rule) in matching {
            for declaration in &rule.declarations {
                expand(declaration, &mut specified);
            }
        }
    }
    specified
}

/// Stores a declaration, expanding the `margin`/`padding` shorthands into their longhands.
pub(crate) fn expand(declaration: &Declaration, specified: &mut Specified) {
    let name = declaration.name.as_str();
    if name == "margin" || name == "padding" {
        let parts: Vec<&str> = declaration.value.split_whitespace().collect();
        let [top, right, bottom, left] = match parts.as_slice() {
            [all] => [*all; 4],
            [vertical, horizontal] => [*vertical, *horizontal, *vertical, *horizontal],
            [top, horizontal, bottom] => [*top, *horizontal, *bottom, *horizontal],
            [top, right, bottom, left] => [*top, *right, *bottom, *left],
            _ => return,
        };
        for (side, value) in ["top", "right", "bottom", "left"]
            .iter()
            .zip([top, right, bottom, left])
        {
            specified.insert(format!("{name}-{side}"), value.to_string());
        }
        return;
    }
    if name == "page-break-after" || name == "page-break-before" {
        // Accepted and ignored until a document needs forced breaks.
        return;
    }
    specified.insert(declaration.name.clone(), declaration.value.clone());
}

/// A CSS length resolved against a font size, or a percentage left for the caller.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Length {
    Points(f32),
    Percent(f32),
}

/// Parses `12px`, `1.5em`, `10pt`, `2cm`, `20mm`, `1in`, `50%` or `0`. One CSS px is 0.75pt.
pub(crate) fn length(value: &str, font_size: f32) -> Option<Length> {
    let value = value.trim().to_ascii_lowercase();
    let split = value
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+'))
        .unwrap_or(value.len());
    let number: f32 = value[..split].parse().ok()?;
    let points = match &value[split..] {
        "" if number == 0.0 => 0.0,
        "px" => number * 0.75,
        "pt" => number,
        "em" | "rem" => number * font_size,
        "cm" => number * 72.0 / 2.54,
        "mm" => number * 72.0 / 25.4,
        "in" => number * 72.0,
        "%" => return Some(Length::Percent(number)),
        _ => return None,
    };
    Some(Length::Points(points))
}

/// Parses `#rgb`, `#rrggbb`, `rgb(r, g, b)` and the basic named colours.
pub(crate) fn color(value: &str) -> Option<crate::Rgb> {
    let value = value.trim().to_ascii_lowercase();
    if let Some(hex) = value.strip_prefix('#') {
        let digits: Vec<u8> = hex
            .chars()
            .map(|c| c.to_digit(16).map(|d| d as u8))
            .collect::<Option<_>>()?;
        return match digits.as_slice() {
            [r, g, b] => Some(crate::Rgb(r * 17, g * 17, b * 17)),
            [r1, r2, g1, g2, b1, b2] => Some(crate::Rgb(r1 * 16 + r2, g1 * 16 + g2, b1 * 16 + b2)),
            _ => None,
        };
    }
    if let Some(args) = value.strip_prefix("rgb(").and_then(|v| v.strip_suffix(')')) {
        let parts: Vec<u8> = args
            .split(',')
            .map(|p| p.trim().parse().ok())
            .collect::<Option<_>>()?;
        return match parts.as_slice() {
            [r, g, b] => Some(crate::Rgb(*r, *g, *b)),
            _ => None,
        };
    }
    Some(match value.as_str() {
        "black" => crate::Rgb(0, 0, 0),
        "white" => crate::Rgb(255, 255, 255),
        "red" => crate::Rgb(255, 0, 0),
        "green" => crate::Rgb(0, 128, 0),
        "blue" => crate::Rgb(0, 0, 255),
        "gray" | "grey" => crate::Rgb(128, 128, 128),
        _ => return None,
    })
}

/// The reference inside `url(…)`, with quotes removed.
pub(crate) fn url(value: &str) -> Option<String> {
    let inner = value.trim().strip_prefix("url(")?.strip_suffix(')')?.trim();
    let unquoted = inner
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| inner.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(inner);
    Some(unquoted.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rules_selector_lists_and_named_pages() {
        let sheet = Stylesheet::parse(
            "/* c */ h1, h2 { font-family: 'DejaVu Sans' } img.img-fluid{max-width:100%;height:auto}
             @page { margin: 50px; padding: 0 } @page cover { background-image: url(\"a;b.png\") }
             @page :first { margin: 0 } @media print { p { color: red } } @import 'x.css';
             div p { color: red } a:hover { color: red } .green { color: #95C552 }",
        );
        let selectors: Vec<String> = sheet
            .rules
            .iter()
            .map(|r| format!("{:?}{:?}", r.selector.tag, r.selector.classes))
            .collect();
        assert_eq!(
            selectors,
            [
                "Some(\"h1\")[]",
                "Some(\"h2\")[]",
                "Some(\"img\")[\"img-fluid\"]",
                "None[\"green\"]",
            ]
        );
        assert_eq!(sheet.rules[2].declarations.len(), 2);
        assert_eq!(
            sheet.pages.len(),
            2,
            "@page :first is not supported yet and is dropped"
        );
        assert_eq!(sheet.pages[1].name.as_deref(), Some("cover"));
        assert_eq!(
            url(&sheet.pages[1].declarations[0].value).as_deref(),
            Some("a;b.png")
        );
    }

    #[test]
    fn cascade_orders_by_sheet_then_specificity_then_source_order() {
        let ua = Stylesheet::parse("a { color: blue; text-decoration: underline }");
        let author = Stylesheet::parse(
            "a.x { color: green } a, .green { color: #95C552 } a { text-decoration: none; margin: 1px 2px }",
        );
        let specified = cascade(&[&ua, &author], "a", None, &["x", "green"]);
        assert_eq!(specified["color"], "green", "a.x outranks a and .green");
        assert_eq!(
            specified["text-decoration"], "none",
            "author beats user agent"
        );
        assert_eq!(specified["margin-left"], "2px");
        assert_eq!(specified["margin-bottom"], "1px");
    }

    #[test]
    fn parses_lengths_and_colours() {
        assert_eq!(length("20px", 12.0), Some(Length::Points(15.0)));
        assert_eq!(length("1.5em", 12.0), Some(Length::Points(18.0)));
        assert_eq!(length("-50px", 12.0), Some(Length::Points(-37.5)));
        assert_eq!(length("100%", 12.0), Some(Length::Percent(100.0)));
        assert_eq!(length("0", 12.0), Some(Length::Points(0.0)));
        assert_eq!(length("auto", 12.0), None);
        assert_eq!(color("#95C552"), Some(crate::Rgb(149, 197, 82)));
        assert_eq!(color("#fff"), Some(crate::Rgb(255, 255, 255)));
    }
}
