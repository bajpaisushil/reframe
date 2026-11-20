//! Turning saved browser windows into launches: one `--new-window` launch per window, split
//! when a window has more tabs than one launch should carry.
//!
//! Limits: at most [`MAX_URLS_PER_WINDOW`] addresses per launch, and every command line stays
//! below [`MAX_COMMAND_LINE_CHARS`] (Windows allows 32,767 characters). A window over either
//! limit is reopened as several consecutive windows, keeping tab order.

use crate::model::BrowserWindow;

use super::urls;

/// Most addresses opened by one launch (one new window).
pub const MAX_URLS_PER_WINDOW: usize = 100;

/// Upper bound for a whole command line, including the program.
pub const MAX_COMMAND_LINE_CHARS: usize = 30_000;

/// Worst-case extra characters per argument on a command line (separator and quotes).
const ARG_OVERHEAD: usize = 3;

/// Room that must be left for addresses: enough for the longest address that is kept.
pub const MIN_URL_BUDGET: usize = urls::MAX_URL_CHARS + ARG_OVERHEAD;

/// Characters `arg` occupies on a command line in the worst case.
pub fn arg_chars(arg: &str) -> usize {
    arg.chars().count() + ARG_OVERHEAD
}

/// The addresses of one launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowLaunch {
    pub urls: Vec<String>,
    /// Text the new window's title will contain: the browser activates the first address of
    /// a command line, so this is the first tab's title (or its host for untitled pages).
    pub title_hint: Option<String>,
}

/// Launches for `windows` when the program and fixed arguments take `fixed_chars`. Tabs whose
/// address is not restorable are skipped (they are re-checked here because workspaces can be
/// imported); windows left without tabs produce no launch.
pub fn window_launches(windows: &[BrowserWindow], fixed_chars: usize) -> Vec<WindowLaunch> {
    let budget = MAX_COMMAND_LINE_CHARS.saturating_sub(fixed_chars);
    let mut out = Vec::new();
    for window in windows {
        let mut current: Vec<String> = Vec::new();
        let mut hint = None;
        let mut used = 0usize;
        for tab in &window.tabs {
            let Some(url) = urls::restorable_url(&tab.url) else {
                continue;
            };
            let cost = arg_chars(&url);
            if cost > budget {
                continue;
            }
            if !current.is_empty() && (current.len() >= MAX_URLS_PER_WINDOW || used + cost > budget)
            {
                out.push(WindowLaunch {
                    urls: std::mem::take(&mut current),
                    title_hint: hint.take(),
                });
                used = 0;
            }
            if current.is_empty() {
                hint = title_hint(&tab.title, &url);
            }
            used += cost;
            current.push(url);
        }
        if !current.is_empty() {
            out.push(WindowLaunch {
                urls: current,
                title_hint: hint,
            });
        }
    }
    out
}

fn title_hint(title: &str, url: &str) -> Option<String> {
    let title = title.trim();
    if title.is_empty() || title == url || urls::restorable_url(title).as_deref() == Some(url) {
        urls::display_host(url)
    } else {
        Some(title.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::BrowserTab;

    fn window(urls: &[String]) -> BrowserWindow {
        BrowserWindow {
            tabs: urls
                .iter()
                .enumerate()
                .map(|(i, url)| BrowserTab {
                    url: url.clone(),
                    title: format!("Tab {i}"),
                    pinned: false,
                })
                .collect(),
            active_tab: Some(0),
            window_title: None,
        }
    }

    #[test]
    fn splits_by_count_and_keeps_order() {
        let urls: Vec<String> = (0..250)
            .map(|i| format!("https://example.com/{i}"))
            .collect();
        let launches = window_launches(&[window(&urls)], 100);
        let sizes: Vec<usize> = launches.iter().map(|l| l.urls.len()).collect();
        assert_eq!(sizes, [100, 100, 50]);
        let flat: Vec<&String> = launches.iter().flat_map(|l| &l.urls).collect();
        assert_eq!(flat, urls.iter().collect::<Vec<_>>());
        assert_eq!(launches[1].title_hint.as_deref(), Some("Tab 100"));
    }

    #[test]
    fn splits_by_command_line_length() {
        let urls: Vec<String> = (0..40)
            .map(|i| format!("https://example.com/{i}?q={}", "x".repeat(2000)))
            .collect();
        let fixed = 200;
        let launches = window_launches(&[window(&urls)], fixed);
        assert!(launches.len() > 1);
        for launch in &launches {
            let total: usize = fixed + launch.urls.iter().map(|u| arg_chars(u)).sum::<usize>();
            assert!(total <= MAX_COMMAND_LINE_CHARS, "{total}");
        }
        assert_eq!(launches.iter().map(|l| l.urls.len()).sum::<usize>(), 40);
    }

    #[test]
    fn skips_internal_pages_and_empty_windows() {
        let mixed = window(&[
            "chrome://newtab/".into(),
            "https://a.example/".into(),
            "javascript:alert(1)".into(),
            "edge://settings/".into(),
            "https://b.example/".into(),
        ]);
        let internal_only = window(&["chrome://newtab/".into(), "about:blank".into()]);
        let launches = window_launches(&[internal_only, mixed], 100);
        assert_eq!(launches.len(), 1);
        assert_eq!(
            launches[0].urls,
            ["https://a.example/", "https://b.example/"]
        );
        assert_eq!(launches[0].title_hint.as_deref(), Some("Tab 1"));
    }

    #[test]
    fn untitled_tabs_hint_with_their_host() {
        let mut w = window(&["https://www.example.org/x".into()]);
        w.tabs[0].title = "https://www.example.org/x".into();
        let launches = window_launches(&[w], 0);
        assert_eq!(launches[0].title_hint.as_deref(), Some("example.org"));
    }
}
