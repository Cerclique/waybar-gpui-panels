//! The `repo` panel: open issues, open pull requests and CI of one repository
//! at a time, picked from a dropdown. Read-only: a click on a row opens it in
//! the browser and closes the popover.
//!
//! It shows the watch list (see [`watch_list`]), or its panel arguments,
//! `owner/name …`, when given.
//!
//! Data comes from the daemon's [`Cache`]: an open shows the cached status at
//! once and refreshes it in the background.

use std::{collections::HashSet, env, time::Duration};

use crate::{
    cache::{Cache, Entry},
    mocha,
    status::{CiState, LIST_LIMIT, Repo},
    watch_list,
};
use waybar_gpui::{
    Panel, Theme, card, close,
    components::{
        Sizable as _,
        button::{Button, DropdownButton},
        menu::PopupMenuItem,
        tab::{Tab, TabBar},
    },
    gpui::{
        self, AnyElement, App, Context, FontWeight, Global, IntoElement, Rgba, SharedString,
        Subscription, Task, Window, div, prelude::*, px,
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
        let path = watch_list::path(
            env::var_os("XDG_CONFIG_HOME").as_deref(),
            env::home_dir().as_deref(),
        );
        let repos = watch_list::load(args, path.as_deref());
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
        let header = header(repos, self.selected, cx);
        let Some(entry) = Cache::get(&repos[self.selected], cx) else {
            return card.child(header).child(muted(theme, "Loading…"));
        };
        let header = header.child(freshness(theme, entry));
        let Some((status, _)) = &entry.status else {
            // Nothing cached: the error, if any, fills the panel.
            let message = match &entry.error {
                Some(e) => e.to_string(),
                None => "Loading…".to_owned(),
            };
            return card.child(header).child(muted(theme, message));
        };
        let entity = cx.entity();
        let tabs = TabBar::new("views")
            .underline()
            .small()
            .selected_index(View::ALL.iter().position(|v| *v == self.view).unwrap_or(0))
            .on_click(move |index, _, cx| {
                entity.update(cx, |panel, cx| {
                    panel.view = View::ALL[*index];
                    cx.notify();
                })
            })
            .child(Tab::new().label(format!("Issues {}", count_label(status.issues.len()))))
            .child(Tab::new().label(format!("PRs {}", count_label(status.pulls.len()))))
            .child(Tab::new().label(format!("CI {}", status.runs.len())));

        let rows: Vec<AnyElement> = match self.view {
            View::Issues => status
                .issues
                .iter()
                .map(|i| {
                    link_row(theme, format!("issue-{}", i.number), &i.url)
                        .child(number(theme, i.number))
                        .child(title(&i.title))
                        .children(
                            i.labels
                                .iter()
                                .map(|l| pill(theme.surface, theme.subtext, l.clone())),
                        )
                        .child(author(theme, &i.author))
                        .into_any_element()
                })
                .collect(),
            View::Pulls => status
                .pulls
                .iter()
                .map(|p| {
                    link_row(theme, format!("pr-{}", p.number), &p.url)
                        .child(number(theme, p.number))
                        .child(title(&p.title))
                        .when(p.draft, |r| {
                            r.child(pill(theme.surface, mocha::overlay0(), "draft"))
                        })
                        .when_some(p.checks, |r, state| r.child(ci_pill(state)))
                        .child(author(theme, &p.author))
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

        card.child(header.children(status.ci_summary().map(ci_pill)))
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

/// A list's length, as `30+` when it hit the fetch limit and may have more.
fn count_label(len: usize) -> String {
    if len >= LIST_LIMIT {
        format!("{LIST_LIMIT}+")
    } else {
        len.to_string()
    }
}

/// The header's note on the shown data: its age, "updating…" while a refresh
/// runs, and the last refresh's error while cached data stays on screen.
fn freshness(theme: Theme, entry: &Entry) -> impl IntoElement {
    let cached = entry.status.as_ref();
    div()
        .flex()
        .flex_1()
        .justify_end()
        .gap_2()
        .text_xs()
        .text_color(theme.subtext)
        .when_some(cached, |d, (_, at)| d.child(age_label(at.elapsed())))
        .when(entry.refreshing, |d| d.child("updating…"))
        .when_some(entry.error.as_ref().filter(|_| cached.is_some()), |d, e| {
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

/// A row that opens `url` in the browser on click, then closes the popover.
fn link_row(theme: Theme, id: String, url: &str) -> gpui::Stateful<gpui::Div> {
    let url = url.to_owned();
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
        .on_click(move |_, _, cx: &mut App| {
            cx.open_url(&url);
            close(cx);
        })
}

fn number(theme: Theme, number: u64) -> impl IntoElement {
    div().text_color(theme.subtext).child(format!("#{number}"))
}

fn title(title: &str) -> impl IntoElement {
    div().flex_1().min_w_0().truncate().child(title.to_owned())
}

fn author(theme: Theme, author: &str) -> impl IntoElement {
    div()
        .text_xs()
        .text_color(theme.subtext)
        .child(author.to_owned())
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
