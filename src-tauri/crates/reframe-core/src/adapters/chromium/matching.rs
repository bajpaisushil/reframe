//! Pairing the windows of a browser session with the operating-system windows the user
//! selected.
//!
//! A browser window's title is its active tab's title plus the product name
//! ("Inbox - Gmail - Google Chrome", "GitHub and 3 more pages - Work - Microsoft​ Edge", just
//! "Inbox - Gmail" on macOS), or the name the user gave the window. Titles are therefore the
//! main evidence: an OS title that equals or starts with the expected title is a strong match,
//! one that contains it (on word boundaries) or shares most of its words a weaker one. The
//! bounds saved by the browser are compared with the OS bounds (also at common display scale
//! factors, since browsers store device-independent pixels) to break ties between similar
//! titles, and — only when unambiguous — to pair windows whose titles gave no evidence (for
//! example when the OS hides window titles).
//!
//! Limitations: the browser saves its session a few seconds after changes, so a tab switched
//! to just before capture can make a window unrecognizable; incognito, app and DevTools
//! windows are never in the session and stay unmatched.

use reframe_platform::{Rect, WindowState};

use super::snss::{SessionWindow, ShowState};
use super::urls;
use crate::util;

/// An operating-system window as seen by the matcher.
#[derive(Debug, Clone, Copy)]
pub struct OsWindow<'a> {
    pub title: &'a str,
    /// Normal (restored) bounds.
    pub bounds: Rect,
    pub state: WindowState,
}

/// Result of [`match_windows`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Matching {
    /// For each OS window (input order), the index of its session window.
    pub assignments: Vec<Option<usize>>,
    /// Overall confidence, used to choose between browser profiles (higher is better).
    pub score: f64,
}

impl Matching {
    pub fn matched(&self) -> usize {
        self.assignments.iter().flatten().count()
    }
}

/// Minimum title evidence for a title-based pairing.
const TITLE_MATCH: f64 = 0.3;
/// Minimum bounds overlap for a pairing without title evidence.
const BOUNDS_MATCH: f64 = 0.85;
/// Same, for OS windows whose title is unknown (empty).
const BOUNDS_MATCH_UNTITLED: f64 = 0.6;
/// How much better than any alternative a bounds-only pairing must be.
const BOUNDS_MARGIN: f64 = 0.05;
/// Display scale factors tried when comparing saved and OS bounds.
const SCALES: [f64; 5] = [1.0, 1.25, 1.5, 1.75, 2.0];
/// Coordinates beyond this are treated as garbage (keeps the arithmetic overflow-free).
const MAX_COORDINATE: u32 = 10_000_000;

const PRODUCT_NAMES: [&str; 4] = ["google chrome", "chromium", "microsoft edge", "brave"];

#[derive(Debug, Clone, Copy)]
struct Pair {
    session: usize,
    os: usize,
    title: f64,
    bounds: f64,
    rank: f64,
}

