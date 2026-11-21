//! Turning `kubectl get -o json` into the columns k9s shows.
//!
//! Each kind the view knows gets its own columns; anything else falls back
//! to name and age, which is enough to browse a cluster's own resources.

use serde_json::Value;

/// How a row is doing, for its colour. Nothing here is a judgement about
/// the cluster: it is read straight from phases and conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    Ok,
    Warn,
    Bad,
    /// Nothing to say (a config map has no health).
    Plain,
}

/// One line of the table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub cells: Vec<String>,
    /// `metadata.name`, for drilling in and for the status line.
    pub name: String,
    pub namespace: Option<String>,
    pub health: Health,
    /// The object itself, for what a drill-down needs (selectors,
    /// containers). Kept as text to stay cheap to clone.
    pub object: Value,
}

/// A whole listing, ready to draw.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Table {
    pub columns: Vec<String>,
    pub rows: Vec<Row>,
}

impl Table {
    /// The width each column needs, in characters.
    pub fn widths(&self) -> Vec<usize> {
        let mut w: Vec<usize> = self.columns.iter().map(|c| c.chars().count()).collect();
        for row in &self.rows {
            for (i, cell) in row.cells.iter().enumerate() {
                if let Some(x) = w.get_mut(i) {
                    *x = (*x).max(cell.chars().count());
                }
            }
        }
        w
    }
}

/// What one pod or node is using now, from `kubectl top`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    /// CPU in thousandths of a core, as `250m`.
    pub cpu_milli: u64,
    pub mem_bytes: u64,
}

/// What everything is using, by name (`namespace/name` when the listing
/// spans namespaces, which is how `kubectl top -A` prints it).
pub type Metrics = std::collections::HashMap<String, Usage>;

/// `250m`, `1`, `1500000n` as thousandths of a core.
pub fn cpu_quantity(text: &str) -> Option<u64> {
    let text = text.trim();
    let (number, scale) = match text.strip_suffix(['m', 'u', 'n']) {
        Some(head) => (head, text.as_bytes()[text.len() - 1]),
        None => (text, b'c'),
    };
    let value: f64 = number.parse().ok()?;
    Some(match scale {
        b'n' => (value / 1_000_000.0) as u64,
        b'u' => (value / 1_000.0) as u64,
        b'm' => value as u64,
        _ => (value * 1_000.0) as u64,
    })
}

/// `128Mi`, `1Gi`, `500M`, `1024` as bytes. Kubernetes writes both the
/// binary suffixes (Ki, Mi, Gi) and the decimal ones (K, M, G).
pub fn mem_quantity(text: &str) -> Option<u64> {
    let text = text.trim();
    let units: [(&str, u64); 10] = [
        ("Ki", 1 << 10),
        ("Mi", 1 << 20),
        ("Gi", 1 << 30),
        ("Ti", 1u64 << 40),
        ("Pi", 1u64 << 50),
        ("K", 1_000),
        ("M", 1_000_000),
        ("G", 1_000_000_000),
        ("T", 1_000_000_000_000),
        ("k", 1_000),
    ];
    for (suffix, scale) in units {
        if let Some(head) = text.strip_suffix(suffix) {
            let value: f64 = head.trim().parse().ok()?;
            return Some((value * scale as f64) as u64);
        }
    }
    text.parse::<f64>().ok().map(|v| v as u64)
}

/// A `kubectl top pods|nodes --no-headers` table. With `-A` the first
/// column is the namespace, and the key becomes `namespace/name`.
pub fn parse_usage(text: &str, all_namespaces: bool) -> Metrics {
    let mut out = Metrics::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let (key, rest) = match (all_namespaces, f.len()) {
            (true, n) if n >= 4 => (format!("{}/{}", f[0], f[1]), &f[2..]),
            (false, n) if n >= 3 => (f[0].to_string(), &f[1..]),
            _ => continue,
        };
        // Nodes also print percentages: NAME CPU CPU% MEM MEM%.
        let (cpu, mem) = if rest.len() >= 4 { (rest[0], rest[2]) } else { (rest[0], rest[1]) };
        if let (Some(cpu_milli), Some(mem_bytes)) = (cpu_quantity(cpu), mem_quantity(mem)) {
            out.insert(key, Usage { cpu_milli, mem_bytes });
        }
    }
    out
}

/// `250m` / `1.2` for CPU, the way kubectl writes it back.
fn cpu_cell(milli: u64) -> String {
    if milli >= 10_000 {
        format!("{:.1}", milli as f64 / 1000.0)
    } else {
        format!("{milli}m")
    }
}

