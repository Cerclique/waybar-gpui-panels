//! The `repo` panel: open issues, open pull requests and CI of one repository
//! at a time, picked from a dropdown, with its latest release in the header.
//! Read-only: a click on a row opens it in the browser and closes the popover.
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
    status::{CiState, LIST_LIMIT, Release, Repo},
    watch_list,
};
use waybar_gpui::{
    Panel, Theme, card, close,
    components::{
        IconName, Sizable as _,
        button::{Button, ButtonVariants as _, DropdownButton},
        menu::PopupMenuItem,
        tab::{Tab, TabBar},
        tooltip::Tooltip,
    },
    gpui::{
        self, AnyElement, App, ClickEvent, Context, FontWeight, Global, IntoElement, Rgba,
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

pub struct RepoPanel {
    repos: Result<Vec<Repo>, String>,
    selected: usize,
    view: View,
    /// Repositories refreshed during this open: each at most once.
    refreshed: HashSet<usize>,
    /// Redraws when a refresh lands in the cache.
    _cache_changed: Subscription,
    /// Redraws every second, to keep the data's age current.
    _tick: Task<()>,
}

impl Panel for RepoPanel {
    const NAME: &'static str = "repo";

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
        let tab = |label: &'static str, count: String, view: View| {
            let (bg, fg) = if view == self.view {
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
            .on_click(move |index, _, cx| {
                entity.update(cx, |panel, cx| {
                    panel.view = View::ALL[*index];
                    cx.notify();
                })
            })
            .child(tab(
                "Issues",
                count_label(status.issues.len()),
                View::Issues,
            ))
            .child(tab("PRs", count_label(status.pulls.len()), View::Pulls))
            .child(tab("CI", status.runs.len().to_string(), View::Ci));

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
                    // A draft is muted instead of carrying a pill.
                    let muted = if p.draft {
                        Some(mocha::overlay0())
                    } else {
                        None
                    };
                    link_row(theme, format!("pr-{}", p.number), &p.url)
                        .child(number(muted.unwrap_or(theme.subtext), p.number))
                        .child(
                            title(format!("pr-title-{}", p.number), &p.title)
                                .when_some(muted, |t, colour| t.text_color(colour)),
                        )
                        .child(checks_pill(theme, p.checks))
                        .into_any_element()
                })
                .collect(),
            View::Ci => status
                .runs
                .iter()
                .enumerate()
                .map(|(n, run)| {
                    link_row(theme, format!("run-{n}"), &run.url)
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(run.workflow.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_color(theme.subtext)
                                .child(run.branch.clone()),
                        )
                        .child(ci_pill(run.state))
                        .into_any_element()
                })
                .collect(),
        };
        let empty = rows.is_empty();

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
            .when(empty, |c| c.child(muted(theme, "Nothing open")))
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

/// How long ago a release was published, rounded down to its largest unit.
fn release_age(age: Duration) -> String {
    let hours = age.as_secs() / 3600;
    let days = hours / 24;
    match days {
        _ if hours == 0 => "now".to_owned(),
        0 => format!("{hours}h"),
        1..7 => format!("{days}d"),
        7..28 => format!("{}w", days / 7),
        28..360 => format!("{}mo", (days / 30).max(1)),
        _ => format!("{}y", (days / 365).max(1)),
    }
}

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
                .child(format!("· {}", release_age(age))),
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

/// A row that opens `url` in the browser on click, then closes the popover.
fn link_row(theme: Theme, id: String, url: &str) -> gpui::Stateful<gpui::Div> {
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
        .on_click(open_and_close(url))
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
        .tooltip({
            let full = full.clone();
            move |window, cx| Tooltip::new(full.clone()).build(window, cx)
        })
        .child(full)
}

/// A small rounded label.
fn pill(bg: Rgba, fg: Rgba, text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .px_1p5()
        .rounded_md()
        .text_xs()
        .bg(bg)
        .text_color(fg)
        .child(text.into())
}

fn ci_pill(state: CiState) -> impl IntoElement {
    let colour = match state {
        CiState::Success => mocha::green(),
        CiState::Running => mocha::yellow(),
        CiState::Failure => mocha::red(),
    };
    pill(Rgba { a: 0.18, ..colour }, colour, state.label())
}

/// A pull request's checks; a grey `no checks` pill without CI, so every
/// row ends in a pill.
fn checks_pill(theme: Theme, checks: Option<CiState>) -> AnyElement {
    match checks {
        Some(state) => ci_pill(state).into_any_element(),
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
    fn rounds_a_release_age_down_to_its_largest_unit() {
        let mins = |m: u64| Duration::from_secs(m * 60);
        let hours = |h: u64| mins(h * 60);
        let days = |d: u64| hours(d * 24);
        let cases = [
            (mins(59), "now"),
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
            assert_eq!(release_age(age), shown, "{age:?}");
        }
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