/// Pair `session` windows with `os` windows (front-to-back; the first one is the frontmost).
/// Each window is used at most once.
pub fn match_windows(
    session: &[&SessionWindow],
    active_window: Option<i32>,
    os: &[OsWindow<'_>],
) -> Matching {
    let expected: Vec<Vec<String>> = session.iter().map(|w| expected_titles(w)).collect();
    let titles: Vec<String> = os.iter().map(|w| normalize(w.title)).collect();

    let mut pairs = Vec::with_capacity(session.len() * os.len());
    for (s, window) in session.iter().enumerate() {
        for (o, os_window) in os.iter().enumerate() {
            let title = title_evidence(&titles[o], &expected[s]);
            let bounds = bounds_evidence(window, os_window);
            let mut rank = title + 0.25 * bounds;
            if same_state(window.show_state, os_window.state) {
                rank += 0.1;
            }
            if o == 0 && active_window == Some(window.id) {
                rank += 0.05;
            }
            pairs.push(Pair {
                session: s,
                os: o,
                title,
                bounds,
                rank,
            });
        }
    }

    let mut assignments: Vec<Option<usize>> = vec![None; os.len()];
    let mut taken = vec![false; session.len()];
    let mut score = 0.0;

    // 1. Title evidence, strongest pairs first.
    let mut by_title: Vec<Pair> = pairs
        .iter()
        .copied()
        .filter(|p| p.title >= TITLE_MATCH)
        .collect();
    by_title.sort_by(|a, b| {
        b.rank
            .total_cmp(&a.rank)
            .then(a.os.cmp(&b.os))
            .then(a.session.cmp(&b.session))
    });
    for pair in by_title {
        if !taken[pair.session] && assignments[pair.os].is_none() {
            taken[pair.session] = true;
            assignments[pair.os] = Some(pair.session);
            score += 2.0 + pair.title;
        }
    }

    // 2. Unambiguous geometry for the windows left.
    let bounds_of = |s: usize, o: usize| pairs[s * os.len() + o].bounds;
    loop {
        let mut progress = false;
        for (o, title) in titles.iter().enumerate() {
            if assignments[o].is_some() {
                continue;
            }
            let free_sessions: Vec<usize> = (0..session.len()).filter(|s| !taken[*s]).collect();
            let Some((s, best, second)) =
                best_two(free_sessions.iter().map(|&s| (s, bounds_of(s, o))))
            else {
                continue;
            };
            let threshold = if title.is_empty() {
                BOUNDS_MATCH_UNTITLED
            } else {
                BOUNDS_MATCH
            };
            let rival = (0..os.len())
                .filter(|&p| p != o && assignments[p].is_none())
                .map(|p| bounds_of(s, p))
                .fold(0.0, f64::max);
            if best >= threshold && best - second >= BOUNDS_MARGIN && best - rival >= BOUNDS_MARGIN
            {
                taken[s] = true;
                assignments[o] = Some(s);
                score += 1.0;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    Matching { assignments, score }
}

/// The best and second-best values of `(index, value)` pairs.
fn best_two(values: impl Iterator<Item = (usize, f64)>) -> Option<(usize, f64, f64)> {
    let mut best: Option<(usize, f64)> = None;
    let mut second = 0.0f64;
    for (index, value) in values {
        match best {
            Some((_, b)) if value <= b => second = second.max(value),
            Some((_, b)) => {
                second = second.max(b);
                best = Some((index, value));
            }
            None => best = Some((index, value)),
        }
    }
    best.map(|(index, value)| (index, value, second))
}

/// Titles the OS window of `window` is expected to show (normalized).
fn expected_titles(window: &SessionWindow) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(title) = &window.user_title {
        out.push(normalize(title));
    }
    if let Some(tab) = window.active_tab() {
        let title = normalize(&tab.title);
        if !title.is_empty() {
            out.push(title);
        } else if let Some(host) = urls::display_host(&tab.url) {
            // Untitled pages show their address.
            out.push(normalize(&host));
        }
    }
    out.retain(|t| !t.is_empty());
    out.dedup();
    out
}

/// How strongly the normalized OS title `os_title` supports one of the `expected` titles,
/// in `[0, 1]`.
fn title_evidence(os_title: &str, expected: &[String]) -> f64 {
    if os_title.is_empty() {
        return 0.0;
    }
    let stripped = strip_product_suffix(os_title);
    expected
        .iter()
        .map(|candidate| {
            if os_title == candidate || stripped == candidate {
                1.0
            } else if starts_with_word(os_title, candidate) {
                0.9
            } else if candidate.chars().count() >= 3 && contains_word(os_title, candidate) {
                0.75
            } else {
                0.7 * util::title_similarity(stripped, candidate)
            }
        })
        .fold(0.0, f64::max)
}

/// Lower-case, without invisible formatting characters (Edge puts a zero-width space in its
/// name, RTL locales add direction marks) and with collapsed whitespace.
pub fn normalize(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut pending_space = false;
    for c in title.chars() {
        if is_invisible(c) {
            continue;
        }
        if c.is_whitespace() || c.is_control() {
            pending_space = !out.is_empty();
            continue;
        }
        if pending_space {
            out.push(' ');
            pending_space = false;
        }
        out.extend(c.to_lowercase());
    }
    out
}

fn is_invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}' | '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{2069}' | '\u{FEFF}'
    )
}

/// Remove trailing " - <browser product>" parts of a normalized title.
fn strip_product_suffix(title: &str) -> &str {
    let mut title = title;
    while let Some(i) = title.rfind(" - ") {
        let tail = &title[i + 3..];
        if PRODUCT_NAMES.iter().any(|p| tail.starts_with(p)) {
            title = &title[..i];
        } else {
            break;
        }
    }
    title
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric()
}

fn starts_with_word(hay: &str, needle: &str) -> bool {
    hay.strip_prefix(needle)
        .is_some_and(|rest| rest.chars().next().is_none_or(|c| !is_word_char(c)))
}

