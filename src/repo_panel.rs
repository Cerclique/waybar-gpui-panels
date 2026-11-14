//! The `repo` panel: open issues, open pull requests and CI of one repository
//! at a time, picked from a dropdown. Read-only: a click on a row opens it in
//! the browser and closes the popover.
//!
//! Its panel arguments are the repositories, `owner/name …`.

use std::collections::HashMap;

use crate::{
    mocha,
    status::{self, CiState, Repo, RepoStatus},
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
        self, AnyElement, App, Context, FontWeight, Global, IntoElement, Rgba, SharedString, Task,
        Window, div, prelude::*, px,
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

enum Load {
    Loading,
    Loaded(RepoStatus),
    Failed(String),
}

/// The last repository picked, kept across opens by the daemon.
#[derive(Default)]
struct LastRepo(Option<String>);
impl Global for LastRepo {}

pub struct RepoPanel {
    repos: Result<Vec<Repo>, String>,
    selected: usize,
    view: View,
    /// Per repository, loaded on first selection during this open.
    loads: HashMap<usize, Load>,
    /// Dropped with the panel on close, which abandons the fetches.
    fetches: Vec<Task<()>>,
}

impl Panel for RepoPanel {
    const NAME: &'static str = "repo";

    fn new(args: &[String], _: &mut Window, cx: &mut Context<Self>) -> Self {
        let repos = parse_repos(args);
        let selected = match &repos {
            Ok(repos) => initial_index(repos, cx.default_global::<LastRepo>().0.as_deref()),
            Err(_) => 0,
        };
        let mut panel = Self {
            repos,
            selected,
            view: View::Issues,
            loads: HashMap::new(),
            fetches: Vec::new(),
        };
        panel.select(selected, cx);
        panel
    }
}

impl RepoPanel {
    /// Shows repository `index`, fetching it if this open hasn't yet.
    fn select(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(repo) = self.repos.as_ref().ok().and_then(|r| r.get(index)).cloned() else {
            return;
        };
        self.selected = index;
        cx.global_mut::<LastRepo>().0 = Some(repo.to_string());
        if self.loads.contains_key(&index) {
            return;
        }
        self.loads.insert(index, Load::Loading);
        self.fetches.push(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { status::load(&repo) })
                .await;
            // An error here means the popover closed first: nothing to do.
            let _ = this.update(cx, |panel, cx| {
                let load = match result {
                    Ok(status) => Load::Loaded(status),
                    Err(e) => Load::Failed(e),
                };
                panel.loads.insert(index, load);
                cx.notify();
            });
        }));
    }
}

/// Every argument is an `owner/name`; at least one is needed.
fn parse_repos(args: &[String]) -> Result<Vec<Repo>, String> {
    if args.is_empty() {
        return Err("usage: repo <owner/name>…".to_owned());
    }
    args.iter().map(|a| Repo::parse(a)).collect()
}

/// The last picked repository if it's among `repos`, else the first.
fn initial_index(repos: &[Repo], last: Option<&str>) -> usize {
    last.and_then(|last| repos.iter().position(|r| r.to_string() == last))
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
        let status = match self.loads.get(&self.selected) {
            Some(Load::Loaded(status)) => status,
            Some(Load::Failed(e)) => return card.child(header).child(muted(theme, e.clone())),
            Some(Load::Loading) | None => {
                return card.child(header).child(muted(theme, "Loading…"));
            }
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
            .child(Tab::new().label(format!("Issues {}", status.issues.len())))
            .child(Tab::new().label(format!("PRs {}", status.pulls.len())))
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

    fn args(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_owned()).collect()
    }

    #[test]
    fn needs_at_least_one_valid_repo() {
        assert!(parse_repos(&[]).is_err());
        assert!(parse_repos(&args(&["a/b", "nope"])).is_err());
        assert_eq!(parse_repos(&args(&["a/b", "c/d"])).unwrap().len(), 2);
    }

    #[test]
    fn reopens_on_the_last_picked_repo() {
        let repos = parse_repos(&args(&["a/b", "c/d"])).unwrap();
        assert_eq!(initial_index(&repos, None), 0);
        assert_eq!(initial_index(&repos, Some("c/d")), 1);
        assert_eq!(initial_index(&repos, Some("gone/away")), 0);
    }
}
