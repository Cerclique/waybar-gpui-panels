//! The watch list: the repositories the `repo` panel shows, read from
//! `$XDG_CONFIG_HOME/waybar-github-viewer/repos` unless panel arguments
//! replace it.

use std::{
    env,
    ffi::OsStr,
    fs, io,
    path::{Path, PathBuf},
};

use crate::status::Repo;

/// Parses a watch list file's `text`, one repository per line in file order.
/// Blank lines are ignored and `#` starts a comment. Errors name `file` and
/// the line.
pub fn parse(text: &str, file: &str) -> Result<Vec<Repo>, String> {
    let entries = (1..).zip(text.lines()).filter_map(|(line_no, line)| {
        let entry = line.split_once('#').map_or(line, |(entry, _)| entry).trim();
        (!entry.is_empty()).then_some((line_no, entry))
    });
    collect(Source::File(file), entries)
}

/// Parses panel arguments, one repository each, in the forms the file takes.
pub fn from_args(args: &[String]) -> Result<Vec<Repo>, String> {
    collect(
        Source::Arguments,
        (1..).zip(args.iter().map(String::as_str)),
    )
}

/// Where entries come from, for their error messages.
#[derive(Clone, Copy)]
enum Source<'a> {
    /// A watch list file, by name; entries are numbered by line.
    File(&'a str),
    /// Panel arguments, numbered from 1.
    Arguments,
}

impl Source<'_> {
    /// Where entry `n` is: `repos:4` or `argument 2`.
    fn at(self, n: usize) -> String {
        match self {
            Self::File(file) => format!("{file}:{n}"),
            Self::Arguments => format!("argument {n}"),
        }
    }

    /// What one entry is called: `line` or `argument`.
    fn unit(self) -> &'static str {
        match self {
            Self::File(_) => "line",
            Self::Arguments => "argument",
        }
    }
}

/// Parses numbered `entries` from `source` in order, rejecting a repeat of a
/// repository already given (compared case-insensitively).
fn collect<'a>(
    source: Source<'_>,
    entries: impl IntoIterator<Item = (usize, &'a str)>,
) -> Result<Vec<Repo>, String> {
    // Each repository with its number, for duplicate errors.
    let mut repos: Vec<(Repo, usize)> = Vec::new();
    for (n, entry) in entries {
        let at = source.at(n);
        let repo = Repo::parse(entry).map_err(|e| format!("{at}: {e}"))?;
        if let Some((first, first_n)) = repos.iter().find(|(r, _)| r.is_same(&repo)) {
            return Err(format!(
                "{at}: duplicate of {} {first_n} ({first})",
                source.unit()
            ));
        }
        repos.push((repo, n));
    }
    Ok(repos.into_iter().map(|(repo, _)| repo).collect())
}

/// The repositories to show: the panel arguments if any, else the watch list
/// at `path`, read afresh. An empty list is an error.
pub fn load(args: &[String], path: Option<&Path>) -> Result<Vec<Repo>, String> {
    if !args.is_empty() {
        return from_args(args);
    }
    let Some(path) = path else {
        return Err(
            "no repositories: set $HOME or $XDG_CONFIG_HOME, or pass owner/name arguments"
                .to_owned(),
        );
    };
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(format!(
                "no repositories: create {} or pass owner/name arguments",
                path.display()
            ));
        }
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    let file = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let repos = parse(&text, &file)?;
    if repos.is_empty() {
        return Err(format!(
            "no repositories in {}: add owner/name lines or pass owner/name arguments",
            path.display()
        ));
    }
    Ok(repos)
}

/// Where the watch list lives, from this process's `$XDG_CONFIG_HOME` and
/// `$HOME`. `None` when neither gives a directory.
pub fn default_path() -> Option<PathBuf> {
    path_in(
        env::var_os("XDG_CONFIG_HOME").as_deref(),
        env::home_dir().as_deref(),
    )
}

