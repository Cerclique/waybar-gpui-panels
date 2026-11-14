//! The last status of each repository, kept by the daemon across opens and
//! refreshed in the background.
//!
//! A refresh outlives the popover that started it: its result lands here, and
//! panels observe [`Cache`] to redraw. In `preview` the program exits when its
//! popover closes, so nothing is kept between opens.

use std::{collections::HashMap, time::Instant};

use waybar_gpui::gpui::{App, Global};

use crate::{
    gh::GhError,
    status::{self, Repo, RepoStatus},
};

/// Every repository's [`Entry`], by lowercase `owner/name`: names differing
/// only in case are the same repository.
#[derive(Default)]
pub struct Cache {
    entries: HashMap<String, Entry>,
}

impl Global for Cache {}

/// What's known about one repository.
#[derive(Default)]
pub struct Entry {
    /// The last successful refresh, and when it finished.
    pub status: Option<(RepoStatus, Instant)>,
    /// Why the last refresh failed; cleared by a successful one.
    pub error: Option<GhError>,
    /// Whether a refresh is running.
    pub refreshing: bool,
    /// Fetched once, by the first refresh that succeeds in getting it.
    default_branch: Option<String>,
}

fn key(repo: &Repo) -> String {
    repo.to_string().to_lowercase()
}

impl Cache {
    /// `repo`'s entry, if it was ever refreshed.
    pub fn get<'a>(repo: &Repo, cx: &'a App) -> Option<&'a Entry> {
        cx.try_global::<Self>()?.entries.get(&key(repo))
    }

    /// Starts refreshing `repo` in the background, unless a refresh of it is
    /// already running: that one is reused.
    pub fn refresh(repo: &Repo, cx: &mut App) {
        let key = key(repo);
        let entry = cx
            .default_global::<Self>()
            .entries
            .entry(key.clone())
            .or_default();
        if entry.refreshing {
            return;
        }
        entry.refreshing = true;
        let branch = entry.default_branch.clone();
        let repo = repo.clone();
        cx.spawn(async move |cx| {
            let result = cx
                .background_executor()
                .spawn(async move { status::load(&repo, branch) })
                .await;
            cx.update_global::<Self, _>(|cache, _| {
                let entry = cache.entries.entry(key).or_default();
                entry.refreshing = false;
                match result {
                    Ok(loaded) => {
                        entry.default_branch = Some(loaded.default_branch);
                        entry.status = Some((loaded.status, Instant::now()));
                        entry.error = None;
                    }
                    Err(e) => entry.error = Some(e),
                }
            });
        })
        .detach();
    }
}
