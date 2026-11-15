//! Waybar popovers, one panel per crate.
//!
//! On-click: `waybar-gpui-panels toggle <panel> [args…]`. Run without arguments to
//! list the registered panels.

fn main() -> std::process::ExitCode {
    waybar_gpui::Program::new("waybar-gpui-panels")
        .panel::<panel_github::RepoPanel>()
        .run()
}
