# Waybar GitHub Viewer

Waybar popovers that show the state of GitHub repositories: open issues, open pull requests and CI.

## Language

**Repository**:
A GitHub repository, named `owner/name`. Names that differ only in case refer to the same repository.
_Avoid_: Repo URL, project

**Watch list**:
The single, ordered list of repositories the user keeps in their config file for the panel to show.
_Avoid_: Repo list, watched repos, favourites

**Panel arguments**:
Repositories given on the Waybar on-click command. When present, they replace the watch list for that open.
_Avoid_: Override list
