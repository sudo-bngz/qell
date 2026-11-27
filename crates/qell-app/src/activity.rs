//! The activity journal's record: what you did in Qell, kept on your
//! machine for the daily log (Journal: Write Today's Log).
//!
//! - Qell notes saves, test runs and the git commands it runs itself,
//!   one JSON line each in `<data>/qell/activity/events.jsonl`.
//! - Qell's own terminals report each command (folder, exit status,
//!   duration) through a small hook added to the shell they start (zsh and
//!   bash), into `commands.tsv` next to it. Shells you open outside Qell
//!   are not touched. A command typed with a leading space isn't kept, and
//!   values that look like secrets are masked.
//!
//! Nothing is sent anywhere: the log is only read when you ask for a
//! summary.

use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Where the activity is kept.
pub fn dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("qell").join("activity")
}

pub fn events_path() -> PathBuf {
    dir().join("events.jsonl")
}

pub fn commands_path() -> PathBuf {
    dir().join("commands.tsv")
}

pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64)
}

/// One thing done in Qell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Event {
    /// A file saved.
    Save { t: i64, path: PathBuf },
    /// Tests run: what, and how it went.
    Tests { t: i64, file: PathBuf, command: String, passed: usize, failed: usize, ok: bool },
    /// A git command Qell ran (commit, switch, stash…), with what it said.
    Git { t: i64, repo: PathBuf, what: String },
}

impl Event {
    pub fn time(&self) -> i64 {
        match self {
            Event::Save { t, .. } | Event::Tests { t, .. } | Event::Git { t, .. } => *t,
        }
    }
}

/// Add an event to the record (when the journal is on).
pub fn record(event: &Event) {
    let line = match serde_json::to_string(event) {
        Ok(l) => l,
        Err(_) => return,
    };
    let _ = std::fs::create_dir_all(dir());
    let file = std::fs::OpenOptions::new().create(true).append(true).open(events_path());
    if let Ok(mut f) = file {
        let _ = writeln!(f, "{line}");
    }
}

/// The events between two times.
pub fn events(from: i64, to: i64) -> Vec<Event> {
    let text = std::fs::read_to_string(events_path()).unwrap_or_default();
    text.lines().filter_map(|l| serde_json::from_str::<Event>(l).ok()).filter(|e| (from..to).contains(&e.time())).collect()
}

/// A terminal command, as the shell hook wrote it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub start: i64,
    pub end: i64,
    pub status: i32,
    pub dir: PathBuf,
    pub line: String,
}

/// `start \t end \t status \t dir \t command` lines.
pub fn parse_commands(text: &str) -> Vec<Command> {
    text.lines()
        .filter_map(|l| {
            let mut f = l.splitn(5, '\t');
            let start = f.next()?.trim().parse().ok()?;
            let end = f.next()?.trim().parse().unwrap_or(start);
            let status = f.next()?.trim().parse().unwrap_or(0);
            let dir = PathBuf::from(f.next()?);
            let line = f.next()?.trim().to_string();
            (!line.is_empty()).then_some(Command { start, end, status, dir, line: mask_secrets(&line) })
        })
        .collect()
}

pub fn commands(from: i64, to: i64) -> Vec<Command> {
    let text = std::fs::read_to_string(commands_path()).unwrap_or_default();
    parse_commands(&text).into_iter().filter(|c| (from..to).contains(&c.start)).collect()
}

/// Forget what is older than `days` (the record never grows for ever).
pub fn prune(days: i64) {
    let cutoff = now() - days * 86_400;
    if let Ok(text) = std::fs::read_to_string(events_path()) {
        let kept: Vec<&str> =
            text.lines().filter(|l| serde_json::from_str::<Event>(l).map(|e| e.time() >= cutoff).unwrap_or(false)).collect();
        if kept.len() < text.lines().count() {
            let _ = std::fs::write(events_path(), kept.iter().map(|l| format!("{l}\n")).collect::<String>());
        }
    }
    // The shell writes commands as typed: secrets are masked here too, so
    // they don't stay on disk past the next start.
    if let Ok(text) = std::fs::read_to_string(commands_path()) {
        let kept: Vec<String> = text
            .lines()
            .filter(|l| l.split('\t').next().and_then(|t| t.parse::<i64>().ok()).is_some_and(|t| t >= cutoff))
            .map(|l| match l.rsplit_once('\t') {
                Some((head, cmd)) => format!("{head}\t{}", mask_secrets(cmd)),
                None => l.to_string(),
            })
            .collect();
        let new: String = kept.iter().map(|l| format!("{l}\n")).collect();
        if new != text {
            let _ = std::fs::write(commands_path(), new);
        }
    }
}

/// Create the record's files readable by you only.
pub fn private_files() {
    let _ = std::fs::create_dir_all(dir());
    for p in [events_path(), commands_path()] {
        if !p.exists() {
            let _ = std::fs::write(&p, "");
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600));
        }
    }
}

