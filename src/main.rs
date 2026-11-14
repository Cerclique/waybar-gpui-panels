//! Waybar popovers showing a GitHub repository's status.
//!
//! On-click: `waybar-github-viewer toggle repo [owner/name…]`. Without
//! repositories, the panel shows the watch list in
//! `$XDG_CONFIG_HOME/waybar-github-viewer/repos`.

mod cache;
mod gh;
mod mocha;
mod repo_panel;
mod status;
mod watch_list;

fn main() -> std::process::ExitCode {
    waybar_gpui::Program::new("waybar-github-viewer")
        .panel::<repo_panel::RepoPanel>()
        .run()
}