/// Where the watch list lives, given `$XDG_CONFIG_HOME` and `$HOME`: under
/// `xdg_config_home` when it's an absolute path, else under `~/.config`.
fn path_in(xdg_config_home: Option<&OsStr>, home: Option<&Path>) -> Option<PathBuf> {
    let config = match xdg_config_home.map(Path::new) {
        Some(dir) if dir.is_absolute() => dir.to_owned(),
        _ => home?.join(".config"),
    };
    Some(config.join("waybar-github-viewer").join("repos"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(repos: &[Repo]) -> Vec<String> {
        repos.iter().map(Repo::to_string).collect()
    }

    #[test]
    fn reads_repos_in_file_order_skipping_comments_and_blank_lines() {
        let text = "\
# work
Cerclique/waybar-gpui

https://github.com/Cerclique/waybar_github_viewer  # this one
   git@github.com:rust-lang/rust.git
";
        assert_eq!(
            names(&parse(text, "repos").unwrap()),
            [
                "Cerclique/waybar-gpui",
                "Cerclique/waybar_github_viewer",
                "rust-lang/rust",
            ]
        );
    }

    #[test]
    fn names_the_file_and_line_of_a_malformed_entry() {
        let text = "a/b\n\n# c/d\nCerclique\n";
        assert_eq!(
            parse(text, "repos").unwrap_err(),
            r#"repos:4: expected owner/name or a github.com URL, got "Cerclique""#
        );
        assert!(
            parse("https://github.com/a/b/pulls", "repos")
                .unwrap_err()
                .starts_with("repos:1: ")
        );
    }

    #[test]
    fn rejects_a_duplicate_ignoring_case() {
        let text = "x/y\nCerclique/waybar-gpui\n\n\n\n\nhttps://github.com/cerclique/Waybar-GPUI\n";
        assert_eq!(
            parse(text, "repos").unwrap_err(),
            "repos:7: duplicate of line 2 (Cerclique/waybar-gpui)"
        );
    }

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn panel_arguments_accept_the_same_forms() {
        let repos = from_args(&args(&["a/b", "https://github.com/c/d.git"])).unwrap();
        assert_eq!(names(&repos), ["a/b", "c/d"]);
        assert_eq!(
            from_args(&args(&["a/b", "nope"])).unwrap_err(),
            r#"argument 2: expected owner/name or a github.com URL, got "nope""#
        );
    }

    #[test]
    fn rejects_duplicate_panel_arguments_ignoring_case() {
        assert_eq!(
            from_args(&args(&["a/b", "c/d", "A/B"])).unwrap_err(),
            "argument 3: duplicate of argument 1 (a/b)"
        );
    }

    #[test]
    fn lives_under_xdg_config_home_else_home_dot_config() {
        let home = Some(Path::new("/home/me"));
        assert_eq!(
            path_in(Some("/cfg".as_ref()), home).unwrap(),
            Path::new("/cfg/waybar-github-viewer/repos")
        );
        // Unset, empty or relative XDG_CONFIG_HOME falls back to ~/.config.
        for xdg in [None, Some(""), Some("relative/cfg")] {
            assert_eq!(
                path_in(xdg.map(AsRef::as_ref), home).unwrap(),
                Path::new("/home/me/.config/waybar-github-viewer/repos"),
                "{xdg:?}"
            );
        }
        assert_eq!(path_in(None, None), None);
    }

    /// A fresh directory for one test's watch list file.
    fn temp_dir(test: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "waybar-github-viewer-{}-{test}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn panel_arguments_replace_the_watch_list() {
        let file = temp_dir("override").join("repos");
        fs::write(&file, "x/y\n").unwrap();
        let repos = load(&args(&["a/b"]), Some(&file)).unwrap();
        assert_eq!(names(&repos), ["a/b"]);
    }

    #[test]
    fn reads_the_file_again_on_every_load() {
        let file = temp_dir("reread").join("repos");
        fs::write(&file, "a/b\nc/d\n").unwrap();
        assert_eq!(names(&load(&[], Some(&file)).unwrap()), ["a/b", "c/d"]);
        fs::write(&file, "e/f\n").unwrap();
        assert_eq!(names(&load(&[], Some(&file)).unwrap()), ["e/f"]);
    }

    #[test]
    fn a_missing_file_says_where_to_create_it() {
        let file = temp_dir("missing").join("repos");
        assert_eq!(
            load(&[], Some(&file)).unwrap_err(),
            format!(
                "no repositories: create {} or pass owner/name arguments",
                file.display()
            )
        );
    }

    #[test]
    fn an_empty_watch_list_names_the_file() {
        let file = temp_dir("empty").join("repos");
        for text in ["", "\n# nothing yet\n\n"] {
            fs::write(&file, text).unwrap();
            assert_eq!(
                load(&[], Some(&file)).unwrap_err(),
                format!(
                    "no repositories in {}: add owner/name lines or pass owner/name arguments",
                    file.display()
                ),
                "{text:?}"
            );
        }
    }

    #[test]
    fn file_errors_name_the_file_and_line() {
        let file = temp_dir("malformed").join("repos");
        fs::write(&file, "a/b\nnope\n").unwrap();
        assert_eq!(
            load(&[], Some(&file)).unwrap_err(),
            r#"repos:2: expected owner/name or a github.com URL, got "nope""#
        );
    }
}
