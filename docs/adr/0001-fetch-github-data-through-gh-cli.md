# Fetch GitHub data through the `gh` CLI, cached by the daemon

The github panel gets its data by running `gh … --json` commands (`gh issue list`, `gh pr list`, `gh release view` and `gh workflow list` in parallel, then one `gh run list` per workflow, also in parallel) rather than calling GitHub's API itself. `gh` already handles authentication, so the panel stores no tokens and adds no HTTP or TLS dependencies. The cost is a process spawn per command (roughly 200–400 ms each, and one per workflow for its runs) and a runtime dependency on an installed, logged-in `gh`. The daemon keeps the last result per repository. An open shows the cached result immediately and refreshes it in the background, so the spawn cost is mostly hidden.

## Considered Options

- **One `gh api graphql` query.** One process and one round trip, and it covers PR check status. Rejected because Actions workflow runs don't map cleanly onto GraphQL, and the hand-maintained query would be the most complex part of the code.
- **Calling GitHub directly with `ureq` (REST or GraphQL).** Fastest, and it doesn't need `gh` at runtime. Rejected for now because it means handling tokens ourselves and adding more dependencies. It stays the migration path if spawning `gh` becomes a problem.
- **`octocrab`.** Requires a tokio runtime alongside GPUI's executor. A heavy dependency for little gain.

## Consequences

- All GitHub access goes through `status::load`, so switching to direct HTTP later changes only that module.
- The bar text is static. Data is fetched only when the popover opens. A live bar summary would need a polling path, which is not decided yet.
- In `preview` mode nothing is kept between opens, so every open fetches fresh data.
