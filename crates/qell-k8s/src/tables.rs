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
pub fn build(kind: &str, items: &[Value], now: i64, show_namespace: bool) -> Table {
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
    columns.extend(rest.iter().map(|c| c.to_string()));
    let mut rows: Vec<Row> = items
        .iter()
        .map(|item| {
            let (mut cells, health) = row_of(item, now);
            let namespace = Some(s(item, &["metadata", "namespace"])).filter(|n| !n.is_empty());
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
    for c in array(item, &["status", "initContainerStatuses"]).iter().chain(statuses) {
        let waiting = s(c, &["state", "waiting", "reason"]);
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
        let t = build("pods", &[pod("api-0", "Running", None, true)], now, false);
        assert_eq!(t.columns, ["NAME", "READY", "STATUS", "RESTARTS", "AGE", "IP", "NODE"]);
        assert_eq!(t.rows[0].cells, ["api-0", "1/1", "Running", "3", "2h", "10.0.1.9", "ip-10-0-1-5"]);
        assert_eq!(t.rows[0].health, Health::Ok);
        assert_eq!(t.rows[0].namespace.as_deref(), Some("web"));
    }

    #[test]
    fn a_waiting_container_names_the_trouble() {
        let t = build("pods", &[pod("api-1", "Pending", Some("CrashLoopBackOff"), false)], NOW, false);
        assert_eq!(t.rows[0].cells[2], "CrashLoopBackOff");
        assert_eq!(t.rows[0].health, Health::Bad);
    }

    #[test]
    fn a_pod_on_its_way_out_is_terminating() {
        let mut p = pod("api-2", "Running", None, true);
        p["metadata"]["deletionTimestamp"] = json!("2026-11-21T07:05:05Z");
        let t = build("pods", &[p], NOW, false);
        assert_eq!(t.rows[0].cells[2], "Terminating");
        assert_eq!(t.rows[0].health, Health::Warn);
    }

    #[test]
    fn listings_across_namespaces_say_which() {
        let t = build("pods", &[pod("api-0", "Running", None, true)], NOW, true);
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
        let t = build("deployments", &[item], NOW, false);
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
        let t = build("services", &[item], NOW, false);
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
        let t = build("nodes", &[item], NOW, false);
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
        let t = build("events", &[ev("Pulled", "Normal"), ev("BackOff", "Warning")], NOW, false);
        assert_eq!(t.rows[0].cells[2], "BackOff");
        assert_eq!(t.rows[0].health, Health::Bad);
        // Messages are one line, whatever the cluster sends.
        assert_eq!(t.rows[0].cells[4], "back-off restarting");
    }

    #[test]
    fn anything_else_still_lists_by_name_and_age() {
        let item = json!({ "metadata": { "name": "thing", "creationTimestamp": "2026-11-21T07:04:05Z" } });
        let t = build("widgets", &[item], NOW, false);
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
    fn columns_are_as_wide_as_their_widest_cell() {
        let t = build("pods", &[pod("a-very-long-pod-name", "Running", None, true)], NOW, false);
        assert_eq!(t.widths()[0], "a-very-long-pod-name".len());
    }
}
