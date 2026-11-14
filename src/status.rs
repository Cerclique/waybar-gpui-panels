//! A repository's status: open issues, open pull requests and recent CI runs.
//!
//! [`load`] is the seam for the data source: it runs `gh … --json` commands
//! (see `docs/adr/0001-fetch-github-data-through-gh-cli.md`) and parses their
//! output. It blocks, so it runs on a background thread.

use std::{collections::HashSet, fmt, thread, time::Duration};

use serde::Deserialize;

use crate::gh::{GhError, run_gh};

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
        CiState::worst(self.runs.iter().map(|r| r.state))
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
    /// Maps one GitHub status, conclusion or commit-status state, in any case.
    /// Anything that isn't a known final outcome hasn't completed: `Running`.
    pub fn from_github(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "failure" | "timed_out" | "startup_failure" | "action_required" | "error" => {
                Self::Failure
            }
            "success" | "neutral" | "skipped" | "cancelled" | "stale" => Self::Success,
            _ => Self::Running,
        }
    }

    /// The worst of `states`; `None` when there are none.
    pub fn worst(states: impl IntoIterator<Item = Self>) -> Option<Self> {
        states.into_iter().max_by_key(|s| s.severity())
    }

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

/// How many issues and pull requests are fetched: a list this long may have more.
pub const LIST_LIMIT: usize = 30;

/// How long one `gh` command may run.
const GH_TIMEOUT: Duration = Duration::from_secs(10);

/// How many recent runs on the default branch are scanned for the latest
/// run of each workflow.
const RUN_LIMIT: usize = 100;

/// What [`load`] fetched.
pub struct Loaded {
    /// The default branch, to pass back to the next [`load`].
    pub default_branch: String,
    pub status: RepoStatus,
}

/// Fetches `repo`'s status: its open issues, open pull requests and the
/// latest CI run of each workflow on its default branch. Looks the default
/// branch up first unless `default_branch` already gives it.
///
/// Blocking; call it off the foreground thread.
pub fn load(repo: &Repo, default_branch: Option<String>) -> Result<Loaded, GhError> {
    let default_branch = match default_branch {
        Some(branch) => branch,
        None => fetch_default_branch(repo)?,
    };
    let status = fetch_status(repo, &default_branch)?;
    Ok(Loaded {
        default_branch,
        status,
    })
}

fn fetch_default_branch(repo: &Repo) -> Result<String, GhError> {
    let repo = repo.to_string();
    let json = run_gh(
        &["repo", "view", &repo, "--json", "defaultBranchRef"],
        GH_TIMEOUT,
    )?;
    parse_default_branch(&json).map_err(unexpected_output)
}

/// Runs the three list commands in parallel. Fails as a whole if any one of
/// them fails, so fresh and stale lists never mix.
fn fetch_status(repo: &Repo, branch: &str) -> Result<RepoStatus, GhError> {
    let repo = repo.to_string();
    let limit = LIST_LIMIT.to_string();
    let run_limit = RUN_LIMIT.to_string();
    let list = |kind: &'static str, fields: &'static str| {
        let (repo, limit) = (&repo, &limit);
        move || {
            run_gh(
                &[
                    kind,
                    "list",
                    "--repo",
                    repo,
                    "--limit",
                    limit,
                    "--search",
                    "sort:updated-desc",
                    "--json",
                    fields,
                ],
                GH_TIMEOUT,
            )
        }
    };
    let (issues, pulls, runs) = thread::scope(|s| {
        let issues = s.spawn(list("issue", "number,title,author,labels,url"));
        let pulls = s.spawn(list(
            "pr",
            "number,title,author,isDraft,url,statusCheckRollup",
        ));
        let runs = s.spawn(|| {
            run_gh(
                &[
                    "run",
                    "list",
                    "--repo",
                    &repo,
                    "--branch",
                    branch,
                    "--limit",
                    &run_limit,
                    "--json",
                    "workflowDatabaseId,workflowName,headBranch,status,conclusion,url",
                ],
                GH_TIMEOUT,
            )
        });
        (join(issues), join(pulls), join(runs))
    });
    Ok(RepoStatus {
        issues: parse_issues(&issues?).map_err(unexpected_output)?,
        pulls: parse_pulls(&pulls?).map_err(unexpected_output)?,
        runs: latest_runs(&runs?).map_err(unexpected_output)?,
    })
}