/// Hide what looks like a secret in a command line: values of
/// `--password`/`--token`/`--secret…` options, of `*_KEY=`, `*TOKEN=`,
/// `*SECRET*=`, `*PASSWORD=` assignments, `Authorization:` headers and
/// passwords in URLs.
pub fn mask_secrets(line: &str) -> String {
    let words: Vec<&str> = line.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let secret_name = |n: &str| {
        let n = n.to_ascii_lowercase();
        [
            "password",
            "passwd",
            "secret",
            "token",
            "apikey",
            "api-key",
            "api_key",
            "access-key",
            "access_key",
            "private-key",
            "auth",
        ]
        .iter()
        .any(|k| n.contains(k))
            || n.ends_with("_key")
            || n.ends_with("-key")
    };
    let mut hide_next = false;
    let mut after_auth = false;
    for w in words {
        // After `Authorization:`: a scheme (Bearer, Basic, token) stays and
        // its value goes; anything else goes.
        if after_auth && !w.is_empty() {
            after_auth = false;
            let scheme = w.trim_start_matches(['\'', '"']).to_ascii_lowercase();
            if ["bearer", "basic", "token"].contains(&scheme.as_str()) {
                out.push(w.to_string());
                hide_next = true;
                continue;
            }
            let quote = if w.ends_with(['\'', '"']) { &w[w.len() - 1..] } else { "" };
            out.push(format!("****{quote}"));
            continue;
        }
        if hide_next && !w.is_empty() {
            let quote = if w.ends_with(['\'', '"']) { &w[w.len() - 1..] } else { "" };
            out.push(format!("****{quote}"));
            hide_next = false;
            continue;
        }
        // --token=abc, TOKEN=abc
        if let Some((name, value)) = w.split_once('=') {
            if !value.is_empty() && secret_name(name.trim_start_matches('-')) {
                out.push(format!("{name}=****"));
                continue;
            }
        }
        // --password abc
        if w.starts_with('-') && secret_name(w.trim_start_matches('-')) && !w.contains('=') {
            hide_next = true;
            out.push(w.to_string());
            continue;
        }
        // Authorization: Bearer abc / -H 'Authorization: …'
        if w.trim_start_matches(['\'', '"']).to_ascii_lowercase().starts_with("authorization:") {
            out.push(w.to_string());
            after_auth = true;
            continue;
        }
        // https://user:pass@host
        if let Some(i) = w.find("://") {
            if let Some(at) = w[i + 3..].find('@') {
                let cred = &w[i + 3..i + 3 + at];
                if let Some((user, _)) = cred.split_once(':') {
                    out.push(format!("{}{user}:****{}", &w[..i + 3], &w[i + 3 + at..]));
                    continue;
                }
            }
        }
        out.push(w.to_string());
    }
    out.join(" ")
}

// ---- shell integration -------------------------------------------------

const ZSH_HOOK: &str = r#"
# Qell: note each command (folder, exit status, duration) for the daily
# log in QELL_COMMAND_LOG. A command starting with a space isn't kept.
zmodload zsh/datetime 2>/dev/null
__qell_preexec() { __qell_cmd="$1"; __qell_start=$EPOCHSECONDS; __qell_dir="$PWD"; }
__qell_precmd() {
  local s=$?
  [[ -z "$__qell_cmd" || -z "$QELL_COMMAND_LOG" || "$__qell_cmd" == " "* ]] && { __qell_cmd=""; return; }
  local c="${__qell_cmd//$'\t'/ }"; c="${c//$'\n'/ ; }"
  print -r -- "${__qell_start}"$'\t'"${EPOCHSECONDS}"$'\t'"${s}"$'\t'"${__qell_dir}"$'\t'"${c}" >> "$QELL_COMMAND_LOG" 2>/dev/null
  __qell_cmd=""
}
autoload -Uz add-zsh-hook
add-zsh-hook preexec __qell_preexec
add-zsh-hook precmd __qell_precmd
"#;

const BASH_HOOK: &str = r#"
# Qell: note each command (folder, exit status) for the daily log in
# QELL_COMMAND_LOG. A command starting with a space isn't kept.
__qell_last=""
__qell_prompt() {
  local s=$? n c now
  read -r n c <<< "$(HISTTIMEFORMAT= builtin history 1)"
  if [[ -n "$QELL_COMMAND_LOG" && -n "$n" && "$n" != "$__qell_last" && -n "$__qell_last" ]]; then
    now=$(date +%s)
    c="${c//$'\t'/ }"
    printf '%s\t%s\t%s\t%s\t%s\n' "$now" "$now" "$s" "$PWD" "$c" >> "$QELL_COMMAND_LOG" 2>/dev/null
  fi
  __qell_last="$n"
  return $s
}
PROMPT_COMMAND="__qell_prompt${PROMPT_COMMAND:+; $PROMPT_COMMAND}"
"#;

