//! Debug tool: extract a PDF and print the structured lines (kind + text) plus
//! the quality report, for eyeballing extraction against real files. Run with:
//!
//! ```bash
//! PDFIUM_LIB_PATH=vendor/pdfium/lib \
//!   cargo run -p zyndeck-ingester --example explore -- <pdf> [first-page] [last-page]
//! ```

use std::path::Path;

use zyndeck_ingester::document::{self, LineKind};
use zyndeck_ingester::pdf;

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: explore <pdf> [first-page] [last-page]");
    let first: usize = args.next().map_or(1, |a| a.parse().unwrap());
    let last: usize = args.next().map_or(usize::MAX, |a| a.parse().unwrap());

    let lib_dir = std::env::var("PDFIUM_LIB_PATH").unwrap_or_else(|_| pdf::DEFAULT_LIB_DIR.into());
    let pdfium = pdf::bind(&lib_dir)?;

    let pages = pdf::read_pages(&pdfium, Path::new(&path))?;
    let doc = document::structure(&pages);

    for line in doc
        .lines
        .iter()
        .filter(|l| (first..=last).contains(&l.page))
    {
        let tag = match line.kind {
            LineKind::Heading => "##",
            LineKind::Body => "  ",
            LineKind::Icons => "<>",
        };
        println!("p{:>3} {tag} {}", line.page, line.text);
    }

    eprintln!(
        "\n{} pages | kept {} lines | dropped {} garbled",
        pages.len(),
        doc.report.kept,
        doc.report.dropped_garbled,
    );

    Ok(())
}