fn join(handle: thread::ScopedJoinHandle<'_, Result<String, GhError>>) -> Result<String, GhError> {
    handle
        .join()
        .unwrap_or_else(|_| Err(GhError::Other("gh runner panicked".to_owned())))
}

fn unexpected_output(e: serde_json::Error) -> GhError {
    GhError::Other(format!("unexpected gh output: {e}"))
}

/// `author` as `gh … --json author` prints it.
#[derive(Deserialize)]
struct GhAuthor {
    login: String,
}

#[derive(Deserialize)]
struct GhLabel {
    name: String,
}

#[derive(Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    author: GhAuthor,
    labels: Vec<GhLabel>,
    url: String,
}

/// Parses `gh issue list --json number,title,author,labels,url`.
pub fn parse_issues(json: &str) -> serde_json::Result<Vec<Issue>> {
    let issues: Vec<GhIssue> = serde_json::from_str(json)?;
    Ok(issues
        .into_iter()
        .map(|i| Issue {
            number: i.number,
            title: i.title,
            author: i.author.login,
            labels: i.labels.into_iter().map(|l| l.name).collect(),
            url: i.url,
        })
        .collect())
}

/// One `statusCheckRollup` entry: a check run (`status` and `conclusion`) or a
/// commit status (`state`).
#[derive(Deserialize)]
struct GhCheck {
    status: Option<String>,
    conclusion: Option<String>,
    state: Option<String>,
}

impl GhCheck {
    fn ci_state(&self) -> CiState {
        match &self.state {
            Some(state) => CiState::from_github(state),
            None => run_state(
                self.status.as_deref().unwrap_or_default(),
                self.conclusion.as_deref().unwrap_or_default(),
            ),
        }
    }
}