fn contains_word(hay: &str, needle: &str) -> bool {
    hay.match_indices(needle).any(|(i, _)| {
        let before = hay[..i].chars().next_back();
        let after = hay[i + needle.len()..].chars().next();
        before.is_none_or(|c| !is_word_char(c)) && after.is_none_or(|c| !is_word_char(c))
    })
}

fn sane(rect: &Rect) -> bool {
    rect.width > 0
        && rect.height > 0
        && rect.x.unsigned_abs() <= MAX_COORDINATE
        && rect.y.unsigned_abs() <= MAX_COORDINATE
        && rect.width.unsigned_abs() <= MAX_COORDINATE
        && rect.height.unsigned_abs() <= MAX_COORDINATE
}

fn scaled(rect: &Rect, factor: f64) -> Rect {
    // Inputs are bounded by `sane`, so the products fit comfortably in i32.
    let scale = |v: i32| (f64::from(v) * factor).round() as i32;
    Rect::new(
        scale(rect.x),
        scale(rect.y),
        scale(rect.width),
        scale(rect.height),
    )
}

fn overlap(a: &Rect, b: &Rect) -> f64 {
    let intersection = a.intersection_area(b) as f64;
    let union = (a.area() + b.area()) as f64 - intersection;
    if union <= 0.0 {
        0.0
    } else {
        intersection / union
    }
}

/// Intersection-over-union of the saved and OS bounds, best over common scale factors.
fn bounds_evidence(window: &SessionWindow, os: &OsWindow<'_>) -> f64 {
    let Some(saved) = window.bounds else {
        return 0.0;
    };
    if !sane(&saved) || !sane(&os.bounds) {
        return 0.0;
    }
    SCALES
        .iter()
        .map(|&factor| overlap(&scaled(&saved, factor), &os.bounds))
        .fold(0.0, f64::max)
}

fn same_state(saved: ShowState, os: WindowState) -> bool {
    matches!(
        (saved, os),
        (ShowState::Normal | ShowState::Default, WindowState::Normal)
            | (ShowState::Maximized, WindowState::Maximized)
            | (ShowState::Minimized, WindowState::Minimized)
            | (ShowState::Fullscreen, WindowState::Fullscreen)
    )
}

#[cfg(test)]
mod tests {
    use super::super::snss::SessionTab;
    use super::*;

    fn session_window(id: i32, active_title: &str, bounds: Rect) -> SessionWindow {
        SessionWindow {
            id,
            window_type: 0,
            bounds: Some(bounds),
            show_state: ShowState::Normal,
            user_title: None,
            tabs: vec![
                SessionTab {
                    url: "https://first.example/".into(),
                    title: "First".into(),
                    pinned: false,
                },
                SessionTab {
                    url: "https://example.com/".into(),
                    title: active_title.into(),
                    pinned: false,
                },
            ],
            selected_tab: 1,
        }
    }

