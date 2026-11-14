//! A repository's status: open issues, open pull requests and recent CI runs.
//!
//! [`load`] is the seam for the data source. It returns sample data until the
//! source (the `gh` CLI or GitHub's API) is chosen; it runs on a background
//! thread, so a blocking network call is fine there.

use std::fmt;

/// An `owner/name` GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    /// Parses `owner/name`; both parts must be non-empty.
    pub fn parse(s: &str) -> Result<Self, String> {
        match s.split_once('/') {
            Some((owner, name)) if !owner.is_empty() && !name.is_empty() && !name.contains('/') => {
                Ok(Self {
                    owner: owner.to_owned(),
                    name: name.to_owned(),
                })
            }
            _ => Err(format!("expected owner/repo, got {s:?}")),
        }
    }
}

impl Repo {
    pub fn url(&self) -> String {
        format!("https://github.com/{self}")
    }
}

impl fmt::Display for Repo {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct RepoStatus {
    pub issues: Vec<Issue>,
    pub pulls: Vec<PullRequest>,
    pub runs: Vec<WorkflowRun>,
}

impl RepoStatus {
    /// The state of the most recent run on each workflow, worst first:
    /// what the header shows for CI as a whole. `None` without runs.
    pub fn ci_summary(&self) -> Option<CiState> {
        self.runs
            .iter()
            .map(|r| r.state)
            .max_by_key(|s| s.severity())
    }
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub labels: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub draft: bool,
    pub checks: Option<CiState>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct WorkflowRun {
    pub workflow: String,
    pub branch: String,
    pub state: CiState,
    pub url: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CiState {
    Success,
    Running,
    Failure,
}

impl CiState {
    fn severity(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Running => 1,
            Self::Failure => 2,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Success => "passing",
            Self::Running => "running",
            Self::Failure => "failing",
        }
    }
}

/// Fetches `repo`'s status. Blocking; call it off the foreground thread.
///
/// Sample data until the data source is decided.
pub fn load(repo: &Repo) -> Result<RepoStatus, String> {
    let base = repo.url();
    Ok(RepoStatus {
        issues: vec![
            Issue {
                number: 42,
                url: format!("{base}/issues/42"),
                title: "Popover flickers when the bar is on the bottom edge".into(),
                author: "alice".into(),
                labels: vec!["bug".into()],
            },
            Issue {
                number: 37,
                url: format!("{base}/issues/37"),
                title: "Support several repositories in one panel".into(),
                author: "bob".into(),
                labels: vec!["enhancement".into(), "needs-triage".into()],
            },
        ],
        pulls: vec![
            PullRequest {
                number: 45,
                url: format!("{base}/pull/45"),
                title: "Cache responses between opens".into(),
                author: "carol".into(),
                draft: false,
                checks: Some(CiState::Running),
            },
            PullRequest {
                number: 44,
                url: format!("{base}/pull/44"),
                title: "Fix CI badge colours".into(),
                author: "alice".into(),
                draft: true,
                checks: Some(CiState::Failure),
            },
        ],
        runs: vec![
            WorkflowRun {
                workflow: "CI".into(),
                branch: "main".into(),
                state: CiState::Success,
                url: format!("{base}/actions"),
            },
            WorkflowRun {
                workflow: "Release".into(),
                branch: "main".into(),
                state: CiState::Failure,
                url: format!("{base}/actions"),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_owner_slash_name() {
        let repo = Repo::parse("Cerclique/waybar-gpui").unwrap();
        assert_eq!(repo.to_string(), "Cerclique/waybar-gpui");
    }

    #[test]
    fn rejects_malformed_repos() {
        for s in ["", "waybar-gpui", "/name", "owner/", "a/b/c"] {
            assert!(Repo::parse(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn ci_summary_is_the_worst_run() {
        let run = |state| WorkflowRun {
            workflow: "w".into(),
            branch: "main".into(),
            state,
            url: String::new(),
        };
        let status = |runs| RepoStatus {
            issues: vec![],
            pulls: vec![],
            runs,
        };
        assert_eq!(status(vec![]).ci_summary(), None);
        assert_eq!(
            status(vec![run(CiState::Success), run(CiState::Running)]).ci_summary(),
            Some(CiState::Running)
        );
        assert_eq!(
            status(vec![run(CiState::Failure), run(CiState::Success)]).ci_summary(),
            Some(CiState::Failure)
        );
    }
}