/// Bytes as mebibytes, which is the unit people read pods in.
fn mem_cell(bytes: u64) -> String {
    let mi = bytes as f64 / (1 << 20) as f64;
    if mi >= 1_024.0 {
        format!("{:.1}Gi", mi / 1024.0)
    } else {
        format!("{mi:.0}Mi")
    }
}

/// `used` against `of`, as a percentage, or `-` when nothing was asked
/// for (no request, no limit).
fn percent(used: u64, of: u64) -> String {
    if of == 0 {
        return "-".to_string();
    }
    format!("{}", (used as f64 / of as f64 * 100.0).round() as u64)
}

/// What a pod's containers ask for and are capped at, added up.
fn pod_resources(item: &Value) -> (u64, u64, u64, u64) {
    let (mut cpu_r, mut mem_r, mut cpu_l, mut mem_l) = (0, 0, 0, 0);
    for c in array(item, &["spec", "containers"]) {
        let get = |kind: &str, key: &str| {
            let text = s(c, &["resources", kind, key]);
            (!text.is_empty()).then_some(text)
        };
        cpu_r += get("requests", "cpu").and_then(|t| cpu_quantity(&t)).unwrap_or(0);
        mem_r += get("requests", "memory").and_then(|t| mem_quantity(&t)).unwrap_or(0);
        cpu_l += get("limits", "cpu").and_then(|t| cpu_quantity(&t)).unwrap_or(0);
        mem_l += get("limits", "memory").and_then(|t| mem_quantity(&t)).unwrap_or(0);
    }
    (cpu_r, mem_r, cpu_l, mem_l)
}

/// The usage cells of whichever kind has them.
fn metric_cells(kind: &str, item: &Value, usage: Option<&Usage>) -> Vec<String> {
    match kind {
        "nodes" => node_metric_cells(item, usage),
        _ => pod_metric_cells(item, usage),
    }
}

/// A node's usage, against what it has to give (`kubectl top node`).
fn node_metric_cells(item: &Value, usage: Option<&Usage>) -> Vec<String> {
    let Some(u) = usage else {
        return vec!["n/a".into(), "n/a".into()];
    };
    let cpu = cpu_quantity(&s(item, &["status", "allocatable", "cpu"])).unwrap_or(0);
    let mem = mem_quantity(&s(item, &["status", "allocatable", "memory"])).unwrap_or(0);
    // A node that doesn't say what it has gets the figure on its own,
    // rather than a percentage of nothing.
    let against = |cell: String, used: u64, of: u64| match of {
        0 => cell,
        _ => format!("{cell} ({}%)", percent(used, of)),
    };
    vec![against(cpu_cell(u.cpu_milli), u.cpu_milli, cpu), against(mem_cell(u.mem_bytes), u.mem_bytes, mem)]
}

/// The metric cells for a pod: what it is using, and how that compares
/// with what it asked for and what it is capped at, as k9s shows them.
fn pod_metric_cells(item: &Value, usage: Option<&Usage>) -> Vec<String> {
    let Some(u) = usage else {
        return vec!["n/a".into(), "n/a".into(), "-".into(), "-".into(), "-".into(), "-".into()];
    };
    let (cpu_r, mem_r, cpu_l, mem_l) = pod_resources(item);
    vec![
        cpu_cell(u.cpu_milli),
        mem_cell(u.mem_bytes),
        percent(u.cpu_milli, cpu_r),
        percent(u.mem_bytes, mem_r),
        percent(u.cpu_milli, cpu_l),
        percent(u.mem_bytes, mem_l),
    ]
}

/// `.a.b.c` through a JSON object, as a string.
fn s(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for key in path {
        cur = &cur[*key];
    }
    match cur {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn n(v: &Value, path: &[&str]) -> i64 {
    let mut cur = v;
    for key in path {
        cur = &cur[*key];
    }
    cur.as_i64().unwrap_or(0)
}

fn array<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    let mut cur = v;
    for key in path {
        cur = &cur[*key];
    }
    cur.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// A dash where k8s shows nothing, as kubectl does.
fn or_none(text: String) -> String {
    if text.is_empty() {
        "<none>".to_string()
    } else {
        text
    }
}

/// Seconds since `timestamp` (RFC 3339, as the API server writes it),
/// given the time now in seconds since the epoch.
pub fn seconds_since(timestamp: &str, now: i64) -> Option<i64> {
    Some(now - epoch_seconds(timestamp)?)
}

/// `2026-11-21T07:04:05Z` as seconds since the epoch. Only the shape the
/// API server uses is accepted; anything else is no date at all.
pub fn epoch_seconds(timestamp: &str) -> Option<i64> {
    let b = timestamp.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' || b[13] != b':' || b[16] != b':' {
        return None;
    }
    let num = |a: usize, z: usize| timestamp.get(a..z)?.parse::<i64>().ok();
    let (y, m, d) = (num(0, 4)?, num(5, 7)?, num(8, 10)?);
    let (hh, mm, ss) = (num(11, 13)?, num(14, 16)?, num(17, 19)?);
    // Days from the civil calendar (Howard Hinnant's algorithm), which
    // beats pulling in a date library for one field.
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hh * 3_600 + mm * 60 + ss)
}