/// A check or workflow run: running until completed, then its conclusion.
fn run_state(status: &str, conclusion: &str) -> CiState {
    if status.eq_ignore_ascii_case("completed") {
        CiState::from_github(conclusion)
    } else {
        CiState::Running
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhPull {
    number: u64,
    title: String,
    author: GhAuthor,
    is_draft: bool,
    url: String,
    status_check_rollup: Option<Vec<GhCheck>>,
}

/// Parses `gh pr list --json number,title,author,isDraft,url,statusCheckRollup`.
/// A pull request's checks are its worst check; `None` without checks.
pub fn parse_pulls(json: &str) -> serde_json::Result<Vec<PullRequest>> {
    let pulls: Vec<GhPull> = serde_json::from_str(json)?;
    Ok(pulls
        .into_iter()
        .map(|p| PullRequest {
            number: p.number,
            title: p.title,
            author: p.author.login,
            draft: p.is_draft,
            checks: CiState::worst(
                p.status_check_rollup
                    .iter()
                    .flatten()
                    .map(GhCheck::ci_state),
            ),
            url: p.url,
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhRun {
    workflow_database_id: u64,
    workflow_name: String,
    head_branch: String,
    status: String,
    conclusion: Option<String>,
    url: String,
}

/// Parses `gh run list --json workflowDatabaseId,workflowName,headBranch,status,conclusion,url`
/// (newest first) and keeps the latest run of each workflow.
pub fn latest_runs(json: &str) -> serde_json::Result<Vec<WorkflowRun>> {
    let runs: Vec<GhRun> = serde_json::from_str(json)?;
    let mut seen = HashSet::new();
    Ok(runs
        .into_iter()
        .filter(|r| seen.insert(r.workflow_database_id))
        .map(|r| WorkflowRun {
            state: run_state(&r.status, r.conclusion.as_deref().unwrap_or_default()),
            workflow: r.workflow_name,
            branch: r.head_branch,
            url: r.url,
        })
        .collect())
}

/// Parses `gh repo view --json defaultBranchRef`.
pub fn parse_default_branch(json: &str) -> serde_json::Result<String> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GhRepo {
        default_branch_ref: GhRef,
    }
    #[derive(Deserialize)]
    struct GhRef {
        name: String,
    }
    let repo: GhRepo = serde_json::from_str(json)?;
    Ok(repo.default_branch_ref.name)
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
    fn parses_issues_from_gh_issue_list() {
        let issues = parse_issues(include_str!("fixtures/issues.json")).unwrap();
        assert_eq!(issues.len(), 30);
        let first = &issues[0];
        assert_eq!(first.number, 15061);
        assert_eq!(
            first.title,
            "[aw] Issue Triage (skills-driven): reported incomplete result"
        );
        assert_eq!(first.author, "app/cli-triage");
        assert_eq!(first.labels, ["needs-triage", "agentic-workflows"]);
        assert_eq!(first.url, "https://github.com/cli/cli/issues/15061");
    }

    #[test]
    fn parses_pulls_from_gh_pr_list() {
        let pulls = parse_pulls(include_str!("fixtures/pulls.json")).unwrap();
        assert_eq!(pulls.len(), 30);
        let first = &pulls[0];
        assert_eq!(first.number, 15062);
        assert_eq!(
            first.title,
            "githubtemplate: ignore symlinked files in non-legacy template subdirectories"
        );
        assert_eq!(first.author, "texasich");
        assert!(!first.draft);
        assert_eq!(first.checks, Some(CiState::Success));
        assert_eq!(first.url, "https://github.com/cli/cli/pull/15062");

        let by_number = |n| pulls.iter().find(|p| p.number == n).unwrap();
        assert!(by_number(15052).draft);
        assert_eq!(by_number(15052).checks, Some(CiState::Failure));
        assert_eq!(by_number(13781).checks, None);
    }

    #[test]
    fn a_pull_is_as_bad_as_its_worst_check() {
        let pull = |rollup: &str| {
            let json = format!(
                r#"[{{"number":1,"title":"t","author":{{"login":"a"}},"isDraft":false,"url":"u","statusCheckRollup":{rollup}}}]"#
            );
            parse_pulls(&json).unwrap()[0].checks
        };
        let check = |status, conclusion| {
            format!(
                r#"{{"__typename":"CheckRun","status":"{status}","conclusion":"{conclusion}"}}"#
            )
        };
        let context = |state| format!(r#"{{"__typename":"StatusContext","state":"{state}"}}"#);

        let success = check("COMPLETED", "SUCCESS");
        let queued = check("QUEUED", "");
        let failed = check("COMPLETED", "TIMED_OUT");
        assert_eq!(
            pull(&format!("[{success},{queued}]")),
            Some(CiState::Running)
        );
        assert_eq!(
            pull(&format!("[{queued},{failed}]")),
            Some(CiState::Failure)
        );
        assert_eq!(
            pull(&format!("[{success},{}]", context("PENDING"))),
            Some(CiState::Running)
        );
        assert_eq!(
            pull(&format!("[{}]", context("ERROR"))),
            Some(CiState::Failure)
        );
        assert_eq!(
            pull(&format!("[{}]", context("SUCCESS"))),
            Some(CiState::Success)
        );
        assert_eq!(pull("[]"), None);
    }

    #[test]
    fn keeps_the_latest_run_of_each_workflow() {
        let runs = latest_runs(include_str!("fixtures/runs.json")).unwrap();
        let summary: Vec<_> = runs
            .iter()
            .map(|r| (r.workflow.as_str(), r.state))
            .collect();
        assert_eq!(
            summary,
            [
                ("Copilot", CiState::Running),
                ("Dependabot PR Triage Scheduler", CiState::Success),
                ("Issue Triaging", CiState::Success),
                ("Issue Triage (skills-driven)", CiState::Failure),
                ("Triage Scheduled Tasks", CiState::Success),
                ("PR Triaging", CiState::Success),
                ("Bump Go", CiState::Success),
                ("Agentic Maintenance", CiState::Success),
                ("Go Vulnerability Check", CiState::Success),
            ]
        );
        assert_eq!(runs[0].branch, "trunk");
        assert_eq!(
            runs[0].url,
            "https://github.com/cli/cli/actions/runs/43613944310"
        );
    }

    #[test]
    fn reads_the_default_branch() {
        assert_eq!(
            parse_default_branch(include_str!("fixtures/repo.json")).unwrap(),
            "trunk"
        );
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_issues("not json").is_err());
    }

    #[test]
    fn maps_github_states_onto_three_ci_states() {
        for v in [
            "queued",
            "in_progress",
            "waiting",
            "pending",
            "expected",
            "IN_PROGRESS",
        ] {
            assert_eq!(CiState::from_github(v), CiState::Running, "{v}");
        }
        for v in [
            "failure",
            "timed_out",
            "startup_failure",
            "action_required",
            "error",
            "FAILURE",
        ] {
            assert_eq!(CiState::from_github(v), CiState::Failure, "{v}");
        }
        for v in [
            "success",
            "neutral",
            "skipped",
            "cancelled",
            "stale",
            "SKIPPED",
        ] {
            assert_eq!(CiState::from_github(v), CiState::Success, "{v}");
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
