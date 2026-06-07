//! Pure structuring of raw pdfium segments into ordered, classified, cleaned
//! text lines, plus a quality report.
//!
//! No PDF I/O happens here — the input is [`RawSegment`]s — so every heuristic
//! (reading order, heading detection, caption-bleed and garble filtering) is
//! unit-testable against hand-built segments. The signals are *relative to the
//! document* (dominant font = body, rarer fonts = headings, symbolic fonts =
//! icons) rather than tied to any one game's typography.

use std::collections::HashMap;

use crate::pdf::RawSegment;

/// Two segments belong to the same visual line when their baselines (segment
/// bottoms) sit within this fraction of the taller segment's height.
const LINE_BASELINE_TOLERANCE: f32 = 0.6;

/// A line needs at least this many alphabetic characters to be considered a
/// heading, so list markers ("1."), bullets and stray glyphs never are.
const MIN_HEADING_LETTERS: usize = 3;

/// A heading is a short title; anything longer is body text that merely happens
/// to use a display font (e.g. an italic intro paragraph).
const MAX_HEADING_CHARS: usize = 48;

/// Minimum fraction of cased letters that must be uppercase for a line to count
/// as a heading. Real headings are set in capitals across the documents tested;
/// sentence-case labels in display fonts are not.
const HEADING_UPPERCASE_RATIO: f32 = 0.8;

/// A line is treated as icons when at least this fraction of its non-space
/// characters are icon glyphs (Private Use Area codepoints).
const ICON_CHAR_RATIO: f32 = 0.6;

/// Above this fraction of "weird" characters, a body line is considered garbled
/// (a broken font subset) and dropped rather than embedded as noise.
const GARBLE_SYMBOL_RATIO: f32 = 0.30;

/// What a reconstructed line is, inferred from relative font usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    /// A section/entry title: a short, predominantly uppercase line in a font
    /// family distinct from the body.
    Heading,
    /// Ordinary running text.
    Body,
    /// Mostly icon glyphs (e.g. resource/stat symbols).
    Icons,
}

/// A single reconstructed line of text with its inferred role and source page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// 1-based page number the line came from.
    pub page: usize,
    pub kind: LineKind,
    pub text: String,
}

/// Counts of what survived extraction, so callers can surface (and log) how
/// much content was unusable instead of silently dropping it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QualityReport {
    pub kept: usize,
    pub dropped_garbled: usize,
}

/// The structured result of extracting a document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedDocument {
    pub lines: Vec<Line>,
    pub report: QualityReport,
}

impl ExtractedDocument {
    /// Renders the document as a Markdown transcript for human review: headings
    /// become `##`, body and icon lines stay as text, and a `<!-- page N -->`
    /// marker is emitted whenever the source page changes (provenance for the
    /// reviewer). This is the editable artifact the later steps consume.
    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let mut page = 0;
        for line in &self.lines {
            if line.page != page {
                page = line.page;
                out.push_str(&format!("\n<!-- page {page} -->\n"));
            }
            match line.kind {
                LineKind::Heading => out.push_str(&format!("\n## {}\n\n", line.text)),
                LineKind::Body | LineKind::Icons => {
                    out.push_str(&line.text);
                    out.push('\n');
                }
            }
        }
        out
    }
}

/// Turns the raw per-page segments of a whole document into ordered, classified,
/// cleaned lines plus a quality report.
pub fn structure(pages: &[Vec<RawSegment>]) -> ExtractedDocument {
    let body_family = dominant_body_family(pages);

    let mut lines = Vec::new();
    let mut report = QualityReport::default();

    for (index, page) in pages.iter().enumerate() {
        let page_number = index + 1;

        for group in group_into_lines(page) {
            let text = assemble_text(group);
            if text.is_empty() {
                continue;
            }

            let kind = classify(group, &body_family);

            // Icon lines are expected to look "weird"; only garble-check prose.
            if kind != LineKind::Icons && is_garbled(&text) {
                report.dropped_garbled += 1;
                continue;
            }

            report.kept += 1;
            lines.push(Line {
                page: page_number,
                kind,
                text,
            });
        }
    }

    ExtractedDocument { lines, report }
}

/// The font family covering the most non-icon characters across the document;
/// this is the body text family, against which heading families stand out.
fn dominant_body_family(pages: &[Vec<RawSegment>]) -> String {
    let mut chars_per_family: HashMap<&str, usize> = HashMap::new();
    for page in pages {
        for segment in page {
            let family = font_family(&segment.font);
            if family.is_empty() {
                continue;
            }
            let letters = segment.text.chars().filter(|c| !is_icon_glyph(*c)).count();
            *chars_per_family.entry(family).or_default() += letters;
        }
    }

    chars_per_family
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(family, _)| family.to_owned())
        .unwrap_or_default()
}

