//! Document names in reader window titles.
//!
//! Readers put the document in the title in different ways:
//! `paper.pdf - SumatraPDF`, `paper.pdf - Adobe Acrobat Reader (64-bit)`, `paper.pdf — Okular`,
//! Evince/Atril/Xreader `Document title — paper.pdf`, zathura `/home/me/paper.pdf` (or
//! `~/paper.pdf`, optionally followed by `[3/10]`), Skim `paper.pdf (page 3 of 10)`, and
//! `*paper.pdf` for modified documents. Instead of one parser per reader, every substring between
//! separators (` - `, ` — `, ` – `, ` · `, ` | `) that ends with a document extension is a
//! candidate; callers try them longest first, so file names containing separators still work.

/// Extensions of documents PDF / e-book readers open.
pub(super) const EXTENSIONS: &[&str] = &[
    "pdf", "epub", "djvu", "djv", "xps", "oxps", "cbz", "cbr", "mobi", "fb2", "ps", "eps", "dvi",
];

const SEPARATORS: &[&str] = &[" - ", " — ", " – ", " · ", " | "];

/// Longer titles are cut (window titles are short; this bounds the work).
const MAX_TITLE_CHARS: usize = 512;

/// Whether `name` ends with a document extension (`report.PDF`, `C:\docs\book.epub`).
pub(super) fn has_document_extension(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.trim().is_empty()
            && !stem.ends_with(['/', '\\'])
            && EXTENSIONS
                .iter()
                .any(|known| known.eq_ignore_ascii_case(extension))
    })
}

/// Substrings of a window title that may name the shown document, longest first.
pub(super) fn document_candidates(title: &str) -> Vec<String> {
    let title: String = title.chars().take(MAX_TITLE_CHARS).collect();
    let mut starts = vec![0];
    let mut ends = vec![title.len()];
    for separator in SEPARATORS {
        for (index, _) in title.match_indices(separator) {
            ends.push(index);
            starts.push(index + separator.len());
        }
    }
    let mut candidates: Vec<String> = Vec::new();
    for &start in &starts {
        for &end in &ends {
            if end <= start {
                continue;
            }
            let piece = clean(&title[start..end]);
            if has_document_extension(&piece) && !candidates.contains(&piece) {
                candidates.push(piece);
            }
        }
    }
    candidates.sort_by_key(|c| std::cmp::Reverse(c.chars().count()));
    candidates
}

/// Remove modification markers and a trailing page indicator.
fn clean(piece: &str) -> String {
    let piece = piece.trim().trim_matches('*').trim();
    if has_document_extension(piece) {
        return piece.to_string();
    }
    // "paper.pdf (page 3 of 10)", "paper.pdf [3/10]", "paper.pdf [modified]".
    let stripped = if piece.ends_with(')') {
        piece.rfind('(').map(|i| &piece[..i])
    } else if piece.ends_with(']') {
        piece.rfind('[').map(|i| &piece[..i])
    } else {
        None
    };
    match stripped.map(|s| s.trim().trim_matches('*').trim()) {
        Some(stripped) if has_document_extension(stripped) => stripped.to_string(),
        _ => piece.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(title: &str) -> Vec<String> {
        document_candidates(title)
    }

    #[test]
    fn extensions() {
        assert!(has_document_extension("paper.pdf"));
        assert!(has_document_extension("REPORT.PDF"));
        assert!(has_document_extension(r"C:\docs\book.epub"));
        assert!(has_document_extension("scan.djvu"));
        assert!(!has_document_extension(".pdf"));
        assert!(!has_document_extension("/docs/.pdf"));
        assert!(!has_document_extension("notes.txt"));
        assert!(!has_document_extension("pdf"));
        assert!(!has_document_extension("SumatraPDF"));
    }

    #[test]
    fn reader_title_formats() {
        assert_eq!(candidates("paper.pdf - SumatraPDF"), vec!["paper.pdf"]);
        assert_eq!(
            candidates(r"C:\Users\me\paper.pdf - SumatraPDF"),
            vec![r"C:\Users\me\paper.pdf"]
        );
        assert_eq!(
            candidates("Report final.PDF - Adobe Acrobat Reader (64-bit)"),
            vec!["Report final.PDF"]
        );
        assert_eq!(candidates("book.epub — Okular"), vec!["book.epub"]);
        assert_eq!(
            candidates("Deep Learning — deep-learning.pdf"),
            vec!["Deep Learning — deep-learning.pdf", "deep-learning.pdf"]
        );
        assert_eq!(
            candidates("/home/me/docs/paper.pdf"),
            vec!["/home/me/docs/paper.pdf"]
        );
        assert_eq!(
            candidates("~/docs/paper.pdf [3/10]"),
            vec!["~/docs/paper.pdf"]
        );
        assert_eq!(candidates("paper.pdf (page 3 of 10)"), vec!["paper.pdf"]);
        assert_eq!(
            candidates("*paper.pdf - Foxit PDF Reader"),
            vec!["paper.pdf"]
        );
        assert_eq!(
            candidates("paper.pdf* [modified] — Okular"),
            vec!["paper.pdf"]
        );
    }

    #[test]
    fn file_names_with_separators_come_first() {
        assert_eq!(
            candidates("Smith - 2020 - Paper.pdf - SumatraPDF"),
            vec!["Smith - 2020 - Paper.pdf", "2020 - Paper.pdf", "Paper.pdf"]
        );
    }

    #[test]
    fn titles_without_documents() {
        assert!(candidates("SumatraPDF").is_empty());
        assert!(candidates("Adobe Acrobat Reader (64-bit)").is_empty());
        assert!(candidates("Document Viewer").is_empty());
        assert!(candidates("").is_empty());
        assert!(candidates("My Paper — Okular").is_empty());
    }

    #[test]
    fn long_and_unicode_titles_are_safe() {
        let long = format!("{}.pdf - SumatraPDF", "é".repeat(2000));
        assert!(candidates(&long).is_empty());
        assert_eq!(
            candidates("Überblick – résumé.pdf"),
            vec!["Überblick – résumé.pdf", "résumé.pdf"]
        );
    }
}
