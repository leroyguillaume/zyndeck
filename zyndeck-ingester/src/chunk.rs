//! Pure splitting of a reviewed Markdown transcript into retrieval chunks.
//!
//! The input is the transcript produced (and possibly human-edited) by the
//! extract step — Markdown with `## ` headings, `<!-- page N -->` provenance
//! markers and body lines (see [`crate::document::ExtractedDocument::to_markdown`]).
//! Chunking is heading-aware: a chunk never crosses a section boundary, and a
//! long section is split at line boundaries to stay within a size budget. Each
//! chunk keeps the section heading and the source page it starts on, so
//! retrieval can show *where* a rule came from. No I/O happens here, so the
//! splitting is unit-testable against literal transcripts.

/// Upper bound on a chunk's body length, in characters. Sized for retrieval
/// granularity rather than the model's context: BGE-M3 accepts ~8K tokens, but
/// smaller, focused chunks (a few rules paragraphs, ~200-300 tokens) retrieve
/// more precisely for question answering. A section longer than this is split at
/// line boundaries; an individual line longer than this is kept whole (rules
/// lines are short, and splitting mid-sentence would hurt retrieval more).
const MAX_CHUNK_CHARS: usize = 1200;

/// A retrieval chunk: a run of body text under one heading, with the provenance
/// needed to cite it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    /// The section heading the chunk falls under; empty if it precedes any
    /// heading.
    pub heading: String,
    /// 1-based source page the chunk starts on.
    pub page: usize,
    /// The chunk's body text: its lines joined by newlines.
    pub content: String,
}

/// Splits a Markdown transcript into ordered chunks.
pub fn split(transcript: &str) -> Vec<Chunk> {
    let mut builder = Builder::default();
    let mut page = 1;

    for raw in transcript.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        if let Some(n) = parse_page_marker(line) {
            page = n;
        } else if let Some(heading) = line.strip_prefix("## ") {
            builder.start_section(heading.trim());
        } else {
            builder.push_line(line, page);
        }
    }

    builder.finish()
}

/// Accumulates the open chunk and flushes completed ones, so the splitting loop
/// stays a flat scan over lines.
#[derive(Default)]
struct Builder {
    chunks: Vec<Chunk>,
    heading: String,
    /// Page the open chunk started on (meaningful only while `content` is
    /// non-empty).
    page: usize,
    content: String,
}

impl Builder {
    /// Begins a new section: closes the open chunk so chunks never span headings.
    fn start_section(&mut self, heading: &str) {
        self.flush();
        self.heading = heading.to_owned();
    }

    /// Appends a body line to the open chunk, starting a fresh chunk first if the
    /// line would push it past the size budget.
    fn push_line(&mut self, line: &str, page: usize) {
        if self.content.is_empty() {
            self.page = page;
            self.content.push_str(line);
            return;
        }
        // +1 for the joining newline.
        let would_be = self.content.chars().count() + 1 + line.chars().count();
        if would_be > MAX_CHUNK_CHARS {
            self.flush();
            self.page = page;
            self.content.push_str(line);
        } else {
            self.content.push('\n');
            self.content.push_str(line);
        }
    }

    /// Closes the open chunk, if any, into the output.
    fn flush(&mut self) {
        if !self.content.is_empty() {
            self.chunks.push(Chunk {
                heading: self.heading.clone(),
                page: self.page,
                content: std::mem::take(&mut self.content),
            });
        }
    }

    /// Closes the last open chunk and returns all chunks in document order.
    fn finish(mut self) -> Vec<Chunk> {
        self.flush();
        self.chunks
    }
}

/// Parses a `<!-- page N -->` provenance marker, returning `N`; `None` for any
/// other line (including comments that are not page markers).
fn parse_page_marker(line: &str) -> Option<usize> {
    let inner = line.strip_prefix("<!--")?.strip_suffix("-->")?.trim();
    inner.strip_prefix("page")?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_page_markers_without_emitting_them() {
        let chunks = split("<!-- page 7 -->\nThe attacker deals damage.");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].page, 7);
        assert_eq!(chunks[0].content, "The attacker deals damage.");
    }

    #[test]
    fn ignores_non_page_comments_as_body() {
        // A comment that is not a page marker is just a body line.
        let chunks = split("<!-- note -->");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "<!-- note -->");
    }

    #[test]
    fn breaks_chunks_on_a_heading_boundary() {
        let transcript = "\
<!-- page 1 -->
## ATTACK
Roll the dice.
## DEFENSE
Raise your shield.";
        let chunks = split(transcript);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading, "ATTACK");
        assert_eq!(chunks[0].content, "Roll the dice.");
        assert_eq!(chunks[1].heading, "DEFENSE");
        assert_eq!(chunks[1].content, "Raise your shield.");
    }

    #[test]
    fn joins_lines_of_a_section_into_one_chunk() {
        let chunks = split("## RULES\nfirst line\nsecond line");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].content, "first line\nsecond line");
    }

    #[test]
    fn splits_a_long_section_at_line_boundaries() {
        // Each line is 400 chars; three of them exceed the 1200-char budget, so
        // the section splits into two chunks (two lines, then one).
        let line = "x".repeat(400);
        let transcript = format!("## BIG\n{line}\n{line}\n{line}");
        let chunks = split(&transcript);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].content, format!("{line}\n{line}"));
        assert_eq!(chunks[1].content, line);
        // Both halves keep the section heading.
        assert!(chunks.iter().all(|c| c.heading == "BIG"));
    }

    #[test]
    fn records_the_starting_page_when_a_chunk_spans_a_page_break() {
        // A section flows across a page boundary; the chunk is tagged with the
        // page it started on.
        let transcript = "\
<!-- page 4 -->
## FLOW
keeps going
<!-- page 5 -->
across pages";
        let chunks = split(transcript);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].page, 4);
        assert_eq!(chunks[0].content, "keeps going\nacross pages");
    }

    #[test]
    fn drops_heading_only_sections() {
        // A heading with no body under it yields no chunk.
        let chunks = split("## EMPTY\n## REAL\nbody");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading, "REAL");
    }

    #[test]
    fn empty_transcript_yields_no_chunks() {
        assert!(split("").is_empty());
        assert!(split("\n  \n").is_empty());
    }

    #[test]
    fn body_before_any_heading_has_an_empty_heading() {
        let chunks = split("intro text before headings");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].heading, "");
        assert_eq!(chunks[0].content, "intro text before headings");
    }
}
