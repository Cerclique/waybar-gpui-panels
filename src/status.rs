//! A repository's status: open issues, open pull requests, recent CI runs and
//! the latest release.
//!
//! [`load`] is the seam for the data source: it runs `gh … --json` commands
//! (see `docs/adr/0001-fetch-github-data-through-gh-cli.md`) and parses their
//! output. It blocks, so it runs on a background thread.

use std::{
    collections::HashSet,
    fmt, thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Deserialize;

use crate::gh::{GhError, run_gh};

/// An `owner/name` GitHub repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Repo {
    /// Parses `owner/name`, `https://github.com/owner/name` (optionally
    /// ending in `/`) or `git@github.com:owner/name`, each optionally ending
    /// in `.git`. Any other host or a deeper URL is an error. The result is
    /// always `owner/name`, with the owner and name capitalised as given.
    pub fn parse(s: &str) -> Result<Self, String> {
        let path = if let Some(rest) = s.strip_prefix("https://github.com/") {
            rest.strip_suffix('/').unwrap_or(rest)
        } else {
            s.strip_prefix("git@github.com:").unwrap_or(s)
        };
        let path = path.strip_suffix(".git").unwrap_or(path);
        match path.split_once('/') {
            Some((owner, name)) if is_name(owner) && is_name(name) => Ok(Self {
                owner: owner.to_owned(),
                name: name.to_owned(),
            }),
            _ => Err(format!(
                "expected owner/name or a github.com URL, got {s:?}"
            )),
        }
    }

    /// Whether `self` and `other` are the same repository: GitHub names are
    /// case-insensitive.
    pub fn is_same(&self, other: &Self) -> bool {
        self.owner.eq_ignore_ascii_case(&other.owner) && self.name.eq_ignore_ascii_case(&other.name)
    }
}

/// A non-empty owner or repository name, in the characters GitHub allows.
fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
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
    /// `None` when the repository has no published release.
    pub release: Option<Release>,
}

/// The repository's latest published release: no drafts, no prereleases.
#[derive(Debug, Clone)]
pub struct Release {
    pub tag: String,
    pub published_at: SystemTime,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
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
    /// Completed without a verdict (cancelled, skipped, ...). Never failing:
    /// health checks treat it as passing, but it ranks below `Success` so a
    /// rollup is neutral only when every check is.
    Neutral(NeutralKind),
    Success,
    Running,
    Failure,
}

/// The GitHub conclusion behind a [`CiState::Neutral`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeutralKind {
    Cancelled,
    Skipped,
    Neutral,
    Stale,
}

impl NeutralKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::Skipped => "skipped",
            Self::Neutral => "neutral",
            Self::Stale => "stale",
        }
    }
}

impl CiState {
    /// Maps one GitHub status, conclusion or commit-status state, in any case.
    /// Anything that isn't a known final outcome hasn't completed: `Running`.
    pub fn from_github(value: &str) -> Self {
        match value.to_ascii_lowercase().as_str() {
            "failure" | "timed_out" | "startup_failure" | "action_required" | "error" => {
                Self::Failure
            }
            "success" => Self::Success,
            "cancelled" => Self::Neutral(NeutralKind::Cancelled),
            "skipped" => Self::Neutral(NeutralKind::Skipped),
            "neutral" => Self::Neutral(NeutralKind::Neutral),
            "stale" => Self::Neutral(NeutralKind::Stale),
            _ => Self::Running,
        }
    }

    /// The worst of `states`; `None` when there are none.
    pub fn worst(states: impl IntoIterator<Item = Self>) -> Option<Self> {
        states.into_iter().max_by_key(|s| s.severity())
    }