/// An age the way kubectl prints it: `12s`, `5m`, `3h20m`, `6d4h`, `21d`.
pub fn age(seconds: i64) -> String {
    let s = seconds.max(0);
    let (m, h, d) = (s / 60, s / 3_600, s / 86_400);
    if d >= 8 {
        format!("{d}d")
    } else if d >= 1 {
        let hours = h - d * 24;
        if hours == 0 {
            format!("{d}d")
        } else {
            format!("{d}d{hours}h")
        }
    } else if h >= 1 {
        let mins = m - h * 60;
        if mins == 0 {
            format!("{h}h")
        } else {
            format!("{h}h{mins}m")
        }
    } else if m >= 1 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

/// The age cell of an object, from its creation timestamp.
fn age_of(item: &Value, now: i64) -> String {
    match seconds_since(&s(item, &["metadata", "creationTimestamp"]), now) {
        Some(secs) => age(secs),
        None => "<unknown>".to_string(),
    }
}

/// Builds one line's cells and its health from an object of that kind.
type RowOf = fn(&Value, i64) -> (Vec<String>, Health);

/// Build the table for `kind` from the items of a `kubectl get -o json`.
/// `show_namespace` is for listings that span namespaces.
pub fn build(kind: &str, items: &[Value], now: i64, show_namespace: bool, metrics: &Metrics) -> Table {
    let mut columns: Vec<String> = Vec::new();
    if show_namespace {
        columns.push("NAMESPACE".into());
    }
    let (rest, row_of): (Vec<&str>, RowOf) = match kind {
        "pods" => (vec!["NAME", "READY", "STATUS", "RESTARTS", "AGE", "IP", "NODE"], pod_row),
        "deployments" => (vec!["NAME", "READY", "UP-TO-DATE", "AVAILABLE", "AGE"], deployment_row),
        "statefulsets" => (vec!["NAME", "READY", "AGE"], statefulset_row),
        "daemonsets" => (vec!["NAME", "DESIRED", "CURRENT", "READY", "AGE"], daemonset_row),
        "services" => (vec!["NAME", "TYPE", "CLUSTER-IP", "EXTERNAL-IP", "PORTS", "AGE"], service_row),
        "nodes" => (vec!["NAME", "STATUS", "ROLES", "AGE", "VERSION"], node_row),
        "namespaces" => (vec!["NAME", "STATUS", "AGE"], namespace_row),
        "events" => (vec!["LAST SEEN", "TYPE", "REASON", "OBJECT", "MESSAGE"], event_row),
        "configmaps" | "secrets" => (vec!["NAME", "TYPE", "DATA", "AGE"], data_row),
        _ => (vec!["NAME", "AGE"], plain_row),
    };
    // What each one is using now, when metrics-server is answering.
    let metric_columns: &[&str] = match kind {
        _ if metrics.is_empty() => &[],
        "pods" => &["CPU", "MEM", "%CPU/R", "%MEM/R", "%CPU/L", "%MEM/L"],
        "nodes" => &["CPU", "MEM"],
        _ => &[],
    };
    columns.extend(rest.iter().map(|c| c.to_string()));
    // Usage goes where k9s puts it: after the state, before the age.
    let metrics_at = columns.iter().position(|c| c == "AGE").unwrap_or(columns.len());
    for (i, name) in metric_columns.iter().enumerate() {
        columns.insert(metrics_at + i, name.to_string());
    }
    let mut rows: Vec<Row> = items
        .iter()
        .map(|item| {
            let (mut cells, health) = row_of(item, now);
            let namespace = Some(s(item, &["metadata", "namespace"])).filter(|n| !n.is_empty());
            if !metric_columns.is_empty() {
                let name = s(item, &["metadata", "name"]);
                let key = match &namespace {
                    Some(ns) => format!("{ns}/{name}"),
                    None => name.clone(),
                };
                let usage = metrics.get(&key).or_else(|| metrics.get(&name));
                let at = metrics_at - usize::from(show_namespace);
                for (i, cell) in metric_cells(kind, item, usage).into_iter().enumerate() {
                    cells.insert(at + i, cell);
                }
            }
            if show_namespace {
                cells.insert(0, namespace.clone().unwrap_or_default());
            }
            Row { cells, name: s(item, &["metadata", "name"]), namespace, health, object: item.clone() }
        })
        .collect();
    // Events read best newest first; everything else by name, as kubectl.
    if kind == "events" {
        rows.reverse();
    }
    Table { columns, rows }
}

fn plain_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    (vec![s(item, &["metadata", "name"]), age_of(item, now)], Health::Plain)
}

