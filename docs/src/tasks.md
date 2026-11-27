# Tasks

The commands you run for a project — `make dev`, `npm run build`,
`terraform plan -var-file=staging.tfvars` — one key away.

**Tasks: Run Task…** (`Ctrl/⌘+Shift+B`) lists them; pick one and it runs in
a terminal tab of its own, named after it (`task dev`). You see its real
output; `Ctrl+C` stops it; afterwards the tab stays as a shell in that
folder. Running the same task again replaces its tab.
**Tasks: Run Last Task Again** runs the last one without asking.

## Where they come from

Qell reads them when you open the list — nothing is watched, and nothing
runs until you pick it. Looking from the open file's folder up to the
workspace:

| | Runs |
|---|---|
| every `.qell/tasks.toml` on the way | your own tasks, below |
| the nearest `Makefile` | `make <target>` — with the `## description` after a target, or the `#` comment above it |
| the nearest `package.json` | `npm run <script>` — or `pnpm`, `yarn` or `bun` when that's the lock file there |
| the nearest `justfile` | `just <recipe>` — public recipes, with the comment above |

So in a folder holding several projects, the tasks follow the file you're
in.

## Your own tasks

**Tasks: Configure Tasks** opens the workspace's `.qell/tasks.toml`
(creating it, with the tasks already found listed in a comment):

```toml
[[task]]
name = "api"
command = "go run ./cmd/api"
cwd = "api"                         # optional: below the folder holding .qell/
env = { PORT = "8080" }             # optional
description = "The API on :8080"    # optional

[[task]]
name = "plan staging"
command = "terraform plan -var-file=staging.tfvars"
cwd = "infra"
```

A command runs with `bash`, so pipes, `&&` and variables work. Commit the
file to share the tasks with your team, or add `.qell/` to `.gitignore` to
keep them yours.

When the [daily log](journal.md) is on, the tasks you run are part of the
day's activity.
