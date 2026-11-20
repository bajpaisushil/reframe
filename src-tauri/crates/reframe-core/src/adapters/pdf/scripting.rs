//! macOS Preview and Skim: open documents through AppleScript.
//!
//! `osascript` asks the reader for the path of every open document (Skim also for the current
//! page). The first query triggers macOS's Automation permission prompt; until it is granted,
//! or when the reader does not answer within a few seconds, a warning is reported and documents
//! come only from window titles and the command line. Preview does not expose the current page.

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::adapters::file_manager::support;

const TIMEOUT: Duration = Duration::from_secs(5);

/// Path of every Preview document, one per record.
const PREVIEW_SCRIPT: &str = r#"with timeout of 4 seconds
	tell application id "com.apple.Preview"
		set out to ""
		repeat with d in documents
			try
				set out to out & (path of d) & (character id 31) & (character id 30)
			end try
		end repeat
		return out
	end tell
end timeout"#;

/// Path and current page (1-based) of every Skim document.
const SKIM_SCRIPT: &str = r#"with timeout of 4 seconds
	tell application id "net.sourceforge.skim-app.skim"
		set out to ""
		repeat with d in documents
			try
				set p to path of d
				set n to ""
				try
					set n to (index of current page of d) as text
				end try
				set out to out & p & (character id 31) & n & (character id 30)
			end try
		end repeat
		return out
	end tell
end timeout"#;

/// A document reported by the reader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ScriptedDocument {
    pub path: PathBuf,
    pub page: Option<u32>,
}

/// Readers Reframe can ask through AppleScript.
pub(super) fn is_scriptable(app_id: &str) -> bool {
    matches!(app_id, "preview" | "skim")
}

/// Ask `app_id` for its open documents.
pub(super) fn open_documents(app_id: &str) -> Result<Vec<ScriptedDocument>, String> {
    let script = match app_id {
        "preview" => PREVIEW_SCRIPT,
        "skim" => SKIM_SCRIPT,
        _ => return Ok(Vec::new()),
    };
    support::run_applescript(script, TIMEOUT)
        .map(|raw| parse_output(&raw))
        .map_err(|e| e.to_string())
}

/// Parse `path<US>page<RS>` records; records without an absolute path are dropped.
pub(super) fn parse_output(raw: &str) -> Vec<ScriptedDocument> {
    support::applescript_records(raw)
        .into_iter()
        .filter_map(|fields| {
            let path = fields.first()?.trim();
            if !path.starts_with('/') {
                return None;
            }
            let page = fields
                .get(1)
                .and_then(|p| p.trim().parse::<u32>().ok())
                .filter(|p| *p >= 1);
            Some(ScriptedDocument {
                path: PathBuf::from(path),
                page,
            })
        })
        .collect()
}

/// Documents shown in the selected windows (`titles`): those whose file name a title names
/// (see [`super::titles::document_candidates`]). When some selected window matches no
/// document (its title may show the document's own title instead), the unmatched documents are
/// kept too, so nothing visible is lost.
pub(super) fn select_for_windows<'a>(
    documents: &'a [ScriptedDocument],
    titles: &[&str],
) -> Vec<&'a ScriptedDocument> {
    if titles.is_empty() {
        return documents.iter().collect();
    }
    let name_of = |doc: &ScriptedDocument| {
        doc.path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    // A document is shown when its file name is one the title names (not merely a substring of
    // the title: "data.pdf" does not show "a.pdf").
    let shows = |title: &str, doc: &ScriptedDocument| {
        let name = name_of(doc);
        !name.is_empty()
            && super::titles::document_candidates(title)
                .iter()
                .any(|candidate| support::display_name(Path::new(candidate)).to_lowercase() == name)
    };
    let matched: Vec<bool> = documents
        .iter()
        .map(|doc| titles.iter().any(|t| shows(t, doc)))
        .collect();
    let unmatched_window = titles
        .iter()
        .any(|t| !documents.iter().any(|doc| shows(t, doc)));
    documents
        .iter()
        .zip(matched)
        .filter(|(_, matched)| *matched || unmatched_window)
        .map(|(doc, _)| doc)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(path: &str, page: Option<u32>) -> ScriptedDocument {
        ScriptedDocument {
            path: PathBuf::from(path),
            page,
        }
    }

    #[test]
    fn parses_paths_and_pages() {
        let raw = "/Users/me/a.pdf\u{1f}3\u{1e}/Users/me/b, c.pdf\u{1f}\u{1e}relative.pdf\u{1f}1\u{1e}/x.pdf\u{1f}0\u{1e}\n";
        assert_eq!(
            parse_output(raw),
            vec![
                doc("/Users/me/a.pdf", Some(3)),
                doc("/Users/me/b, c.pdf", None),
                doc("/x.pdf", None),
            ]
        );
        assert!(parse_output("").is_empty());
    }

    #[test]
    fn keeps_documents_shown_in_selected_windows() {
        let docs = vec![doc("/a/one.pdf", None), doc("/a/two.pdf", None)];
        let selected = select_for_windows(&docs, &["two.pdf"]);
        assert_eq!(selected, vec![&docs[1]]);
        // A window showing a document title instead of a file name keeps every document.
        let selected = select_for_windows(&docs, &["two.pdf", "Annual Report"]);
        assert_eq!(selected.len(), 2);
        assert_eq!(select_for_windows(&docs, &[]).len(), 2);
        assert_eq!(
            select_for_windows(&docs, &["ONE.PDF (page 2 of 9)"]),
            vec![&docs[0]]
        );
        // "data.pdf" contains "a.pdf", but only the document named "data.pdf" is shown.
        let similar = vec![doc("/a/a.pdf", None), doc("/a/data.pdf", None)];
        assert_eq!(
            select_for_windows(&similar, &["data.pdf"]),
            vec![&similar[1]]
        );
        assert_eq!(
            select_for_windows(&similar, &["*a.pdf — Edited", "/a/data.pdf"]),
            vec![&similar[0], &similar[1]]
        );
    }

    #[test]
    fn only_preview_and_skim_are_scripted() {
        assert!(is_scriptable("preview"));
        assert!(is_scriptable("skim"));
        assert!(!is_scriptable("evince"));
        assert_eq!(open_documents("evince"), Ok(Vec::new()));
        assert!(SKIM_SCRIPT.contains("index of current page"));
        assert!(PREVIEW_SCRIPT.contains("path of d"));
        if !cfg!(target_os = "macos") {
            assert!(
                open_documents("preview")
                    .unwrap_err()
                    .contains("only available on macOS")
            );
        }
    }
}