fn namespace_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let phase = s(item, &["status", "phase"]);
    let health = if phase == "Active" { Health::Ok } else { Health::Warn };
    (vec![s(item, &["metadata", "name"]), phase, age_of(item, now)], health)
}

fn pod_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let statuses = array(item, &["status", "containerStatuses"]);
    let ready = statuses.iter().filter(|c| c["ready"].as_bool().unwrap_or(false)).count();
    let restarts: i64 = statuses.iter().map(|c| c["restartCount"].as_i64().unwrap_or(0)).sum();
    let total = array(item, &["spec", "containers"]).len().max(statuses.len());
    let status = pod_status(item, statuses);
    let health = match status.as_str() {
        "Running" | "Completed" | "Succeeded" => Health::Ok,
        "Terminating" | "Pending" | "ContainerCreating" | "PodInitializing" => Health::Warn,
        // `Init:1/2` is on its way; `Init:ImagePullBackOff` is stuck,
        // and falls through to the not-ready arm below.
        _ if status.starts_with("Init:") && status.contains('/') => Health::Warn,
        _ if ready < total => Health::Bad,
        _ => Health::Ok,
    };
    let cells = vec![
        s(item, &["metadata", "name"]),
        format!("{ready}/{total}"),
        status,
        restarts.to_string(),
        age_of(item, now),
        or_none(s(item, &["status", "podIP"])),
        or_none(s(item, &["spec", "nodeName"])),
    ];
    (cells, health)
}

/// What kubectl shows in a pod's STATUS column: the container that is
/// unhappy, if any, otherwise the phase.
fn pod_status(item: &Value, statuses: &[Value]) -> String {
    if !s(item, &["metadata", "deletionTimestamp"]).is_empty() {
        return "Terminating".to_string();
    }
    // Still starting up: which init container it is on, as kubectl says.
    let inits = array(item, &["status", "initContainerStatuses"]);
    let wanted = array(item, &["spec", "initContainers"]).len();
    if wanted > 0 {
        let done = inits.iter().filter(|c| s(c, &["state", "terminated", "reason"]) == "Completed").count();
        if done < wanted {
            let stuck = inits.iter().find_map(|c| {
                let reason = s(c, &["state", "waiting", "reason"]);
                let failed = s(c, &["state", "terminated", "reason"]);
                [reason, failed].into_iter().find(|r| !r.is_empty() && r != "PodInitializing" && r != "Completed")
            });
            return match stuck {
                Some(reason) => format!("Init:{reason}"),
                None => format!("Init:{done}/{wanted}"),
            };
        }
    }
    for c in inits.iter().chain(statuses) {
        let waiting = s(c, &["state", "waiting", "reason"]);
        let last = s(c, &["lastState", "terminated", "reason"]);
        // A container that keeps being killed for memory says so, rather
        // than only `CrashLoopBackOff`, which is how it comes back.
        if waiting == "CrashLoopBackOff" && last == "OOMKilled" {
            return "OOMKilled".to_string();
        }
        if !waiting.is_empty() {
            return waiting;
        }
        let terminated = s(c, &["state", "terminated", "reason"]);
        if !terminated.is_empty() && terminated != "Completed" {
            return terminated;
        }
    }
    let reason = s(item, &["status", "reason"]);
    if !reason.is_empty() {
        return reason;
    }
    s(item, &["status", "phase"])
}

