# Daily log

Every day at work you say what you did. Qell can write it for you.

**Journal: Write Today's Log** (or **Write Log for Last Working Day**, for
Monday's stand-up) gathers what Qell recorded that day and asks an LLM of
your choice for a short stand-up note. It's saved as `YYYY-MM-DD.md` and
opened, rendered:

```markdown
# 2026-11-27 — Friday

Worked on receipts for the shop API and on the checkout Lambda.

## Done
- **shop**: receipts now show the total in euros; cart tests pass (SHOP-42).
- **infra**: raised the checkout Lambda's memory, planned on staging.

## Issues
- `terraform plan` failed twice on the missing `stage` variable before the fix.

---

## Activity
…the raw record, by repository
```

## What it's made from

For each repository you touched that day:

- **your commits** — read with `git log --author=<your email>`, so commits
  made in a terminal count too — and the branch you're on (ticket ids in
  branch names reach the summary);
- the **files you saved**, with how often;
- the **tests you ran** and how they went;
- the **tasks you ran**;
- the **git commands run from Qell** (commit, switch, stash…);
- the **commands you ran in Qell's terminals**, with their folder, the
  failures and the long ones. `ls`, `cd`, `git status` and the like are
  left out, and repeats are folded (`terraform plan → failed (1) ×2`).

## Which LLM

Set it under `[journal]` in the [configuration](configuration.md):

```toml
[journal]
command = "claude -p"       # or "ollama run llama3.1", "llm -m …"
language = "French"
dir = "~/notes/standup"
```

1. **`command`** — any program that reads a prompt on stdin and writes the
   answer on stdout: `claude -p`, `ollama run <model>`, `llm`… It's tried
   first.
2. **`api_key`** (or `$ANTHROPIC_API_KEY`) — Qell calls the Anthropic API
   with `model`.
3. **Neither** — the log holds the activity only, with a line saying how to
   set one.

If the LLM fails or takes more than three minutes, you still get the
activity and the reason. Writing a log again keeps the previous one as
`.md.bak`. **Journal: Open Logs Folder** shows them all.

## Privacy

- Everything stays **on your machine**, in files only you can read, and is
  forgotten after `keep_days` (60).
- **Nothing is sent anywhere until you ask for a log** — and then only the
  day's activity, to the LLM you chose.
- Secrets in commands are **masked**: `--password x`, `TOKEN=…`, `*_KEY=…`,
  `Authorization: Bearer …`, `https://user:pass@…`.
- A command typed with a **leading space** isn't kept (in bash, with
  `HISTCONTROL=ignorespace`).
- Terminal commands come from a small hook in the **zsh or bash Qell
  starts** — your `.zshrc` / `.bashrc` still load, and shells outside Qell
  aren't touched. `shell_integration = false` turns it off;
  `enabled = false` turns the whole journal off.
