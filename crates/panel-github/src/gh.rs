//! Running the `gh` CLI, and turning its failures into short messages.

use std::{
    fmt,
    io::{self, Read},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

/// Why a `gh` command failed. Shows as the short message the panel prints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhError {
    NotInstalled,
    NotLoggedIn,
    RepoNotFound,
    /// `gh release view` on a repository without a published release.
    ReleaseNotFound,
    TimedOut,
    Offline,
    /// Anything else: the first line of `gh`'s error output.
    Other(String),
}

impl fmt::Display for GhError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotInstalled => f.write_str("gh not installed"),
            Self::NotLoggedIn => f.write_str("run `gh auth login`"),
            Self::RepoNotFound => f.write_str("repo not found"),
            Self::ReleaseNotFound => f.write_str("release not found"),
            Self::TimedOut => f.write_str("timed out"),
            Self::Offline => f.write_str("offline"),
            Self::Other(line) => f.write_str(line),
        }
    }
}

impl std::error::Error for GhError {}

impl GhError {
    /// Classifies a failed command from its error output.
    pub fn from_stderr(stderr: &str) -> Self {
        let has = |needle: &str| stderr.contains(needle);
        if has("gh auth login") || has("HTTP 401") {
            Self::NotLoggedIn
        } else if has("Could not resolve to a Repository") || has("HTTP 404") {
            Self::RepoNotFound
        } else if has("release not found") {
            Self::ReleaseNotFound
        } else if has("error connecting to") || has("no such host") || has("network is unreachable")
        {
            Self::Offline
        } else {
            let line = stderr.lines().map(str::trim).find(|l| !l.is_empty());
            Self::Other(line.unwrap_or("gh failed").to_owned())
        }
    }
}

/// Runs `gh args…` and returns its standard output. Kills it after `timeout`.
///
/// Blocking; call it off the foreground thread.
pub fn run_gh(args: &[&str], timeout: Duration) -> Result<String, GhError> {
    let mut child = Command::new("gh")
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => GhError::NotInstalled,
            _ => GhError::Other(format!("can't run gh: {e}")),
        })?;
    // Drain both pipes on their own threads so a full pipe can't block `gh`.
    let stdout = read_to_end(child.stdout.take());
    let stderr = read_to_end(child.stderr.take());

    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(GhError::TimedOut);
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(GhError::Other(format!("can't wait for gh: {e}"))),
        }
    };
    let stdout = stdout.join().unwrap_or_default();
    let stderr = stderr.join().unwrap_or_default();
    if status.success() {
        Ok(stdout)
    } else {
        Err(GhError::from_stderr(&stderr))
    }
}

fn read_to_end(pipe: Option<impl Read + Send + 'static>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut out = String::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_string(&mut out);
        }
        out
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_gh_error_output() {
        let cases = [
            (
                "To get started with GitHub CLI, please run:  gh auth login\n\
                 Alternatively, populate the GH_TOKEN environment variable with a GitHub API authentication token.\n",
                GhError::NotLoggedIn,
            ),
            (
                "HTTP 401: Bad credentials (https://api.github.com/graphql)\n\
                 Try authenticating with:  gh auth login -h github.com\n",
                GhError::NotLoggedIn,
            ),
            (
                "GraphQL: Could not resolve to a Repository with the name 'nobody/nope'. (repository)\n",
                GhError::RepoNotFound,
            ),
            (
                "failed to get runs: HTTP 404: Not Found (https://api.github.com/repos/nobody/nope/actions/runs?branch=main)\n",
                GhError::RepoNotFound,
            ),
            (
                "error connecting to api.github.com\ncheck your internet connection or https://githubstatus.com\n",
                GhError::Offline,
            ),
            (
                "Post \"https://api.github.com/graphql\": dial tcp: lookup api.github.com: no such host\n",
                GhError::Offline,
            ),
            (
                "\nHTTP 502: Bad Gateway\nsecond line\n",
                GhError::Other("HTTP 502: Bad Gateway".to_owned()),
            ),
            ("release not found\n", GhError::ReleaseNotFound),
            ("", GhError::Other("gh failed".to_owned())),
        ];
        for (stderr, expected) in cases {
            assert_eq!(GhError::from_stderr(stderr), expected, "{stderr:?}");
        }
    }

    #[test]
    fn shows_short_messages() {
        assert_eq!(GhError::NotInstalled.to_string(), "gh not installed");
        assert_eq!(GhError::NotLoggedIn.to_string(), "run `gh auth login`");
        assert_eq!(GhError::TimedOut.to_string(), "timed out");
    }
}