fn deployment_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let want = n(item, &["spec", "replicas"]);
    let ready = n(item, &["status", "readyReplicas"]);
    let health = if ready >= want && want > 0 {
        Health::Ok
    } else if ready == 0 && want > 0 {
        Health::Bad
    } else {
        Health::Warn
    };
    let cells = vec![
        s(item, &["metadata", "name"]),
        format!("{ready}/{want}"),
        n(item, &["status", "updatedReplicas"]).to_string(),
        n(item, &["status", "availableReplicas"]).to_string(),
        age_of(item, now),
    ];
    (cells, health)
}

fn statefulset_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let want = n(item, &["spec", "replicas"]);
    let ready = n(item, &["status", "readyReplicas"]);
    let health = if ready >= want && want > 0 { Health::Ok } else { Health::Warn };
    (vec![s(item, &["metadata", "name"]), format!("{ready}/{want}"), age_of(item, now)], health)
}

fn daemonset_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let want = n(item, &["status", "desiredNumberScheduled"]);
    let current = n(item, &["status", "currentNumberScheduled"]);
    let ready = n(item, &["status", "numberReady"]);
    let health = if ready >= want { Health::Ok } else { Health::Warn };
    let cells = vec![s(item, &["metadata", "name"]), want.to_string(), current.to_string(), ready.to_string(), age_of(item, now)];
    (cells, health)
}

fn service_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let ports: Vec<String> = array(item, &["spec", "ports"])
        .iter()
        .map(|p| {
            let proto = s(p, &["protocol"]);
            let node = n(p, &["nodePort"]);
            let port = n(p, &["port"]);
            if node > 0 {
                format!("{port}:{node}/{proto}")
            } else {
                format!("{port}/{proto}")
            }
        })
        .collect();
    let external: Vec<String> = array(item, &["status", "loadBalancer", "ingress"])
        .iter()
        .map(|i| {
            let ip = s(i, &["ip"]);
            if ip.is_empty() {
                s(i, &["hostname"])
            } else {
                ip
            }
        })
        .chain(array(item, &["spec", "externalIPs"]).iter().map(|v| v.as_str().unwrap_or("").to_string()))
        .filter(|v| !v.is_empty())
        .collect();
    let kind = s(item, &["spec", "type"]);
    // A load balancer with no address yet is still coming up.
    let health = if kind == "LoadBalancer" && external.is_empty() { Health::Warn } else { Health::Plain };
    let cells = vec![
        s(item, &["metadata", "name"]),
        kind,
        or_none(s(item, &["spec", "clusterIP"])),
        or_none(external.join(",")),
        or_none(ports.join(",")),
        age_of(item, now),
    ];
    (cells, health)
}

fn node_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let ready = array(item, &["status", "conditions"]).iter().find(|c| s(c, &["type"]) == "Ready").map(|c| s(c, &["status"]));
    let unschedulable = item["spec"]["unschedulable"].as_bool().unwrap_or(false);
    let (status, health) = match ready.as_deref() {
        Some("True") if unschedulable => ("Ready,SchedulingDisabled".to_string(), Health::Warn),
        Some("True") => ("Ready".to_string(), Health::Ok),
        Some(_) => ("NotReady".to_string(), Health::Bad),
        None => ("Unknown".to_string(), Health::Warn),
    };
    let roles: Vec<String> = item["metadata"]["labels"]
        .as_object()
        .map(|l| {
            l.keys()
                .filter_map(|k| k.strip_prefix("node-role.kubernetes.io/").map(str::to_string))
                .filter(|r| !r.is_empty())
                .collect()
        })
        .unwrap_or_default();
    let cells = vec![
        s(item, &["metadata", "name"]),
        status,
        or_none(roles.join(",")),
        age_of(item, now),
        s(item, &["status", "nodeInfo", "kubeletVersion"]),
    ];
    (cells, health)
}

fn event_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let when = [
        s(item, &["lastTimestamp"]),
        s(item, &["eventTime"]),
        s(item, &["deprecatedLastTimestamp"]),
        s(item, &["metadata", "creationTimestamp"]),
    ]
    .into_iter()
    .find_map(|t| seconds_since(&t, now))
    .map(age)
    .unwrap_or_else(|| "<unknown>".to_string());
    let kind = s(item, &["type"]);
    let health = if kind == "Warning" { Health::Bad } else { Health::Plain };
    let object = format!("{}/{}", s(item, &["involvedObject", "kind"]).to_lowercase(), s(item, &["involvedObject", "name"]));
    let cells = vec![when, kind, s(item, &["reason"]), object, s(item, &["message"]).replace('\n', " ")];
    (cells, health)
}