/// The startup files that wrap the user's own (zsh reads them from
/// `ZDOTDIR`): each loads the user's file, the `.zshrc` then adds the hook.
fn zsh_files() -> [(&'static str, String); 4] {
    let pass = |file: &str| {
        format!(
            "__qell_wrap=\"$ZDOTDIR\"\nZDOTDIR=\"${{QELL_USER_ZDOTDIR:-$HOME}}\"\n[[ -f \"$ZDOTDIR/{file}\" ]] && source \"$ZDOTDIR/{file}\"\nZDOTDIR=\"$__qell_wrap\"\n"
        )
    };
    [
        (".zshenv", pass(".zshenv")),
        (".zprofile", pass(".zprofile")),
        (
            ".zshrc",
            format!(
                "ZDOTDIR=\"${{QELL_USER_ZDOTDIR:-$HOME}}\"\n[[ -f \"$ZDOTDIR/.zshrc\" ]] && source \"$ZDOTDIR/.zshrc\"\n{ZSH_HOOK}"
            ),
        ),
        // Read after .zshrc for login shells; ZDOTDIR is the user's again.
        (".zlogin", pass(".zlogin")),
    ]
}

/// A program and its arguments.
pub type Argv = (String, Vec<String>);

/// How to start a shell with the hook: the program, its arguments and the
/// environment to add. `shell` is the configured command line (empty:
/// `$SHELL`). Other shells start as they are (`None`).
pub fn integrate(shell: &str) -> Option<(Argv, Vec<(String, String)>)> {
    let mut words = shell.split_whitespace().map(String::from);
    let (program, mut args) = match words.next() {
        Some(p) => (p, words.collect::<Vec<_>>()),
        None => (std::env::var("SHELL").ok().filter(|s| !s.is_empty())?, Vec::new()),
    };
    let name = Path::new(&program).file_name()?.to_string_lossy().into_owned();
    let base = dir().join("shell");
    let mut env = vec![("QELL_COMMAND_LOG".to_string(), commands_path().to_string_lossy().into_owned())];
    private_files();
    match name.as_str() {
        "zsh" => {
            let zdir = base.join("zsh");
            std::fs::create_dir_all(&zdir).ok()?;
            for (file, text) in zsh_files() {
                std::fs::write(zdir.join(file), text).ok()?;
            }
            let user =
                std::env::var("ZDOTDIR").unwrap_or_else(|_| dirs::home_dir().unwrap_or_default().to_string_lossy().into_owned());
            env.push(("QELL_USER_ZDOTDIR".into(), user));
            env.push(("ZDOTDIR".into(), zdir.to_string_lossy().into_owned()));
            Some(((program, args), env))
        }
        // bash ignores --rcfile in a login shell: those start as they are.
        "bash" if !args.iter().any(|a| a == "-l" || a == "--login") => {
            std::fs::create_dir_all(&base).ok()?;
            let rc = base.join("bashrc");
            let text = format!("[[ -f ~/.bashrc ]] && source ~/.bashrc\n{BASH_HOOK}");
            std::fs::write(&rc, text).ok()?;
            args.insert(0, "--rcfile".into());
            args.insert(1, rc.to_string_lossy().into_owned());
            Some(((program, args), env))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_are_masked() {
        assert_eq!(mask_secrets("mysql -u root --password hunter2 db"), "mysql -u root --password **** db");
        assert_eq!(mask_secrets("GITHUB_TOKEN=abc gh pr list"), "GITHUB_TOKEN=**** gh pr list");
        assert_eq!(mask_secrets("aws configure set aws_secret_access_key=xyz"), "aws configure set aws_secret_access_key=****");
        assert_eq!(
            mask_secrets("curl -H 'Authorization: Bearer abc.def' https://x"),
            "curl -H 'Authorization: Bearer ****' https://x"
        );
        assert_eq!(mask_secrets("git clone https://me:pw@github.com/x"), "git clone https://me:****@github.com/x");
        assert_eq!(mask_secrets("terraform plan -var-file=testing.tfvars"), "terraform plan -var-file=testing.tfvars");
        assert_eq!(mask_secrets("kubectl get pods -n api"), "kubectl get pods -n api");
    }

    #[test]
    fn command_lines() {
        let c = parse_commands("100\t112\t1\t/src/infra\tterraform plan\nbad line\n200\t200\t0\t/x\t\n");
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].end - c[0].start, c[0].status, c[0].line.as_str()), (12, 1, "terraform plan"));
    }

    #[test]
    fn events_round_trip() {
        let e = Event::Git { t: 5, repo: "/r".into(), what: "Switched to main".into() };
        let line = serde_json::to_string(&e).unwrap();
        assert!(line.contains("\"kind\":\"git\""));
        assert_eq!(serde_json::from_str::<Event>(&line).unwrap(), e);
    }
}
