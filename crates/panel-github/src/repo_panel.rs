//! The `github` panel: open issues, open pull requests and CI of one repository
//! at a time, picked from a dropdown, with its latest release in the header.
//! Read-only: a click on a row opens it in the browser and closes the popover.
//! CI is a tree of workflows, each expanding to its recent runs.
//!
//! It shows the watch list (see [`watch_list`]), or its panel arguments,
//! `owner/name …`, when given.
//!
//! Data comes from the daemon's [`Cache`]: an open shows the cached status at
//! once and refreshes it in the background.

use std::{
    collections::HashSet,
    time::{Duration, SystemTime},
};

use crate::{
    cache::{Cache, Entry},
    mocha,
    status::{CiState, LIST_LIMIT, Release, Repo, Workflow},
    watch_list,
};
use waybar_gpui::{
    Panel, Theme, card, close,
    components::{
        Icon, IconName, Sizable as _,
        button::{Button, ButtonVariants as _, DropdownButton},
        menu::PopupMenuItem,
        tab::{Tab, TabBar},
        tooltip::Tooltip,
    },
    gpui::{
        self, AnyElement, App, ClickEvent, Context, Entity, FontWeight, Global, IntoElement, Rgba,
        SharedString, Subscription, Task, Window, div, prelude::*, px,
    },
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Issues,
    Pulls,
    Ci,
}

impl View {
    const ALL: [Self; 3] = [Self::Issues, Self::Pulls, Self::Ci];
}

/// The last repository picked, kept across opens by the daemon.
#[derive(Default)]
struct LastRepo(Option<Repo>);
impl Global for LastRepo {}

/// The CI workflows expanded in the tree, by repository key and workflow id,
/// kept across opens by the daemon. Collapsed unless listed.
#[derive(Default)]
struct ExpandedWorkflows(HashSet<(String, u64)>);
impl Global for ExpandedWorkflows {}

impl ExpandedWorkflows {
    fn contains(&self, repo: &Repo, workflow: u64) -> bool {
        self.0.contains(&(repo.key(), workflow))
    }

    fn toggle(&mut self, repo: &Repo, workflow: u64) {
        let key = (repo.key(), workflow);
        if !self.0.remove(&key) {
            self.0.insert(key);
        }
    }
}

pub struct RepoPanel {
    repos: Result<Vec<Repo>, String>,
    selected: usize,
    view: View,
    /// Repositories refreshed during this open: each at most once.
    refreshed: HashSet<usize>,
    /// Workflows showing all their runs, by repository index and workflow
    /// id: only for this open.
    all_runs: HashSet<(usize, u64)>,
    /// Redraws when a refresh lands in the cache.
    _cache_changed: Subscription,
    /// Redraws every second, to keep the data's age current.
    _tick: Task<()>,
}

impl Panel for RepoPanel {
    const NAME: &'static str = "github";

    fn new(args: &[String], _: &mut Window, cx: &mut Context<Self>) -> Self {
        let repos = watch_list::load(args, watch_list::default_path().as_deref());
        let selected = match &repos {
            Ok(repos) => initial_index(repos, cx.default_global::<LastRepo>().0.as_ref()),
            Err(_) => 0,
        };
        let _tick = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return; // the popover closed
                }
            }
        });
        let mut panel = Self {
            repos,
            selected,
            view: View::Issues,
            refreshed: HashSet::new(),
            all_runs: HashSet::new(),
            _cache_changed: cx.observe_global::<Cache>(|_, cx| cx.notify()),
            _tick,
        };
        panel.select(selected, cx);
        panel
    }
}

impl RepoPanel {
    /// Shows repository `index`, refreshing it if this open hasn't yet.
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.as_ref().ok().and_then(|r| r.get(index)) else {
            return;
        };
        self.selected = index;
        cx.global_mut::<LastRepo>().0 = Some(repo.clone());
        if self.refreshed.insert(index) {
            Cache::refresh(repo, cx);
        }
    }
}

/// The last picked repository if it's among `repos`, else the first.
fn initial_index(repos: &[Repo], last: Option<&Repo>) -> usize {
    last.and_then(|last| repos.iter().position(|r| r.is_same(last)))
        .unwrap_or(0)
}