fn data_row(item: &Value, now: i64) -> (Vec<String>, Health) {
    let count = item["data"].as_object().map(|d| d.len()).unwrap_or(0);
    let kind = s(item, &["type"]);
    let cells = vec![
        s(item, &["metadata", "name"]),
        if kind.is_empty() { "-".to_string() } else { kind },
        count.to_string(),
        age_of(item, now),
    ];
    (cells, Health::Plain)
}

/// The containers of one pod, as their own table (what Enter on a pod
/// shows).
pub fn containers(pod: &Value, now: i64) -> Table {
    let statuses = array(pod, &["status", "containerStatuses"]);
    let columns = ["NAME", "IMAGE", "READY", "STATE", "RESTARTS", "AGE"].iter().map(|c| c.to_string()).collect();
    let rows = array(pod, &["spec", "containers"])
        .iter()
        .map(|c| {
            let name = s(c, &["name"]);
            let status = statuses.iter().find(|s2| s(s2, &["name"]) == name);
            let ready = status.map(|s2| s2["ready"].as_bool().unwrap_or(false)).unwrap_or(false);
            let restarts = status.map(|s2| n(s2, &["restartCount"])).unwrap_or(0);
            let state = status.map(container_state).unwrap_or_else(|| "Waiting".to_string());
            let started = status
                .map(|s2| s(s2, &["state", "running", "startedAt"]))
                .and_then(|t| seconds_since(&t, now))
                .map(age)
                .unwrap_or_else(|| "-".to_string());
            let health = match (&state[..], ready) {
                ("Running", true) => Health::Ok,
                ("Completed", _) => Health::Ok,
                ("Running", false) => Health::Warn,
                _ => Health::Bad,
            };
            let cells = vec![name.clone(), s(c, &["image"]), ready.to_string(), state, restarts.to_string(), started];
            Row { cells, name, namespace: None, health, object: c.clone() }
        })
        .collect();
    Table { columns, rows }
}