    fn os_window(title: &str, bounds: Rect) -> OsWindow<'_> {
        OsWindow {
            title,
            bounds,
            state: WindowState::Normal,
        }
    }

    #[test]
    fn matches_by_title_regardless_of_order() {
        let a = session_window(1, "Inbox (3) - Gmail", Rect::new(0, 0, 800, 600));
        let b = session_window(
            2,
            "reframe/reframe: Workspaces",
            Rect::new(800, 0, 800, 600),
        );
        let os = [
            os_window(
                "reframe/reframe: Workspaces - Google Chrome",
                Rect::new(5, 5, 300, 300),
            ),
            os_window(
                "Inbox (3) - Gmail - Google Chrome",
                Rect::new(900, 900, 300, 300),
            ),
        ];
        let m = match_windows(&[&a, &b], None, &os);
        assert_eq!(m.assignments, [Some(1), Some(0)]);
        assert_eq!(m.matched(), 2);
    }

    #[test]
    fn understands_edge_and_macos_titles() {
        let a = session_window(1, "Pull requests", Rect::new(0, 0, 800, 600));
        let b = session_window(2, "Calendar", Rect::new(0, 0, 800, 600));
        let os = [
            os_window(
                "Pull requests and 4 more pages - Work - Microsoft\u{200b} Edge",
                Rect::new(0, 0, 10, 10),
            ),
            os_window("Calendar", Rect::new(0, 0, 10, 10)),
        ];
        let m = match_windows(&[&a, &b], None, &os);
        assert_eq!(m.assignments, [Some(0), Some(1)]);
    }

    #[test]
    fn dynamic_titles_still_match() {
        let a = session_window(1, "(3) Inbox | Example Mail", Rect::new(0, 0, 800, 600));
        let os = [os_window(
            "(4) Inbox | Example Mail - Google Chrome",
            Rect::new(0, 0, 10, 10),
        )];
        assert_eq!(match_windows(&[&a], None, &os).assignments, [Some(0)]);
    }

    #[test]
    fn identical_titles_are_separated_by_bounds() {
        let left = session_window(1, "New Tab", Rect::new(0, 0, 960, 1040));
        let right = session_window(2, "New Tab", Rect::new(960, 0, 960, 1040));
        let os = [
            os_window("New Tab - Google Chrome", Rect::new(962, 2, 956, 1036)),
            os_window("New Tab - Google Chrome", Rect::new(2, 2, 956, 1036)),
        ];
        let m = match_windows(&[&left, &right], None, &os);
        assert_eq!(m.assignments, [Some(1), Some(0)]);
    }

    #[test]
    fn scaled_bounds_are_recognized_without_titles() {
        // The browser stores DIPs; the OS reports physical pixels at 150 %.
        let saved = session_window(1, "Docs", Rect::new(100, 100, 1000, 700));
        let other = session_window(2, "Mail", Rect::new(2000, 900, 400, 300));
        let os = [os_window("", Rect::new(150, 150, 1500, 1050))];
        let m = match_windows(&[&saved, &other], None, &os);
        assert_eq!(m.assignments, [Some(0)]);
    }

    #[test]
    fn unrelated_windows_stay_unmatched() {
        let a = session_window(1, "Inbox", Rect::new(0, 0, 800, 600));
        let os = [
            os_window("DevTools - example.com/", Rect::new(1000, 0, 500, 500)),
            os_window(
                "Homebrew — The Missing Package Manager - Google Chrome",
                Rect::new(0, 700, 300, 300),
            ),
        ];
        let m = match_windows(&[&a], None, &os);
        assert_eq!(m.assignments, [None, None]);
        assert_eq!(m.score, 0.0);
    }

    #[test]
    fn bounds_only_matches_must_be_unambiguous() {
        let a = session_window(1, "One", Rect::new(0, 0, 800, 600));
        let b = session_window(2, "Two", Rect::new(0, 0, 800, 600));
        let os = [os_window(
            "Something else entirely",
            Rect::new(0, 0, 800, 600),
        )];
        assert_eq!(match_windows(&[&a, &b], None, &os).assignments, [None]);
        let os = [os_window(
            "Something else entirely",
            Rect::new(0, 0, 800, 600),
        )];
        assert_eq!(match_windows(&[&a], None, &os).assignments, [Some(0)]);
    }

    #[test]
    fn user_titles_and_untitled_pages() {
        let mut named = session_window(1, "Some page", Rect::new(0, 0, 800, 600));
        named.user_title = Some("Research".into());
        let mut untitled = session_window(2, "", Rect::new(0, 0, 800, 600));
        untitled.tabs[1].url = "https://www.example.org/path".into();
        let os = [
            os_window("example.org/path - Google Chrome", Rect::new(0, 0, 1, 1)),
            os_window("Research", Rect::new(0, 0, 1, 1)),
        ];
        let m = match_windows(&[&named, &untitled], None, &os);
        assert_eq!(m.assignments, [Some(1), Some(0)]);
    }

    #[test]
    fn garbage_bounds_do_not_overflow() {
        let mut a = session_window(
            1,
            "X",
            Rect::new(i32::MAX - 5, i32::MIN + 5, i32::MAX, i32::MAX),
        );
        a.show_state = ShowState::Maximized;
        let os = [os_window("Y", Rect::new(i32::MAX - 1, 0, i32::MAX, 10))];
        assert_eq!(match_windows(&[&a], None, &os).assignments, [None]);
    }

    #[test]
    fn normalization() {
        assert_eq!(normalize("  Inbox\u{200b} -\tMail  "), "inbox - mail");
        assert_eq!(normalize("\u{202b}كتاب\u{202c}"), "كتاب");
        assert_eq!(strip_product_suffix("a - b - google chrome"), "a - b");
        assert_eq!(strip_product_suffix("brave - brave"), "brave");
        assert!(starts_with_word("new tab - google chrome", "new tab"));
        assert!(!starts_with_word("homebrew", "home"));
        assert!(contains_word("x home y", "home"));
        assert!(!contains_word("homebrew", "home"));
    }
}