impl Render for RepoPanel {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = Theme::get(cx);
        let card = card(cx).w(px(400.)).max_h(px(480.));
        let repos = match &self.repos {
            Ok(repos) => repos,
            Err(e) => return card.child(muted(theme, e.clone())),
        };
        let repo = &repos[self.selected];
        let header = header(repos, self.selected, cx);
        let refresh = refresh_button(repo, cx);
        let entry = Cache::get(repo, cx);
        let status = entry
            .and_then(|e| e.status.as_ref())
            .map(|(status, _)| status);
        let header = header
            .children(
                status
                    .and_then(|s| s.release.as_ref())
                    .map(|r| release_badge(theme, r)),
            )
            .child(freshness(theme, entry))
            .child(refresh.loading(entry.is_some_and(|e| e.refreshing)));
        let Some(entry) = entry else {
            return card.child(header).child(muted(theme, "Loading…"));
        };
        let Some(status) = status else {
            // Nothing cached: the error, if any, fills the panel.
            let message = match &entry.error {
                Some(e) => e.to_string(),
                None => "Loading…".to_owned(),
            };
            return card.child(header).child(muted(theme, message));
        };
        let entity = cx.entity();
        let failing = status
            .workflows
            .iter()
            .filter(|w| w.last_build() == Some(CiState::Failure))
            .count();
        let tab = |label: &'static str, count: String, alert: bool, view: View| {
            let red = mocha::red();
            let (bg, fg) = if alert {
                (Rgba { a: 0.18, ..red }, red)
            } else if view == self.view {
                (
                    Rgba {
                        a: 0.18,
                        ..theme.accent
                    },
                    theme.accent,
                )
            } else {
                (theme.surface, theme.subtext)
            };
            // The count is a child, not a `suffix`: a suffix sits at the tab's
            // right edge, outside the centred label.
            Tab::new()
                .flex_1()
                .label(label)
                .child(div().ml_1p5().child(pill(bg, fg, count)))
        };
        let tabs = TabBar::new("views")
            .w_full()
            .underline()
            .small()
            .selected_index(View::ALL.iter().position(|v| *v == self.view).unwrap_or(0))
            .on_click({
                let entity = entity.clone();
                move |index, _, cx| {
                    entity.update(cx, |panel, cx| {
                        panel.view = View::ALL[*index];
                        cx.notify();
                    })
                }
            })
            .child(tab(
                "Issues",
                count_label(status.issues.len()),
                false,
                View::Issues,
            ))
            .child(tab(
                "PRs",
                count_label(status.pulls.len()),
                false,
                View::Pulls,
            ))
            .child(tab("CI", failing.to_string(), failing > 0, View::Ci));

        let rows: Vec<AnyElement> = match self.view {
            View::Issues => status
                .issues
                .iter()
                .map(|i| {
                    link_row(theme, format!("issue-{}", i.number), &i.url)
                        .child(number(theme.subtext, i.number))
                        .child(title(format!("issue-title-{}", i.number), &i.title))
                        .into_any_element()
                })
                .collect(),
            View::Pulls => status
                .pulls
                .iter()
                .map(|p| {
                    // A draft is greyed out instead of carrying a pill.
                    let draft_colour = p.draft.then(mocha::overlay0);
                    link_row(theme, format!("pr-{}", p.number), &p.url)
                        .child(number(draft_colour.unwrap_or(theme.subtext), p.number))
                        .child(
                            title(format!("pr-title-{}", p.number), &p.title)
                                .when_some(draft_colour, |t, colour| t.text_color(colour)),
                        )
                        .child(checks_pill(theme, p.checks))
                        .into_any_element()
                })
                .collect(),
            View::Ci => {
                let expanded = cx.try_global::<ExpandedWorkflows>();
                let mut rows = Vec::new();
                for workflow in &status.workflows {
                    let open = expanded.is_some_and(|e| e.contains(repo, workflow.id));
                    rows.push(workflow_row(theme, repo, workflow, open, &entity));
                    if open {
                        let all = self.all_runs.contains(&(self.selected, workflow.id));
                        rows.extend(run_rows(theme, workflow, all, self.selected, &entity));
                    }
                }
                rows
            }
        };
        let empty = match self.view {
            _ if !rows.is_empty() => None,
            View::Issues | View::Pulls => Some("Nothing open"),
            View::Ci => Some("No workflows"),
        };

        card.child(header)
            .child(tabs)
            .child(
                div()
                    .id("rows")
                    .flex()
                    .flex_col()
                    .gap_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .when_some(empty, |c, message| c.child(muted(theme, message)))
    }
}

/// How old the shown data is.
fn age_label(age: Duration) -> String {
    let secs = age.as_secs();
    match secs {
        0..60 => format!("updated {secs}s ago"),
        60..3600 => format!("updated {}m ago", secs / 60),
        _ => format!("updated {}h ago", secs / 3600),
    }
}

