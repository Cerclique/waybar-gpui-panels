# waybar-gpui-panels

GPUI popover panels for [Waybar](https://github.com/Alexays/Waybar) on
Hyprland, one crate per panel, served by a single `waybar-gpui-panels` binary.

## The github panel

A popover that shows the state of the GitHub repositories you follow: open issues, open pull
requests and CI. Click a bar module and the panel opens next to it; click a row
and it opens on GitHub.

It is built on [`waybar_gpui`](https://crates.io/crates/waybar_gpui) and
[GPUI](https://www.gpui.rs). The panel is read-only.

## What it shows

Pick a repository from the dropdown in the header, next to its latest release
(drafts and prereleases are ignored). Three tabs follow, each with a count:

- **Issues**: open issues, as number, title and status.
- **PRs**: open pull requests, in the same form.
- **CI**: a tree of the repository's active workflows. Each workflow carries
  the state of its last build (passing, failing, running, or a neutral state
  such as cancelled or skipped) and expands to its recent runs across all
  branches. The tab's number counts workflows whose last build is failing.

Issue and PR lists show up to 30 entries; a count such as `30+` means there
may be more. The header shows how old the data is. Opening the panel shows the
last result at once and refreshes it in the background; the refresh button
forces another.

## Requirements

- Hyprland and Waybar
- A Vulkan driver
- Rust 1.95+ (edition 2024), to build it
- The [GitHub CLI](https://cli.github.com), installed and logged in
  (`gh auth login`). The panel gets all its data through `gh` and stores no
  token itself (see [ADR 0001](docs/adr/0001-fetch-github-data-through-gh-cli.md)).

## Install

```sh
git clone https://github.com/Cerclique/waybar-gpui-panels
cd waybar-gpui-panels
cargo install --path crates/waybar-gpui-panels --locked      # into ~/.cargo/bin
```

Run the installed program without arguments to print the `on-click` line for
the panel:

```sh
waybar-gpui-panels
```

To try the panel without touching Waybar, open it at your cursor (Esc closes
it). Row clicks only open the browser when run through the daemon, not here:

```sh
waybar-gpui-panels preview github owner/name
```

## Choose the repositories

The **watch list** is a text file at
`$XDG_CONFIG_HOME/waybar-gpui-panels/github/repos` (usually
`~/.config/waybar-gpui-panels/github/repos`). One `owner/name` per line; blank lines
are ignored and `#` starts a comment. It is read on every open, so edits apply
without a restart.

```text
# my projects
Cerclique/waybar-gpui-panels
rust-lang/rust   # the compiler
```

Repository names that differ only in case count as the same repository, and a
repeated one is an error.

You can also pass repositories as arguments on the on-click command. They
replace the watch list for that open, which gives one bar module per set of
repositories.

## Waybar configuration

Add a module to `~/.config/waybar/config.jsonc` and list it in `modules-left`,
`modules-center` or `modules-right`. Use the path printed by the installed
program in `on-click`:

```jsonc
"custom/github": {
  "format": " GitHub",
  "tooltip": false,
  "on-click": "~/.cargo/bin/waybar-gpui-panels toggle github",
},
```

To show specific repositories from that module instead of the watch list:

```jsonc
"custom/github-work": {
  "format": " Work",
  "tooltip": false,
  "on-click": "~/.cargo/bin/waybar-gpui-panels toggle github acme/api acme/web",
},
```

Reload Waybar (`pkill -SIGUSR2 waybar`) and click the module. Clicking it again
closes the panel (if it was opened with the same arguments), as do Esc and a
click outside.

The bar text is static: data is fetched only when the popover opens.

## Commands

| Command | What |
|---|---|
| `toggle github [owner/name…]` | Open the panel, or close it if it is open with the same arguments. |
| `preview github [owner/name…]` | Open the panel from the terminal; exits when it closes. |
| `daemon` | Run the daemon in the foreground. It keeps the cache between opens. |
| `close` / `quit` | Close the open popover / stop the daemon. |

Run `waybar-gpui-panels --help` for the full list.

## Development

```sh
cargo test
cargo clippy --all-targets
```

Terms used in the code are defined in [GLOSSARY.md](GLOSSARY.md).

### Claude Code skill

`.claude/skills/waybar-gpui/SKILL.md` is a copy of the skill maintained in the
[waybar-gpui](https://github.com/Cerclique/waybar-gpui) repository
(`skills/waybar-gpui/SKILL.md`). It is currently the version from the latest
release, **v0.2.0**. Do not edit it here; update it from that repository.

## License

[MIT](LICENSE)
