# Waybar GitHub Viewer

Waybar popovers that show the state of GitHub repositories: open issues, open pull requests and CI.

## Language

**Repository**:
A GitHub repository, named `owner/name`. Names that differ only in case refer to the same repository.
_Avoid_: Repo URL, project

**Release**:
A repository's latest published release, excluding drafts and prereleases.
_Avoid_: Latest tag, version

**Workflow**:
An active GitHub Actions workflow of a repository, shown with its last 20 runs on every branch. Disabled workflows are left out.
_Avoid_: Pipeline, job, action

**Last build**:
A workflow's newest run that isn't neutral (cancelled, skipped, ...), or its newest run when all are neutral. It gives the workflow its state, and a failing last build counts towards the CI tab's number.
_Avoid_: Latest run, workflow status

**Watch list**:
The single, ordered list of repositories the user keeps in their config file for the panel to show.
_Avoid_: Repo list, watched repos, favourites

**Panel arguments**:
Repositories given on the Waybar on-click command. When present, they replace the watch list for that open.
_Avoid_: Override list