/// How long ago something happened, rounded down to its largest unit:
/// `now`, `5m`, `3h`, `2d`, `1w`, `4mo`, `1y`.
fn short_age(age: Duration) -> String {
    let mins = age.as_secs() / 60;
    let hours = mins / 60;
    let days = hours / 24;
    match days {
        _ if mins == 0 => "now".to_owned(),
        _ if hours == 0 => format!("{mins}m"),
        0 => format!("{hours}h"),
        1..7 => format!("{days}d"),
        7..28 => format!("{}w", days / 7),
        28..360 => format!("{}mo", (days / 30).max(1)),
        _ => format!("{}y", (days / 365).max(1)),
    }
}

/// How many runs of a workflow with `len` of them are shown, and how many
/// are left behind "Show more": the first [`SHOWN_RUNS`] until `all`.
fn shown_runs(len: usize, all: bool) -> (usize, usize) {
    let shown = if all { len } else { len.min(SHOWN_RUNS) };
    (shown, len - shown)
}

/// How many runs an expanded workflow shows before "Show more".
const SHOWN_RUNS: usize = 5;

/// A list's length, as `30+` when it hit the fetch limit and may have more.
fn count_label(len: usize) -> String {
    if len >= LIST_LIMIT {
        format!("{LIST_LIMIT}+")
    } else {
        len.to_string()
    }
}

/// The header's note on the shown data: its age, and the last refresh's
/// error while cached data stays on screen. Fills the header's free space.
fn freshness(theme: Theme, entry: Option<&Entry>) -> impl IntoElement {
    let cached = entry.and_then(|e| e.status.as_ref());
    let error = entry
        .and_then(|e| e.error.as_ref())
        .filter(|_| cached.is_some());
    div()
        .flex()
        .flex_1()
        .justify_end()
        .gap_2()
        .text_xs()
        .text_color(theme.subtext)
        .when_some(cached, |d, (_, at)| d.child(age_label(at.elapsed())))
        .when_some(error, |d, e| {
            d.child(div().text_color(mocha::red()).child(e.to_string()))
        })
}

/// The repository picker, then whatever the caller adds on the right.
fn header(repos: &[Repo], selected: usize, cx: &mut Context<RepoPanel>) -> gpui::Div {
    let picked = repos[selected].to_string();
    let picker: AnyElement = if repos.len() == 1 {
        div()
            .font_weight(FontWeight::BOLD)
            .child(picked)
            .into_any_element()
    } else {
        let entity = cx.entity();
        let names: Vec<String> = repos.iter().map(Repo::to_string).collect();
        DropdownButton::new("repos")
            .button(Button::new("repos-btn").small().label(picked))
            .dropdown_menu(move |menu, _, _| {
                names.iter().enumerate().fold(menu, |menu, (index, name)| {
                    let entity = entity.clone();
                    menu.item(
                        PopupMenuItem::new(name.clone())
                            .checked(index == selected)
                            .on_click(move |_, _, cx| {
                                entity.update(cx, |panel, cx| {
                                    panel.select(index, cx);
                                    cx.notify();
                                })
                            }),
                    )
                })
            })
            .into_any_element()
    };
    div()
        .flex()
        .items_center()
        .justify_between()
        .gap_2()
        .pb_1()
        .child(picker)
}

/// Refreshes `repo` now, even if this open already did. Its caller sets
/// `loading` while a refresh runs.
fn refresh_button(repo: &Repo, cx: &mut Context<RepoPanel>) -> Button {
    let entity = cx.entity();
    let repo = repo.clone();
    Button::new("refresh")
        .ghost()
        .xsmall()
        .icon(IconName::RefreshCw)
        .tooltip("Refresh")
        .on_click(move |_, _, cx| {
            entity.update(cx, |_, cx| {
                Cache::refresh(&repo, cx);
                cx.notify();
            })
        })
}

/// The latest release's tag and age; opens the release page on click.
fn release_badge(theme: Theme, release: &Release) -> impl IntoElement {
    let blue = mocha::blue();
    let age = SystemTime::now()
        .duration_since(release.published_at)
        .unwrap_or_default();
    div()
        .id("release")
        .flex()
        .gap_1()
        .px_1p5()
        .rounded_md()
        .text_xs()
        .bg(Rgba { a: 0.18, ..blue })
        .text_color(blue)
        .cursor_pointer()
        .hover(|style| style.bg(Rgba { a: 0.32, ..blue }))
        .on_click(open_and_close(&release.url))
        .child(release.tag.clone())
        .child(
            div()
                .text_color(theme.subtext)
                .child(format!("· {}", short_age(age))),
        )
}

