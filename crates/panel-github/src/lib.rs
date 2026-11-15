//! The `github` panel: the state of GitHub repositories, as a Waybar popover.
//!
//! On-click: `waybar-gpui-panels toggle github [owner/name…]`. Without
//! repositories, the panel shows the watch list in
//! `$XDG_CONFIG_HOME/waybar-gpui-panels/github/repos`.

mod cache;
mod gh;
mod mocha;
mod repo_panel;
mod status;
mod watch_list;

pub use repo_panel::RepoPanel;
