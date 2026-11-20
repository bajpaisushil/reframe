//! XDG base directories (data directories used for `.desktop` files and icon themes).

use std::path::PathBuf;

/// Resolved XDG data directories.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct XdgDirs {
    /// `$HOME`, when known.
    pub home: Option<PathBuf>,
    /// `$XDG_DATA_HOME`, default `~/.local/share`.
    pub data_home: Option<PathBuf>,
    /// `$XDG_DATA_DIRS`, default `/usr/local/share:/usr/share`, in precedence order.
    pub data_dirs: Vec<PathBuf>,
}

impl XdgDirs {
    /// From the process environment.
    pub fn from_process() -> Self {
        let var = |name: &str| std::env::var_os(name).map(|v| v.to_string_lossy().into_owned());
        Self::from_values(
            var("HOME").as_deref(),
            var("XDG_DATA_HOME").as_deref(),
            var("XDG_DATA_DIRS").as_deref(),
        )
    }

    /// From explicit variable values (relative paths are ignored, as the spec requires).
    pub fn from_values(
        home: Option<&str>,
        data_home: Option<&str>,
        data_dirs: Option<&str>,
    ) -> Self {
        let absolute = |s: &str| {
            let p = PathBuf::from(s.trim());
            (p.is_absolute()).then_some(p)
        };
        let home = home.and_then(absolute);
        let data_home = data_home
            .and_then(absolute)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
        let mut dirs: Vec<PathBuf> = Vec::new();
        for dir in data_dirs
            .filter(|d| !d.trim().is_empty())
            .unwrap_or("/usr/local/share:/usr/share")
            .split(':')
            .filter_map(absolute)
        {
            if !dirs.contains(&dir) {
                dirs.push(dir);
            }
        }
        if dirs.is_empty() {
            dirs = vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ];
        }
        Self {
            home,
            data_home,
            data_dirs: dirs,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_spec() {
        let x = XdgDirs::from_values(Some("/home/u"), None, None);
        assert_eq!(x.data_home, Some(PathBuf::from("/home/u/.local/share")));
        assert_eq!(
            x.data_dirs,
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share")
            ]
        );
    }

    #[test]
    fn explicit_values_are_used_and_relative_ones_ignored() {
        let x = XdgDirs::from_values(Some("/home/u"), Some("/data"), Some("/a:relative:/b::/a"));
        assert_eq!(x.data_home, Some(PathBuf::from("/data")));
        assert_eq!(x.data_dirs, vec![PathBuf::from("/a"), PathBuf::from("/b")]);
        let x = XdgDirs::from_values(None, Some("rel"), Some("only:relative"));
        assert_eq!(x.home, None);
        assert_eq!(x.data_home, None);
        assert_eq!(x.data_dirs.len(), 2);
    }
}