/// A click handler that opens `url` in the browser, then closes the popover.
fn open_and_close(url: &str) -> impl Fn(&ClickEvent, &mut Window, &mut App) + 'static {
    let url = url.to_owned();
    move |_, _, cx| {
        cx.open_url(&url);
        close(cx);
    }
}

/// A clickable row, highlighted on hover.
fn row(theme: Theme, id: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(id))
        .flex()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded_md()
        .cursor_pointer()
        .hover(|style| style.bg(theme.surface))
}

/// A row that opens `url` in the browser on click, then closes the popover.
fn link_row(theme: Theme, id: String, url: &str) -> gpui::Stateful<gpui::Div> {
    row(theme, id).on_click(open_and_close(url))
}

/// A workflow's line in the CI tree: a chevron, its name and the pill of its
/// last meaningful build. A click expands or collapses it, remembered in
/// [`ExpandedWorkflows`].
fn workflow_row(
    theme: Theme,
    repo: &Repo,
    workflow: &Workflow,
    open: bool,
    entity: &Entity<RepoPanel>,
) -> AnyElement {
    let (repo, id, entity) = (repo.clone(), workflow.id, entity.clone());
    let chevron = if open {
        IconName::ChevronDown
    } else {
        IconName::ChevronRight
    };
    let state = match workflow.last_build() {
        Some(state) => ci_pill(theme, state).into_any_element(),
        None => pill(theme.surface, mocha::overlay0(), "never run").into_any_element(),
    };
    row(theme, format!("workflow-{id}"))
        .on_click(move |_, _, cx| {
            cx.default_global::<ExpandedWorkflows>().toggle(&repo, id);
            entity.update(cx, |_, cx| cx.notify());
        })
        .child(
            Icon::new(chevron)
                .xsmall()
                .flex_shrink_0()
                .text_color(theme.subtext),
        )
        .child(
            title(format!("workflow-name-{id}"), &workflow.name).font_weight(FontWeight::SEMIBOLD),
        )
        .child(state)
        .into_any_element()
}

/// An expanded workflow's runs, indented under its name: the first
/// [`SHOWN_RUNS`], then a "Show N more" row, unless `all`.
fn run_rows(
    theme: Theme,
    workflow: &Workflow,
    all: bool,
    selected: usize,
    entity: &Entity<RepoPanel>,
) -> Vec<AnyElement> {
    let id = workflow.id;
    if workflow.runs.is_empty() {
        return vec![
            div()
                .pl_7()
                .py_1()
                .child(muted(theme, "No runs yet"))
                .into_any_element(),
        ];
    }
    let (shown, more) = shown_runs(workflow.runs.len(), all);
    let now = SystemTime::now();
    let mut rows: Vec<AnyElement> = workflow.runs[..shown]
        .iter()
        .enumerate()
        .map(|(n, run)| {
            let age = now.duration_since(run.created_at).unwrap_or_default();
            link_row(theme, format!("run-{id}-{n}"), &run.url)
                .pl_7()
                .child(title(format!("run-branch-{id}-{n}"), &run.branch))
                .child(
                    div()
                        .flex_shrink_0()
                        .text_xs()
                        .text_color(theme.subtext)
                        .child(short_age(age)),
                )
                .child(ci_pill(theme, run.state))
                .into_any_element()
        })
        .collect();
    if more > 0 {
        let entity = entity.clone();
        rows.push(
            row(theme, format!("more-{id}"))
                .pl_7()
                .text_xs()
                .text_color(theme.subtext)
                .on_click(move |_, _, cx| {
                    entity.update(cx, |panel, cx| {
                        panel.all_runs.insert((selected, id));
                        cx.notify();
                    })
                })
                .child(format!("Show {more} more"))
                .into_any_element(),
        );
    }
    rows
}

fn number(colour: Rgba, number: u64) -> impl IntoElement {
    div().text_color(colour).child(format!("#{number}"))
}

/// A title that takes the row's leftover width, cut with an ellipsis, and
/// shows in full in a tooltip on hover.
fn title(id: String, title: &str) -> gpui::Stateful<gpui::Div> {
    let full = SharedString::from(title.to_owned());
    div()
        .id(SharedString::from(id))
        .flex_1()
        .min_w_0()
        .truncate()
        .child(full.clone())
        .tooltip(move |window, cx| Tooltip::new(full.clone()).build(window, cx))
}