/// The font family of a name: its subset prefix (six uppercase letters then `+`)
/// and style suffix (`-Book`, `-BookOblique`, …) stripped, so all weights and
/// styles of one face — `AMVWYI+Avenir-Book`, `Avenir-BookOblique` — collapse to
/// `Avenir`. Headings differ from the body by family, not merely by style.
fn font_family(font: &str) -> &str {
    let without_prefix = match font.split_once('+') {
        Some((prefix, rest))
            if prefix.len() == 6 && prefix.bytes().all(|b| b.is_ascii_uppercase()) =>
        {
            rest
        }
        _ => font,
    };
    without_prefix
        .split_once('-')
        .map_or(without_prefix, |(family, _)| family)
}

/// Whether a character is an icon glyph, i.e. lives in a Private Use Area where
/// icon fonts map their symbols.
fn is_icon_glyph(c: char) -> bool {
    ('\u{e000}'..='\u{f8ff}').contains(&c)
}

/// Groups a page's segments into visual lines.
///
/// Relies on pdfium emitting segments column-by-column, top-to-bottom: a line is
/// a maximal run of consecutive segments whose baselines stay within tolerance.
/// When the column changes, the baseline jumps back up, breaking the run — so we
/// never merge across columns without explicit column detection.
fn group_into_lines(page: &[RawSegment]) -> Vec<&[RawSegment]> {
    let mut lines = Vec::new();
    let mut start = 0;

    for i in 1..page.len() {
        let prev = &page[i - 1];
        let curr = &page[i];
        let tolerance = prev.height().max(curr.height()) * LINE_BASELINE_TOLERANCE;
        if (curr.bottom - prev.bottom).abs() > tolerance {
            lines.push(&page[start..i]);
            start = i;
        }
    }

    if start < page.len() {
        lines.push(&page[start..]);
    }

    lines
}

/// Joins a line's segments left-to-right into a single cleaned string.
fn assemble_text(group: &[RawSegment]) -> String {
    let mut ordered: Vec<&RawSegment> = group.iter().collect();
    ordered.sort_by(|a, b| a.left.total_cmp(&b.left));

    let mut text = String::new();
    for segment in ordered {
        text.push_str(clean_segment_text(&segment.text));
    }

    normalize_whitespace(&text)
}

/// Drops caption bleed: pdfium sometimes appends fragments of nearby card-art
/// labels after a carriage return / newline inside an otherwise clean run, so we
/// keep only the text up to the first such break.
fn clean_segment_text(text: &str) -> &str {
    match text.find(['\r', '\n']) {
        Some(cut) => &text[..cut],
        None => text,
    }
}

/// Collapses runs of whitespace into single spaces and trims the ends.
fn normalize_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Classifies a line from its icon content and relative font family.
fn classify(group: &[RawSegment], body_family: &str) -> LineKind {
    let mut non_space = 0usize;
    let mut icons = 0usize;
    for c in group.iter().flat_map(|s| s.text.chars()) {
        if c.is_whitespace() {
            continue;
        }
        non_space += 1;
        if is_icon_glyph(c) {
            icons += 1;
        }
    }
    if non_space == 0 {
        return LineKind::Body;
    }
    if icons as f32 / non_space as f32 >= ICON_CHAR_RATIO {
        return LineKind::Icons;
    }

    // A heading must clear several signals at once, so distinct-font but
    // sentence-case labels (component callouts, captions, running headers) are
    // not mistaken for titles:
    //
    //  - a family distinct from the body (list markers/bullets/italic body share
    //    their line's family with the body and so never qualify);
    //  - short, with enough letters to be a real word, not a marker like "1.";
    //  - predominantly uppercase, the one signal that held across every tested
    //    document's real headings while sentence-case labels failed it.
    let line_family = dominant_family(group);
    let letters = group
        .iter()
        .flat_map(|s| s.text.chars())
        .filter(|c| c.is_alphabetic())
        .count();
    let char_len = group.iter().flat_map(|s| s.text.chars()).count();

    let is_heading = line_family.is_some_and(|f| f != body_family)
        && letters >= MIN_HEADING_LETTERS
        && char_len <= MAX_HEADING_CHARS
        && is_mostly_uppercase(group);
    if is_heading {
        LineKind::Heading
    } else {
        LineKind::Body
    }
}

