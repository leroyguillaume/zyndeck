//! pdfium I/O boundary: turn a PDF file into [`RawSegment`]s carrying the
//! geometry and font attributes the pure structuring step needs.
//!
//! Everything that touches the native pdfium library lives here; the heuristics
//! that interpret these segments live in [`crate::document`] so they can be
//! unit-tested without the library.

use std::path::Path;

use anyhow::{Context, Result};
use pdfium_render::prelude::*;

/// Default directory the pdfium library is loaded from. Its own subdirectory
/// under the FHS location for locally-installed libraries, since it is loaded
/// explicitly by path (not via the system linker) and is a single-app, vendored
/// dependency that shouldn't mix with system libraries. The Docker image and
/// Linux hosts install it here. For local development, fetch it with
/// `scripts/fetch-pdfium.sh` and point `PDFIUM_LIB_PATH` at `vendor/pdfium/lib`.
pub const DEFAULT_LIB_DIR: &str = "/usr/local/lib/pdfium";

/// A run of text extracted from a PDF page, with the geometry and font we need
/// to reconstruct reading order and structure.
///
/// Coordinates are in PDF points with a bottom-left origin, so `bottom` grows
/// upward and a higher `bottom` means higher on the page.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSegment {
    /// Text content of the run, as pdfium decoded it.
    pub text: String,
    pub left: f32,
    pub right: f32,
    pub top: f32,
    pub bottom: f32,
    /// Font name of the run's first glyph, used as a style proxy. Still carries
    /// the PDF subset prefix (e.g. `AMVWYI+Avenir-Book`); normalise before use.
    pub font: String,
}

impl RawSegment {
    /// Glyph height in points — a proxy for font size, since pdfium normalises
    /// the reported font size away.
    pub fn height(&self) -> f32 {
        self.top - self.bottom
    }
}

/// One page's segments, in pdfium's emission order (which already follows
/// columns on the documents we target).
pub type RawPage = Vec<RawSegment>;

/// Binds the pdfium library found in `lib_dir`.
pub fn bind(lib_dir: &str) -> Result<Pdfium> {
    let name = Pdfium::pdfium_platform_library_name_at_path(lib_dir);
    let bindings = Pdfium::bind_to_library(&name)
        .with_context(|| format!("loading pdfium library from {lib_dir}"))?;
    Ok(Pdfium::new(bindings))
}

/// Reads every page of `path` into raw segments.
pub fn read_pages(pdfium: &Pdfium, path: &Path) -> Result<Vec<RawPage>> {
    let document = pdfium
        .load_pdf_from_file(path, None)
        .with_context(|| format!("opening PDF {}", path.display()))?;

    document
        .pages()
        .iter()
        .map(|page| read_page(&page))
        .collect()
}

fn read_page(page: &PdfPage) -> Result<RawPage> {
    let text = page.text().context("reading page text")?;

    let mut segments = Vec::new();
    for segment in text.segments().iter() {
        let content = segment.text();
        if content.trim().is_empty() {
            continue;
        }

        let bounds = segment.bounds();
        segments.push(RawSegment {
            text: content,
            left: bounds.left().value,
            right: bounds.right().value,
            top: bounds.top().value,
            bottom: bounds.bottom().value,
            font: first_glyph_font(&segment),
        });
    }

    Ok(segments)
}

/// Reads the font name of a segment's first non-whitespace glyph as a proxy for
/// the whole run's style.
fn first_glyph_font(segment: &PdfPageTextSegment) -> String {
    let Ok(chars) = segment.chars() else {
        return String::new();
    };

    for c in chars.iter() {
        if c.unicode_char().is_some_and(char::is_whitespace) {
            continue;
        }
        return c.font_name();
    }

    String::new()
}