/// A small rounded label, kept at its natural width.
fn pill(bg: Rgba, fg: Rgba, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .flex_shrink_0()
        .whitespace_nowrap()
        .px_1p5()
        .rounded_md()
        .text_xs()
        .bg(bg)
        .text_color(fg)
        .child(text.into())
}

fn ci_pill(theme: Theme, state: CiState) -> impl IntoElement {
    let tinted = |colour: Rgba| (Rgba { a: 0.18, ..colour }, colour);
    let (bg, fg) = match state {
        CiState::Neutral(_) => (theme.surface, mocha::overlay0()),
        CiState::Success => tinted(mocha::green()),
        CiState::Running => tinted(mocha::yellow()),
        CiState::Failure => tinted(mocha::red()),
    };
    pill(bg, fg, state.label())
}

/// A pull request's checks; a grey `no checks` pill without CI, so every
/// row ends in a pill.
fn checks_pill(theme: Theme, checks: Option<CiState>) -> AnyElement {
    match checks {
        Some(state) => ci_pill(theme, state).into_any_element(),
        None => pill(theme.surface, mocha::overlay0(), "no checks").into_any_element(),
    }
}

fn muted(theme: Theme, text: impl Into<SharedString>) -> impl IntoElement {
    div().text_color(theme.subtext).child(text.into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn says_how_old_the_data_is() {
        let secs = Duration::from_secs;
        assert_eq!(age_label(secs(0)), "updated 0s ago");
        assert_eq!(age_label(secs(12)), "updated 12s ago");
        assert_eq!(age_label(secs(59)), "updated 59s ago");
        assert_eq!(age_label(secs(60)), "updated 1m ago");
        assert_eq!(age_label(secs(3599)), "updated 59m ago");
        assert_eq!(age_label(secs(7200)), "updated 2h ago");
    }

    #[test]
    fn rounds_an_age_down_to_its_largest_unit() {
        let secs = Duration::from_secs;
        let mins = |m: u64| secs(m * 60);
        let hours = |h: u64| mins(h * 60);
        let days = |d: u64| hours(d * 24);
        let cases = [
            (mins(0), "now"),
            (secs(59), "now"),
            (mins(1), "1m"),
            (mins(59), "59m"),
            (hours(1), "1h"),
            (hours(23), "23h"),
            (hours(24), "1d"),
            (days(6), "6d"),
            (days(7), "1w"),
            (days(27), "3w"),
            (days(28), "1mo"),
            (days(59), "1mo"),
            (days(60), "2mo"),
            (days(359), "11mo"),
            (days(360), "1y"),
            (days(729), "1y"),
            (days(730), "2y"),
        ];
        for (age, shown) in cases {
            assert_eq!(short_age(age), shown, "{age:?}");
        }
    }

    #[test]
    fn remembers_expanded_workflows_per_repo() {
        let repo = |s| Repo::parse(s).unwrap();
        let mut expanded = ExpandedWorkflows::default();
        assert!(!expanded.contains(&repo("a/b"), 7));
        expanded.toggle(&repo("a/b"), 7);
        assert!(expanded.contains(&repo("a/b"), 7));
        assert!(expanded.contains(&repo("A/B"), 7));
        assert!(!expanded.contains(&repo("a/b"), 8));
        assert!(!expanded.contains(&repo("c/d"), 7));
        expanded.toggle(&repo("A/b"), 7);
        assert!(!expanded.contains(&repo("a/b"), 7));
    }

    #[test]
    fn shows_five_runs_then_offers_the_rest() {
        assert_eq!(shown_runs(20, false), (5, 15));
        assert_eq!(shown_runs(6, false), (5, 1));
        assert_eq!(shown_runs(5, false), (5, 0));
        assert_eq!(shown_runs(3, false), (3, 0));
        assert_eq!(shown_runs(0, false), (0, 0));
        assert_eq!(shown_runs(20, true), (20, 0));
    }

    #[test]
    fn a_full_list_may_have_more() {
        assert_eq!(count_label(0), "0");
        assert_eq!(count_label(29), "29");
        assert_eq!(count_label(30), "30+");
    }

    #[test]
    fn reopens_on_the_last_picked_repo() {
        let repo = |s| Repo::parse(s).unwrap();
        let repos = [repo("a/b"), repo("c/d")];
        assert_eq!(initial_index(&repos, None), 0);
        assert_eq!(initial_index(&repos, Some(&repo("c/d"))), 1);
        assert_eq!(initial_index(&repos, Some(&repo("gone/away"))), 0);
        assert_eq!(initial_index(&repos, Some(&repo("C/D"))), 1);
    }
}