/// The font family covering the most non-icon characters in a single line.
fn dominant_family(group: &[RawSegment]) -> Option<&str> {
    let mut chars_per_family: HashMap<&str, usize> = HashMap::new();
    for segment in group {
        let family = font_family(&segment.font);
        if family.is_empty() {
            continue;
        }
        let letters = segment.text.chars().filter(|c| !is_icon_glyph(*c)).count();
        *chars_per_family.entry(family).or_default() += letters;
    }
    chars_per_family
        .into_iter()
        .max_by_key(|&(_, count)| count)
        .map(|(family, _)| family)
}

/// Whether a line is predominantly uppercase among its cased letters. Real
/// headings are set in capitals; sentence-case labels (e.g. "48 tech tiles")
/// are not, which is what separates the two across publishers.
fn is_mostly_uppercase(group: &[RawSegment]) -> bool {
    let mut upper = 0usize;
    let mut cased = 0usize;
    for c in group.iter().flat_map(|s| s.text.chars()) {
        if c.is_uppercase() {
            upper += 1;
            cased += 1;
        } else if c.is_lowercase() {
            cased += 1;
        }
    }
    cased > 0 && (upper as f32 / cased as f32) >= HEADING_UPPERCASE_RATIO
}

/// Detects text mangled by a broken font subset (no usable ToUnicode mapping):
/// C0/C1 control characters or the replacement character are sure signs, and a
/// high ratio of stray symbols among the content is a softer one.
fn is_garbled(text: &str) -> bool {
    let mut weird = 0usize;
    let mut total = 0usize;

    for c in text.chars() {
        if c.is_whitespace() {
            continue;
        }
        total += 1;

        // C0 (except whitespace, already skipped) and C1 control ranges, plus
        // the Unicode replacement character, are never legitimate body text.
        if c.is_control() || c == '\u{fffd}' || ('\u{80}'..='\u{9f}').contains(&c) {
            return true;
        }

        if !is_plausible_text_char(c) {
            weird += 1;
        }
    }

    total > 0 && (weird as f32 / total as f32) > GARBLE_SYMBOL_RATIO
}