    fn severity(self) -> u8 {
        match self {
            Self::Neutral(_) => 0,
            Self::Success => 1,
            Self::Running => 2,
            Self::Failure => 3,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Neutral(kind) => kind.label(),
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

/// Fetches `repo`'s status: its open issues, open pull requests, the latest
/// CI run of each workflow on its default branch and its latest release.
/// Looks the default branch up first unless `default_branch` already gives it.
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

/// Runs the three list commands and the release lookup in parallel. Fails as
/// a whole if any one of them fails, so fresh and stale data never mix; a
/// repository without a release isn't a failure.
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
    let (issues, pulls, runs, release) = thread::scope(|s| {
        let issues = s.spawn(list("issue", "number,title,url"));
        let pulls = s.spawn(list("pr", "number,title,isDraft,url,statusCheckRollup"));
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
        let release = s.spawn(|| {
            run_gh(
                &[
                    "release",
                    "view",
                    "--repo",
                    &repo,
                    "--json",
                    "tagName,publishedAt,url",
                ],
                GH_TIMEOUT,
            )
        });
        (join(issues), join(pulls), join(runs), join(release))
    });
    let release = match release {
        Ok(json) => Some(parse_release(&json).map_err(unexpected_output)?),
        Err(GhError::ReleaseNotFound) => None,
        Err(e) => return Err(e),
    };
    Ok(RepoStatus {
        issues: parse_issues(&issues?).map_err(unexpected_output)?,
        pulls: parse_pulls(&pulls?).map_err(unexpected_output)?,
        runs: latest_runs(&runs?).map_err(unexpected_output)?,
        release,
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

#[derive(Deserialize)]
struct GhIssue {
    number: u64,
    title: String,
    url: String,
}

/// Parses `gh issue list --json number,title,url`.
pub fn parse_issues(json: &str) -> serde_json::Result<Vec<Issue>> {
    let issues: Vec<GhIssue> = serde_json::from_str(json)?;
    Ok(issues
        .into_iter()
        .map(|i| Issue {
            number: i.number,
            title: i.title,
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
    is_draft: bool,
    url: String,
    status_check_rollup: Option<Vec<GhCheck>>,
}

/// Parses `gh pr list --json number,title,isDraft,url,statusCheckRollup`.
/// A pull request's checks are its worst check; `None` without checks.
pub fn parse_pulls(json: &str) -> serde_json::Result<Vec<PullRequest>> {
    let pulls: Vec<GhPull> = serde_json::from_str(json)?;
    Ok(pulls
        .into_iter()
        .map(|p| PullRequest {
            number: p.number,
            title: p.title,
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

/// Parses `gh release view --json tagName,publishedAt,url`.
pub fn parse_release(json: &str) -> serde_json::Result<Release> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct GhRelease {
        tag_name: String,
        published_at: String,
        url: String,
    }
    let release: GhRelease = serde_json::from_str(json)?;
    let published_at = parse_utc(&release.published_at).ok_or_else(|| {
        serde::de::Error::custom(format!(
            "expected a UTC timestamp, got {:?}",
            release.published_at
        ))
    })?;
    Ok(Release {
        tag: release.tag_name,
        published_at,
        url: release.url,
    })
}

/// Parses a UTC timestamp as GitHub prints it: `2024-02-29T23:59:59Z`.
fn parse_utc(s: &str) -> Option<SystemTime> {
    let (date, time) = s.strip_suffix('Z')?.split_once('T')?;
    let mut date = date.splitn(3, '-').map(str::parse::<i64>);
    let (year, month, day) = (date.next()?.ok()?, date.next()?.ok()?, date.next()?.ok()?);
    let mut time = time.splitn(3, ':').map(str::parse::<u64>);
    let (hour, min, sec) = (time.next()?.ok()?, time.next()?.ok()?, time.next()?.ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || min > 59 || sec > 59 {
        return None;
    }
    let days = u64::try_from(days_since_epoch(year, month, day)).ok()?;
    Some(UNIX_EPOCH + Duration::from_secs(days * 86_400 + hour * 3600 + min * 60 + sec))
}

/// Days from 1970-01-01 to a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_since_epoch(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
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
    fn parses_github_urls_to_owner_slash_name() {
        for s in [
            "Cerclique/waybar-gpui",
            "Cerclique/waybar-gpui.git",
            "https://github.com/Cerclique/waybar-gpui",
            "https://github.com/Cerclique/waybar-gpui/",
            "https://github.com/Cerclique/waybar-gpui.git",
            "git@github.com:Cerclique/waybar-gpui.git",
            "git@github.com:Cerclique/waybar-gpui",
        ] {
            let repo = Repo::parse(s).unwrap_or_else(|e| panic!("{s:?}: {e}"));
            assert_eq!(repo.to_string(), "Cerclique/waybar-gpui", "{s:?}");
        }
    }

    #[test]
    fn rejects_malformed_repos() {
        for s in [
            "",
            "waybar-gpui",
            "/name",
            "owner/",
            "a/b/c",
            "a/b c",
            "https://github.com/owner",
            "https://github.com/owner/name/pulls",
            "https://github.com/owner/name/tree/main",
            "https://github.com/owner/name/issues/12",
            "http://github.com/owner/name",
            "https://www.github.com/owner/name",
            "https://gitlab.com/owner/name",
            "https://github.example.com/owner/name",
            "git@gitlab.com:owner/name.git",
        ] {
            assert!(Repo::parse(s).is_err(), "{s:?}");
        }
    }

    #[test]
    fn says_what_forms_are_expected() {
        assert_eq!(
            Repo::parse("Cerclique").unwrap_err(),
            r#"expected owner/name or a github.com URL, got "Cerclique""#
        );
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
                r#"[{{"number":1,"title":"t","isDraft":false,"url":"u","statusCheckRollup":{rollup}}}]"#
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
        let skipped = check("COMPLETED", "SKIPPED");
        assert_eq!(
            pull(&format!("[{skipped},{skipped}]")),
            Some(CiState::Neutral(NeutralKind::Skipped))
        );
        assert_eq!(
            pull(&format!("[{skipped},{success}]")),
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
    fn parses_the_latest_release_from_gh_release_view() {
        let release = parse_release(include_str!("fixtures/release.json")).unwrap();
        assert_eq!(release.tag, "v2.104.0");
        assert_eq!(
            release.published_at,
            UNIX_EPOCH + Duration::from_secs(1_794_510_231)
        );
        assert_eq!(
            release.url,
            "https://github.com/cli/cli/releases/tag/v2.104.0"
        );
    }

    #[test]
    fn reads_release_dates_across_leap_days() {
        let published = |at: &str| {
            let json = format!(r#"{{"tagName":"v1","publishedAt":"{at}","url":"u"}}"#);
            parse_release(&json).ok().map(|r| r.published_at)
        };
        let epoch = |secs| Some(UNIX_EPOCH + Duration::from_secs(secs));
        assert_eq!(published("2024-02-29T23:59:59Z"), epoch(1_709_251_199));
        assert_eq!(published("2000-03-01T00:00:00Z"), epoch(951_868_800));
        assert_eq!(published("yesterday"), None);
        assert_eq!(published("2024-13-01T00:00:00Z"), None);
    }

    #[test]
    fn parses_issues_without_author_or_labels() {
        let json = r#"[{"number":7,"title":"t","url":"u"}]"#;
        let issue = &parse_issues(json).unwrap()[0];
        assert_eq!((issue.number, issue.title.as_str()), (7, "t"));
    }

    #[test]
    fn rejects_malformed_json() {
        assert!(parse_issues("not json").is_err());
    }

    #[test]
    fn a_rollup_is_neutral_only_when_every_check_is() {
        let skipped = CiState::Neutral(NeutralKind::Skipped);
        let cancelled = CiState::Neutral(NeutralKind::Cancelled);
        assert!(matches!(
            CiState::worst([skipped, cancelled]),
            Some(CiState::Neutral(_))
        ));
        assert_eq!(
            CiState::worst([skipped, CiState::Success, cancelled]),
            Some(CiState::Success)
        );
        assert_eq!(
            CiState::worst([CiState::Running, skipped]),
            Some(CiState::Running)
        );
        assert_eq!(
            CiState::worst([cancelled, CiState::Failure]),
            Some(CiState::Failure)
        );
    }

    #[test]
    fn labels_a_neutral_state_with_its_conclusion() {
        let label = |v| CiState::from_github(v).label();
        assert_eq!(label("CANCELLED"), "cancelled");
        assert_eq!(label("skipped"), "skipped");
        assert_eq!(label("neutral"), "neutral");
        assert_eq!(label("stale"), "stale");
        assert_eq!(label("success"), "passing");
    }

    #[test]
    fn maps_github_states_onto_ci_states() {
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
        for v in ["success", "SUCCESS"] {
            assert_eq!(CiState::from_github(v), CiState::Success, "{v}");
        }
        for (v, kind) in [
            ("cancelled", NeutralKind::Cancelled),
            ("skipped", NeutralKind::Skipped),
            ("neutral", NeutralKind::Neutral),
            ("stale", NeutralKind::Stale),
            ("SKIPPED", NeutralKind::Skipped),
        ] {
            assert_eq!(CiState::from_github(v), CiState::Neutral(kind), "{v}");
        }
    }
}
