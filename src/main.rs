//! Waybar popovers showing a GitHub repository's status.
//!
//! On-click: `waybar-github-viewer toggle repo <owner/name>…`.

mod cache;
mod gh;
mod mocha;
mod repo_panel;
mod status;

fn main() -> std::process::ExitCode {
    waybar_gpui::Program::new("waybar-github-viewer")
        .panel::<repo_panel::RepoPanel>()
        .run()
}