/// Whether a character plausibly belongs to clean prose (any language) rather
/// than being extraction noise.
fn is_plausible_text_char(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(
            c,
            '.' | ','
                | ';'
                | ':'
                | '!'
                | '?'
                | '\''
                | '"'
                | '('
                | ')'
                | '['
                | ']'
                | '-'
                | '–'
                | '—'
                | '…'
                | '“'
                | '”'
                | '‘'
                | '’'
                | '/'
                | '&'
                | '%'
                | '+'
                | '='
                | '°'
                | '#'
                | '*'
                | '@'
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a segment at the given baseline and horizontal span.
    fn seg(text: &str, left: f32, bottom: f32, font: &str) -> RawSegment {
        RawSegment {
            text: text.to_owned(),
            left,
            right: left + 10.0,
            top: bottom + 9.0,
            bottom,
            font: font.to_owned(),
        }
    }

    #[test]
    fn groups_segments_sharing_a_baseline_into_one_line() {
        let page = vec![
            seg("Hello ", 70.0, 700.0, "Body"),
            seg("world", 110.0, 700.4, "Body"),
            seg("next line", 70.0, 688.0, "Body"),
        ];
        let groups = group_into_lines(&page);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].len(), 2);
        assert_eq!(groups[1].len(), 1);
    }

    #[test]
    fn assembles_and_normalizes_line_text_in_x_order() {
        let group = vec![
            seg("world", 110.0, 700.0, "Body"),
            seg("Hello  ", 70.0, 700.0, "Body"),
        ];
        assert_eq!(assemble_text(&group), "Hello world");
    }

    #[test]
    fn strips_caption_bleed_after_a_line_break() {
        assert_eq!(
            clean_segment_text("undefended damage from \r\nvo"),
            "undefended damage from "
        );
    }

    #[test]
    fn classifies_standalone_distinct_font_as_heading() {
        let group = vec![seg("ATTACKS AGAINST ALLIES", 70.0, 410.0, "ExoMVC-Bold")];
        assert_eq!(classify(&group, "Avenir"), LineKind::Heading);
    }

    #[test]
    fn classifies_marker_plus_body_as_body() {
        // A numbered marker in a heading-ish font shares its line with body text.
        let group = vec![
            seg("1. ", 88.0, 578.0, "Marker"),
            seg("The retaliate X keyword applies here", 106.0, 578.0, "Body"),
        ];
        assert_eq!(classify(&group, "Body"), LineKind::Body);
    }

    #[test]
    fn short_distinct_font_run_is_not_a_heading() {
        let group = vec![seg("1.", 88.0, 578.0, "Marker")];
        assert_eq!(classify(&group, "Body"), LineKind::Body);
    }

    #[test]
    fn classifies_pua_glyph_run_as_icons() {
        let icon = seg(
            "\u{f521}\u{f522}\u{f526}",
            70.0,
            500.0,
            "BIJDRT+MarvelLCGIcons",
        );
        assert_eq!(classify(&[icon], "Avenir-Book"), LineKind::Icons);
    }

    #[test]
    fn font_family_strips_subset_prefix_and_style_suffix() {
        assert_eq!(font_family("AMVWYI+Avenir-Book"), "Avenir");
        assert_eq!(font_family("Avenir-BookOblique"), "Avenir");
        assert_eq!(font_family("ExoMVC-Bold-SC700"), "ExoMVC");
        // Not a 6-letter subset tag: only the style suffix is dropped.
        assert_eq!(font_family("Foo+Bar-Bold"), "Foo+Bar");
    }

    #[test]
    fn italic_body_variant_is_not_a_heading() {
        // Same family as the body, different style and a different subset prefix.
        let group = vec![seg(
            "For the first game, we recommend playing with two players.",
            70.0,
            700.0,
            "ABOUFB+Avenir-BookOblique",
        )];
        assert_eq!(classify(&group, "Avenir"), LineKind::Body);
    }

    #[test]
    fn sentence_case_distinct_font_label_is_not_a_heading() {
        // A SETI-style component label: short, distinct font, but sentence case.
        let group = vec![seg(
            "the main deck of 138 cards",
            70.0,
            700.0,
            "Label-Medium",
        )];
        assert_eq!(classify(&group, "Avenir"), LineKind::Body);
    }

    #[test]
    fn accented_uppercase_heading_is_detected() {
        // French headings use accented capitals and guillemets.
        let group = vec![seg("LES RÈGLES D’OR", 70.0, 700.0, "ExoMVC-Bold")];
        assert_eq!(classify(&group, "Avenir"), LineKind::Heading);
    }

    #[test]
    fn long_distinct_family_line_is_body_not_heading() {
        // A display font, but far too long to be a title.
        let group = vec![seg(
            "This caption runs well past the heading length cap so it stays body",
            70.0,
            700.0,
            "KomikaTitle-SC850",
        )];
        assert_eq!(classify(&group, "Avenir"), LineKind::Body);
    }

    #[test]
    fn flags_control_char_garble() {
        assert!(is_garbled("/o \u{93}et u\u{ab} the wr\u{c3}t"));
    }

    #[test]
    fn clean_prose_is_not_garbled() {
        assert!(!is_garbled(
            "Some effects cause a villain or minion to attack an ally directly."
        ));
        // French accents and guillemets in moderation must survive.
        assert!(!is_garbled(
            "Retirez la carte « identité » de votre deck pour préparer la partie."
        ));
    }

    #[test]
    fn to_markdown_renders_headings_body_and_page_markers() {
        let doc = ExtractedDocument {
            lines: vec![
                Line {
                    page: 10,
                    kind: LineKind::Heading,
                    text: "BASIC CARD".to_owned(),
                },
                Line {
                    page: 10,
                    kind: LineKind::Body,
                    text: "Cards in the Basic classification.".to_owned(),
                },
                Line {
                    page: 11,
                    kind: LineKind::Body,
                    text: "Next page text.".to_owned(),
                },
            ],
            report: QualityReport::default(),
        };

        let md = doc.to_markdown();
        assert!(md.contains("<!-- page 10 -->"));
        assert!(md.contains("## BASIC CARD"));
        assert!(md.contains("Cards in the Basic classification."));
        assert!(md.contains("<!-- page 11 -->"));
        assert!(md.contains("Next page text."));
    }

    #[test]
    fn structure_drops_garbled_lines_and_reports_them() {
        let pages = vec![vec![
            seg("Clean readable rules text here", 70.0, 700.0, "Body"),
            seg(
                "\u{93}\u{93}\u{93} broken \u{c3}\u{ab}\u{93}",
                70.0,
                688.0,
                "Body",
            ),
        ]];
        let doc = structure(&pages);
        assert_eq!(doc.report.kept, 1);
        assert_eq!(doc.report.dropped_garbled, 1);
        assert_eq!(doc.lines.len(), 1);
        assert_eq!(doc.lines[0].kind, LineKind::Body);
    }
}