fn container_state(status: &Value) -> String {
    for (key, field) in [("running", "Running"), ("terminated", "reason"), ("waiting", "reason")] {
        let state = &status["state"][key];
        if state.is_object() {
            return match field {
                "Running" => "Running".to_string(),
                _ => or_none(s(state, &["reason"])),
            };
        }
    }
    "Unknown".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn ages_read_like_kubectl() {
        assert_eq!(age(5), "5s");
        assert_eq!(age(90), "1m");
        assert_eq!(age(3_600), "1h");
        assert_eq!(age(3_600 + 20 * 60), "1h20m");
        assert_eq!(age(86_400 * 6 + 3_600 * 4), "6d4h");
        assert_eq!(age(86_400 * 21), "21d");
    }

    #[test]
    fn timestamps_become_seconds() {
        assert_eq!(epoch_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_seconds("2026-11-21T07:04:05Z"), Some(1_795_244_645));
        assert_eq!(epoch_seconds("not a date"), None);
        assert_eq!(epoch_seconds(""), None);
    }

    fn pod(name: &str, phase: &str, waiting: Option<&str>, ready: bool) -> Value {
        json!({
            "metadata": { "name": name, "namespace": "web", "creationTimestamp": "2026-11-21T07:04:05Z" },
            "spec": { "containers": [{ "name": "app", "image": "nginx:1.27" }], "nodeName": "ip-10-0-1-5" },
            "status": {
                "phase": phase,
                "podIP": "10.0.1.9",
                "containerStatuses": [{
                    "name": "app",
                    "ready": ready,
                    "restartCount": 3,
                    "state": match waiting {
                        Some(r) => json!({ "waiting": { "reason": r } }),
                        None => json!({ "running": { "startedAt": "2026-11-21T07:04:05Z" } }),
                    },
                }],
            },
        })
    }

    #[test]
    fn pods_get_the_columns_k9s_shows() {
        let now = epoch_seconds("2026-11-21T09:04:05Z").unwrap();
        let t = build("pods", &[pod("api-0", "Running", None, true)], now, false, &Metrics::new());
        assert_eq!(t.columns, ["NAME", "READY", "STATUS", "RESTARTS", "AGE", "IP", "NODE"]);
        assert_eq!(t.rows[0].cells, ["api-0", "1/1", "Running", "3", "2h", "10.0.1.9", "ip-10-0-1-5"]);
        assert_eq!(t.rows[0].health, Health::Ok);
        assert_eq!(t.rows[0].namespace.as_deref(), Some("web"));
    }

    #[test]
    fn a_waiting_container_names_the_trouble() {
        let t = build("pods", &[pod("api-1", "Pending", Some("CrashLoopBackOff"), false)], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[2], "CrashLoopBackOff");
        assert_eq!(t.rows[0].health, Health::Bad);
    }

    #[test]
    fn a_pod_still_starting_says_which_init_container_it_is_on() {
        let mut p = pod("api-0", "Pending", Some("PodInitializing"), false);
        p["spec"]["initContainers"] = json!([{ "name": "wait-db" }, { "name": "migrate" }]);
        p["status"]["initContainerStatuses"] = json!([
            { "name": "wait-db", "state": { "terminated": { "reason": "Completed" } } },
            { "name": "migrate", "state": { "running": {} } },
        ]);
        let t = build("pods", &[p.clone()], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[2], "Init:1/2");
        assert_eq!(t.rows[0].health, Health::Warn);
        // One that is failing names the reason instead of counting.
        p["status"]["initContainerStatuses"][1]["state"] = json!({ "waiting": { "reason": "ImagePullBackOff" } });
        let t = build("pods", &[p], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[2], "Init:ImagePullBackOff");
        assert_eq!(t.rows[0].health, Health::Bad);
    }

    #[test]
    fn a_pod_killed_for_memory_says_oomkilled() {
        let mut p = pod("api-0", "Running", Some("CrashLoopBackOff"), false);
        p["status"]["containerStatuses"][0]["lastState"] = json!({ "terminated": { "reason": "OOMKilled" } });
        let t = build("pods", &[p], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[2], "OOMKilled");
        assert_eq!(t.rows[0].health, Health::Bad);
    }

    #[test]
    fn a_pod_on_its_way_out_is_terminating() {
        let mut p = pod("api-2", "Running", None, true);
        p["metadata"]["deletionTimestamp"] = json!("2026-11-21T07:05:05Z");
        let t = build("pods", &[p], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[2], "Terminating");
        assert_eq!(t.rows[0].health, Health::Warn);
    }

    #[test]
    fn listings_across_namespaces_say_which() {
        let t = build("pods", &[pod("api-0", "Running", None, true)], NOW, true, &Metrics::new());
        assert_eq!(t.columns[0], "NAMESPACE");
        assert_eq!(t.rows[0].cells[0], "web");
    }

    #[test]
    fn deployments_count_their_replicas() {
        let item = json!({
            "metadata": { "name": "api", "creationTimestamp": "2026-11-21T07:04:05Z" },
            "spec": { "replicas": 3 },
            "status": { "readyReplicas": 1, "updatedReplicas": 3, "availableReplicas": 1 },
        });
        let t = build("deployments", &[item], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[1], "1/3");
        assert_eq!(t.rows[0].health, Health::Warn);
    }

    #[test]
    fn services_show_their_ports_and_address() {
        let item = json!({
            "metadata": { "name": "api", "creationTimestamp": "2026-11-21T07:04:05Z" },
            "spec": { "type": "LoadBalancer", "clusterIP": "172.20.1.1",
                      "ports": [{ "port": 80, "nodePort": 31234, "protocol": "TCP" }] },
            "status": { "loadBalancer": { "ingress": [{ "hostname": "a.elb.amazonaws.com" }] } },
        });
        let t = build("services", &[item], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[3], "a.elb.amazonaws.com");
        assert_eq!(t.rows[0].cells[4], "80:31234/TCP");
        assert_eq!(t.rows[0].health, Health::Plain);
    }

    #[test]
    fn nodes_read_their_condition_and_roles() {
        let item = json!({
            "metadata": { "name": "ip-10-0-1-5", "creationTimestamp": "2026-11-21T07:04:05Z",
                          "labels": { "node-role.kubernetes.io/worker": "" } },
            "spec": { "unschedulable": true },
            "status": { "conditions": [{ "type": "Ready", "status": "True" }],
                        "nodeInfo": { "kubeletVersion": "v1.31.3" } },
        });
        let t = build("nodes", &[item], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[1], "Ready,SchedulingDisabled");
        assert_eq!(t.rows[0].cells[2], "worker");
        assert_eq!(t.rows[0].cells[4], "v1.31.3");
        assert_eq!(t.rows[0].health, Health::Warn);
    }

    #[test]
    fn warnings_stand_out_among_events_and_the_newest_is_first() {
        let ev = |reason: &str, kind: &str| {
            json!({ "type": kind, "reason": reason, "message": "back-off\nrestarting",
                    "lastTimestamp": "2026-11-21T07:04:05Z",
                    "involvedObject": { "kind": "Pod", "name": "api-0" } })
        };
        let t = build("events", &[ev("Pulled", "Normal"), ev("BackOff", "Warning")], NOW, false, &Metrics::new());
        assert_eq!(t.rows[0].cells[2], "BackOff");
        assert_eq!(t.rows[0].health, Health::Bad);
        // Messages are one line, whatever the cluster sends.
        assert_eq!(t.rows[0].cells[4], "back-off restarting");
    }

    #[test]
    fn anything_else_still_lists_by_name_and_age() {
        let item = json!({ "metadata": { "name": "thing", "creationTimestamp": "2026-11-21T07:04:05Z" } });
        let t = build("widgets", &[item], NOW, false, &Metrics::new());
        assert_eq!(t.columns, ["NAME", "AGE"]);
        assert_eq!(t.rows[0].cells[0], "thing");
    }

    #[test]
    fn a_pods_containers_are_their_own_table() {
        let t = containers(&pod("api-0", "Running", None, true), NOW);
        assert_eq!(t.columns[0], "NAME");
        assert_eq!(t.rows[0].cells[0], "app");
        assert_eq!(t.rows[0].cells[1], "nginx:1.27");
        assert_eq!(t.rows[0].cells[3], "Running");
        assert_eq!(t.rows[0].health, Health::Ok);
    }

    #[test]
    fn pods_show_what_they_use_against_what_they_asked_for() {
        let mut p = pod("api-0", "Running", None, true);
        p["spec"]["containers"][0]["resources"] =
            json!({ "requests": { "cpu": "100m", "memory": "128Mi" }, "limits": { "cpu": "500m", "memory": "256Mi" } });
        let mut metrics = Metrics::new();
        metrics.insert("web/api-0".into(), Usage { cpu_milli: 50, mem_bytes: 64 * 1024 * 1024 });
        let t = build("pods", &[p], NOW, true, &metrics);
        // The usage columns sit between the state and the age, as in k9s.
        let at = t.columns.iter().position(|c| c == "CPU").expect("a CPU column");
        assert_eq!(&t.columns[at..at + 6], ["CPU", "MEM", "%CPU/R", "%MEM/R", "%CPU/L", "%MEM/L"]);
        assert_eq!(&t.rows[0].cells[at..at + 6], ["50m", "64Mi", "50", "50", "10", "25"]);
        assert_eq!(t.columns[at + 6], "AGE");
    }

    #[test]
    fn a_pod_without_metrics_or_limits_says_so_rather_than_lying() {
        let mut metrics = Metrics::new();
        metrics.insert("other".into(), Usage::default());
        let t = build("pods", &[pod("api-0", "Running", None, true)], NOW, false, &metrics);
        let at = t.columns.iter().position(|c| c == "CPU").expect("a CPU column");
        // No metrics for this pod at all.
        assert_eq!(&t.rows[0].cells[at..at + 3], ["n/a", "n/a", "-"]);
        // And with metrics but no requests, the percentage is a dash.
        metrics.insert("api-0".into(), Usage { cpu_milli: 7, mem_bytes: 1 << 20 });
        let t = build("pods", &[pod("api-0", "Running", None, true)], NOW, false, &metrics);
        assert_eq!(&t.rows[0].cells[at..at + 4], ["7m", "1Mi", "-", "-"]);
    }

    #[test]
    fn nodes_show_their_usage_against_what_they_can_give() {
        let item = json!({
            "metadata": { "name": "ip-10-0-1-5", "creationTimestamp": "2026-11-21T07:04:05Z" },
            "status": { "conditions": [{ "type": "Ready", "status": "True" }],
                        "allocatable": { "cpu": "2", "memory": "8Gi" },
                        "nodeInfo": { "kubeletVersion": "v1.31.3" } },
        });
        let mut metrics = Metrics::new();
        metrics.insert("ip-10-0-1-5".into(), Usage { cpu_milli: 500, mem_bytes: 2 * (1 << 30) });
        let t = build("nodes", &[item], NOW, false, &metrics);
        let at = t.columns.iter().position(|c| c == "CPU").expect("a CPU column");
        assert_eq!(&t.rows[0].cells[at..at + 2], ["500m (25%)", "2.0Gi (25%)"]);
    }

    #[test]
    fn columns_are_as_wide_as_their_widest_cell() {
        let t = build("pods", &[pod("a-very-long-pod-name", "Running", None, true)], NOW, false, &Metrics::new());
        assert_eq!(t.widths()[0], "a-very-long-pod-name".len());
    }
}
