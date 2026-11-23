//! Turning the CLI's JSON into the columns worth a line.

use qell_table::{age, bytes, epoch_seconds, Health, Row, Table};
use serde_json::Value;

use crate::{Identity, Listing};

fn s(v: &Value, key: &str) -> String {
    match &v[key] {
        Value::String(s) => s.clone(),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        _ => String::new(),
    }
}

fn items<'a>(v: &'a Value, path: &[&str]) -> &'a [Value] {
    let mut cur = v;
    for key in path {
        cur = &cur[*key];
    }
    cur.as_array().map(Vec::as_slice).unwrap_or(&[])
}

/// When a field says something happened. The CLI writes dates as text
/// or, in its older versions, as seconds since the epoch.
fn when(v: &Value, key: &str) -> Option<i64> {
    match &v[key] {
        Value::Number(n) => n.as_f64().map(|f| f as i64),
        Value::String(text) => epoch_seconds(text).or_else(|| text.parse::<f64>().ok().map(|f| f as i64)),
        _ => None,
    }
}

/// How long ago that was, or a dash.
fn since(v: &Value, key: &str, now: i64) -> String {
    when(v, key).map(|then| age(now - then)).unwrap_or_else(|| "-".to_string())
}

/// How long something took, from one field to another.
fn took(v: &Value, start: &str, end: &str) -> String {
    match (when(v, start), when(v, end)) {
        (Some(a), Some(b)) => age(b - a),
        _ => "-".to_string(),
    }
}

/// The end of an ARN or a path: what people call the thing.
fn last(text: &str) -> String {
    text.rsplit(['/', ':']).next().unwrap_or(text).to_string()
}

fn dash(text: String) -> String {
    if text.is_empty() {
        "-".to_string()
    } else {
        text
    }
}

fn row(name: String, cells: Vec<String>, health: Health, object: &Value) -> Row {
    Row { cells, name, health, object: object.clone() }
}

/// Who the caller is, from `sts get-caller-identity`.
pub fn identity(v: &Value) -> Identity {
    let arn = s(v, "Arn");
    // arn:aws:sts::123456789012:assumed-role/Admin/nico → the end of it.
    let who = arn.splitn(6, ':').nth(5).unwrap_or(&arn).to_string();
    Identity { account: s(v, "Account"), who }
}

/// Build the table for a listing from the CLI's answer.
pub fn build(listing: &Listing, answer: &Value, now: i64) -> Table {
    match listing {
        Listing::Functions => functions(answer, now),
        Listing::Distributions => distributions(answer, now),
        Listing::Invalidations { .. } => invalidations(answer, now),
        Listing::Applications => applications(answer),
        Listing::Deployments { .. } => deployments(answer, now),
        Listing::Buckets => buckets(answer, now),
        Listing::Objects { prefix, .. } => objects(answer, prefix, now),
        Listing::Instances => instances(answer, now),
        Listing::Pipelines => pipelines(answer, now),
        Listing::PipelineState { .. } => pipeline_state(answer, now),
        Listing::BuildProjects => names(answer, "projects", "PROJECT"),
        Listing::Builds { .. } => builds(answer, now),
        Listing::Roles => roles(answer, now),
        Listing::RolePolicies { .. } => role_policies(answer),
        Listing::Users => users(answer, now),
        Listing::Clusters => clusters(answer),
        Listing::Services { .. } => services(answer, now),
        Listing::Tasks { .. } => tasks(answer, now),
        Listing::Databases => databases(answer),
        Listing::DbClusters => db_clusters(answer),
        Listing::Parameters => parameters(answer, now),
        Listing::StateMachines => state_machines(answer, now),
        Listing::Executions { .. } => executions(answer, now),
        Listing::Queues => queues(answer, now),
        Listing::UserPools => user_pools(answer, now),
        Listing::PoolClients { .. } => pool_clients(answer),
        Listing::Apis => apis(answer, now),
        Listing::Stages { .. } => stages(answer, now),
        // These two are built from what is on this machine.
        Listing::Profiles | Listing::Regions => Table::default(),
    }
}

/// The profiles, the one in use marked.
pub fn profiles(names: &[String], current: Option<&str>) -> Table {
    let mut t = Table::new(&["PROFILE", ""]);
    for name in names {
        let on = current == Some(name.as_str());
        let health = if on { Health::Ok } else { Health::Plain };
        t.rows.push(row(name.clone(), vec![name.clone(), if on { "(*)".into() } else { String::new() }], health, &Value::Null));
    }
    t
}

/// The regions, the one in use marked.
pub fn regions(current: Option<&str>) -> Table {
    let mut t = Table::new(&["REGION", "WHERE", ""]);
    for (name, place) in crate::REGIONS {
        let on = current == Some(*name);
        let health = if on { Health::Ok } else { Health::Plain };
        let cells = vec![name.to_string(), place.to_string(), if on { "(*)".into() } else { String::new() }];
        t.rows.push(row(name.to_string(), cells, health, &Value::Null));
    }
    t
}

fn functions(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["NAME", "RUNTIME", "MEMORY", "TIMEOUT", "SIZE", "MODIFIED"]);
    for f in items(answer, &["Functions"]) {
        let name = s(f, "FunctionName");
        // A container image has no runtime of its own to name.
        let runtime = match s(f, "Runtime") {
            r if r.is_empty() => s(f, "PackageType").to_lowercase(),
            r => r,
        };
        let cells = vec![
            name.clone(),
            dash(runtime),
            format!("{}MB", s(f, "MemorySize")),
            format!("{}s", s(f, "Timeout")),
            bytes(f["CodeSize"].as_u64().unwrap_or(0)),
            since(f, "LastModified", now),
        ];
        t.rows.push(row(name, cells, Health::Plain, f));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn distributions(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["ID", "STATUS", "ALIASES", "DOMAIN", "COMMENT", "MODIFIED"]);
    for d in items(answer, &["DistributionList", "Items"]) {
        let id = s(d, "Id");
        let enabled = d["Enabled"].as_bool().unwrap_or(true);
        let status = if enabled { s(d, "Status") } else { "Disabled".to_string() };
        let health = match status.as_str() {
            "Deployed" => Health::Ok,
            "Disabled" => Health::Plain,
            _ => Health::Warn,
        };
        let aliases: Vec<String> =
            items(d, &["Aliases", "Items"]).iter().filter_map(|a| a.as_str().map(str::to_string)).collect();
        let cells = vec![
            id.clone(),
            status,
            dash(aliases.join(",")),
            s(d, "DomainName"),
            dash(s(d, "Comment")),
            since(d, "LastModifiedTime", now),
        ];
        t.rows.push(row(id, cells, health, d));
    }
    t
}

fn invalidations(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["ID", "STATUS", "CREATED"]);
    for i in items(answer, &["InvalidationList", "Items"]) {
        let id = s(i, "Id");
        let status = s(i, "Status");
        let health = if status == "Completed" { Health::Ok } else { Health::Warn };
        t.rows.push(row(id.clone(), vec![id, status, since(i, "CreateTime", now)], health, i));
    }
    t
}

fn applications(answer: &Value) -> Table {
    names(answer, "applications", "APPLICATION")
}

/// The services that list nothing but names.
fn names(answer: &Value, key: &str, column: &str) -> Table {
    let mut t = Table::new(&[column]);
    for a in items(answer, &[key]) {
        let name = a.as_str().unwrap_or("").to_string();
        t.rows.push(row(name.clone(), vec![name], Health::Plain, a));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn deployments(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["ID", "GROUP", "STATUS", "CREATED", "TOOK", "BY", "MESSAGE"]);
    let mut all: Vec<&Value> = items(answer, &["deploymentsInfo"]).iter().collect();
    // Newest first, whatever order the batch came back in.
    all.sort_by_key(|d| std::cmp::Reverse(when(d, "createTime").unwrap_or(0)));
    for d in all {
        let id = s(d, "deploymentId");
        let status = s(d, "status");
        let health = match status.as_str() {
            "Succeeded" => Health::Ok,
            "Failed" | "Stopped" => Health::Bad,
            "" => Health::Plain,
            _ => Health::Warn,
        };
        let took = took(d, "createTime", "completeTime");
        // Why it failed is the line people come here for.
        let message = d["errorInformation"]["message"].as_str().unwrap_or("").replace('\n', " ");
        let cells = vec![
            id.clone(),
            s(d, "deploymentGroupName"),
            status,
            since(d, "createTime", now),
            took,
            dash(s(d, "creator")),
            message,
        ];
        t.rows.push(row(id, cells, health, d));
    }
    t
}

fn buckets(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["BUCKET", "CREATED"]);
    for b in items(answer, &["Buckets"]) {
        let name = s(b, "Name");
        t.rows.push(row(name.clone(), vec![name, since(b, "CreationDate", now)], Health::Plain, b));
    }
    t
}

fn objects(answer: &Value, prefix: &str, now: i64) -> Table {
    let mut t = Table::new(&["NAME", "SIZE", "CLASS", "MODIFIED"]);
    // Shown without the folder you are already in.
    let short = |key: &str| key.strip_prefix(prefix).unwrap_or(key).to_string();
    for p in items(answer, &["CommonPrefixes"]) {
        let key = s(p, "Prefix");
        t.rows.push(row(key.clone(), vec![short(&key), "-".into(), "folder".into(), "-".into()], Health::Plain, p));
    }
    for o in items(answer, &["Contents"]) {
        let key = s(o, "Key");
        // The folder's own placeholder object isn't something in it.
        if key == prefix {
            continue;
        }
        let cells = vec![
            short(&key),
            bytes(o["Size"].as_u64().unwrap_or(0)),
            dash(s(o, "StorageClass")).to_lowercase(),
            since(o, "LastModified", now),
        ];
        t.rows.push(row(key, cells, Health::Plain, o));
    }
    t
}

fn instances(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["ID", "NAME", "STATE", "TYPE", "PRIVATE IP", "PUBLIC IP", "AZ", "LAUNCHED"]);
    for r in items(answer, &["Reservations"]) {
        for i in items(r, &["Instances"]) {
            let id = s(i, "InstanceId");
            let name = items(i, &["Tags"]).iter().find(|t| s(t, "Key") == "Name").map(|t| s(t, "Value")).unwrap_or_default();
            let state = s(&i["State"], "Name");
            let health = match state.as_str() {
                "running" => Health::Ok,
                "stopped" | "terminated" => Health::Plain,
                _ => Health::Warn,
            };
            let cells = vec![
                id.clone(),
                dash(name),
                state,
                s(i, "InstanceType"),
                dash(s(i, "PrivateIpAddress")),
                dash(s(i, "PublicIpAddress")),
                s(&i["Placement"], "AvailabilityZone"),
                since(i, "LaunchTime", now),
            ];
            t.rows.push(row(id, cells, health, i));
        }
    }
    // By name, the way people look for a machine.
    t.rows.sort_by(|a, b| a.cells[1].cmp(&b.cells[1]));
    t
}

fn pipelines(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["NAME", "TYPE", "MODE", "VERSION", "UPDATED"]);
    for p in items(answer, &["pipelines"]) {
        let name = s(p, "name");
        let cells = vec![
            name.clone(),
            dash(s(p, "pipelineType")),
            dash(s(p, "executionMode")),
            s(p, "version"),
            since(p, "updated", now),
        ];
        t.rows.push(row(name, cells, Health::Plain, p));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

/// How a run's status reads, across the Code* services and Step
/// Functions, which spell the same few words in different cases.
fn run_health(status: &str) -> Health {
    match status.to_uppercase().as_str() {
        "SUCCEEDED" => Health::Ok,
        "FAILED" | "FAULT" | "TIMED_OUT" | "ABORTED" | "STOPPED" | "ABANDONED" => Health::Bad,
        "" => Health::Plain,
        _ => Health::Warn,
    }
}

/// One line per action, under its stage, so the red one is the answer
/// to "where is it stuck".
fn pipeline_state(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["STAGE", "ACTION", "STATUS", "CHANGED", "MESSAGE"]);
    for stage in items(answer, &["stageStates"]) {
        let stage_name = s(stage, "stageName");
        let actions = items(stage, &["actionStates"]);
        if actions.is_empty() {
            let status = s(&stage["latestExecution"], "status");
            let cells = vec![stage_name.clone(), "-".into(), dash(status.clone()), "-".into(), String::new()];
            t.rows.push(row(stage_name.clone(), cells, run_health(&status), stage));
        }
        for action in actions {
            let name = s(action, "actionName");
            let latest = &action["latestExecution"];
            let status = s(latest, "status");
            let message = match latest["errorDetails"]["message"].as_str() {
                Some(why) => why.replace('\n', " "),
                None => s(latest, "summary").replace('\n', " "),
            };
            let cells =
                vec![stage_name.clone(), name.clone(), dash(status.clone()), since(latest, "lastStatusChange", now), message];
            t.rows.push(row(format!("{stage_name}/{name}"), cells, run_health(&status), action));
        }
    }
    t
}

fn builds(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["#", "STATUS", "PHASE", "SOURCE", "STARTED", "TOOK", "BY"]);
    let mut all: Vec<&Value> = items(answer, &["builds"]).iter().collect();
    all.sort_by_key(|b| std::cmp::Reverse(when(b, "startTime").unwrap_or(0)));
    for b in all {
        let status = s(b, "buildStatus");
        let cells = vec![
            s(b, "buildNumber"),
            status.clone(),
            dash(s(b, "currentPhase")),
            dash(short(&s(b, "resolvedSourceVersion"), &s(b, "sourceVersion"))),
            since(b, "startTime", now),
            took(b, "startTime", "endTime"),
            dash(last(&s(b, "initiator"))),
        ];
        t.rows.push(row(s(b, "id"), cells, run_health(&status), b));
    }
    t
}

/// A commit as seven characters, or the branch it was asked by.
fn short(resolved: &str, asked: &str) -> String {
    let is_hash = resolved.len() == 40 && resolved.chars().all(|c| c.is_ascii_hexdigit());
    if is_hash {
        resolved[..7].to_string()
    } else if !asked.is_empty() {
        asked.to_string()
    } else {
        resolved.to_string()
    }
}

fn roles(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["ROLE", "PATH", "CREATED", "DESCRIPTION"]);
    for r in items(answer, &["Roles"]) {
        let name = s(r, "RoleName");
        t.rows.push(row(
            name.clone(),
            vec![name, s(r, "Path"), since(r, "CreateDate", now), s(r, "Description")],
            Health::Plain,
            r,
        ));
    }
    t
}

fn role_policies(answer: &Value) -> Table {
    let mut t = Table::new(&["POLICY", "ARN"]);
    for p in items(answer, &["AttachedPolicies"]) {
        let name = s(p, "PolicyName");
        t.rows.push(row(name.clone(), vec![name, s(p, "PolicyArn")], Health::Plain, p));
    }
    t
}

fn users(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["USER", "PATH", "CREATED", "PASSWORD USED"]);
    for u in items(answer, &["Users"]) {
        let name = s(u, "UserName");
        let cells = vec![name.clone(), s(u, "Path"), since(u, "CreateDate", now), since(u, "PasswordLastUsed", now)];
        t.rows.push(row(name, cells, Health::Plain, u));
    }
    t
}

fn clusters(answer: &Value) -> Table {
    let mut t = Table::new(&["CLUSTER", "STATUS", "SERVICES", "RUNNING", "PENDING", "INSTANCES"]);
    for c in items(answer, &["clusters"]) {
        let name = s(c, "clusterName");
        let status = s(c, "status");
        let health = if status == "ACTIVE" { Health::Ok } else { Health::Warn };
        let cells = vec![
            name.clone(),
            status,
            s(c, "activeServicesCount"),
            s(c, "runningTasksCount"),
            s(c, "pendingTasksCount"),
            s(c, "registeredContainerInstancesCount"),
        ];
        t.rows.push(row(name, cells, health, c));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn services(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["SERVICE", "STATUS", "DESIRED", "RUNNING", "PENDING", "LAUNCH", "TASK DEF", "ROLLOUT", "CREATED"]);
    for v in items(answer, &["services"]) {
        let name = s(v, "serviceName");
        let (desired, running) = (v["desiredCount"].as_i64().unwrap_or(0), v["runningCount"].as_i64().unwrap_or(0));
        // The deployment in charge is the first one.
        let rollout = s(&v["deployments"][0], "rolloutState");
        let health = if rollout == "FAILED" {
            Health::Bad
        } else if running < desired || rollout == "IN_PROGRESS" || s(v, "status") != "ACTIVE" {
            Health::Warn
        } else {
            Health::Ok
        };
        let launch = match s(v, "launchType") {
            l if l.is_empty() => s(&v["capacityProviderStrategy"][0], "capacityProvider"),
            l => l,
        };
        let cells = vec![
            name.clone(),
            s(v, "status"),
            desired.to_string(),
            running.to_string(),
            s(v, "pendingCount"),
            dash(launch),
            // `web:42`: the family and its revision, which is one name.
            s(v, "taskDefinition").rsplit('/').next().unwrap_or_default().to_string(),
            dash(rollout),
            since(v, "createdAt", now),
        ];
        t.rows.push(row(name, cells, health, v));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn tasks(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["TASK", "STATUS", "DESIRED", "HEALTH", "CONTAINERS", "CPU", "MEM", "STARTED", "REASON"]);
    for k in items(answer, &["tasks"]) {
        let id = last(&s(k, "taskArn"));
        let (status, check) = (s(k, "lastStatus"), s(k, "healthStatus"));
        let health = match (status.as_str(), check.as_str()) {
            (_, "UNHEALTHY") => Health::Bad,
            ("RUNNING", _) => Health::Ok,
            ("STOPPED", _) => Health::Plain,
            _ => Health::Warn,
        };
        let cells = vec![
            id.clone(),
            status,
            s(k, "desiredStatus"),
            dash(check),
            items(k, &["containers"]).len().to_string(),
            dash(s(k, "cpu")),
            dash(s(k, "memory")),
            since(k, "startedAt", now),
            s(k, "stoppedReason"),
        ];
        t.rows.push(row(id, cells, health, k));
    }
    t
}

/// RDS says how a database is in one word, and there are many of them.
fn db_health(status: &str) -> Health {
    match status {
        "available" => Health::Ok,
        "stopped" => Health::Plain,
        "failed" | "storage-full" | "restore-error" | "inaccessible-encryption-credentials" => Health::Bad,
        other if other.starts_with("incompatible") => Health::Bad,
        _ => Health::Warn,
    }
}

fn databases(answer: &Value) -> Table {
    let mut t = Table::new(&["DATABASE", "ENGINE", "VERSION", "CLASS", "STATUS", "MULTI-AZ", "AZ", "CLUSTER", "ENDPOINT"]);
    for d in items(answer, &["DBInstances"]) {
        let name = s(d, "DBInstanceIdentifier");
        let status = s(d, "DBInstanceStatus");
        let cells = vec![
            name.clone(),
            s(d, "Engine"),
            s(d, "EngineVersion"),
            s(d, "DBInstanceClass"),
            status.clone(),
            if d["MultiAZ"].as_bool().unwrap_or(false) { "yes".into() } else { "no".into() },
            dash(s(d, "AvailabilityZone")),
            dash(s(d, "DBClusterIdentifier")),
            dash(s(&d["Endpoint"], "Address")),
        ];
        t.rows.push(row(name, cells, db_health(&status), d));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn db_clusters(answer: &Value) -> Table {
    let mut t = Table::new(&["CLUSTER", "ENGINE", "VERSION", "STATUS", "MEMBERS", "ENDPOINT"]);
    for c in items(answer, &["DBClusters"]) {
        let name = s(c, "DBClusterIdentifier");
        let status = s(c, "Status");
        let cells = vec![
            name.clone(),
            s(c, "Engine"),
            s(c, "EngineVersion"),
            status.clone(),
            items(c, &["DBClusterMembers"]).len().to_string(),
            dash(s(c, "Endpoint")),
        ];
        t.rows.push(row(name, cells, db_health(&status), c));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

/// Names and types only: a parameter's value is never asked for.
fn parameters(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["PARAMETER", "TYPE", "TIER", "MODIFIED", "BY", "DESCRIPTION"]);
    for p in items(answer, &["Parameters"]) {
        let name = s(p, "Name");
        let cells = vec![
            name.clone(),
            s(p, "Type"),
            s(p, "Tier"),
            since(p, "LastModifiedDate", now),
            dash(last(&s(p, "LastModifiedUser"))),
            s(p, "Description"),
        ];
        t.rows.push(row(name, cells, Health::Plain, p));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn state_machines(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["STATE MACHINE", "TYPE", "CREATED"]);
    for m in items(answer, &["stateMachines"]) {
        let name = s(m, "name");
        t.rows.push(row(name.clone(), vec![name, s(m, "type"), since(m, "creationDate", now)], Health::Plain, m));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn executions(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["EXECUTION", "STATUS", "STARTED", "TOOK"]);
    for e in items(answer, &["executions"]) {
        let name = s(e, "name");
        let status = s(e, "status");
        let cells = vec![name.clone(), status.clone(), since(e, "startDate", now), took(e, "startDate", "stopDate")];
        t.rows.push(row(name, cells, run_health(&status), e));
    }
    t
}

fn queues(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["QUEUE", "MESSAGES", "IN FLIGHT", "DELAYED", "TYPE", "DLQ", "CREATED"]);
    for q in items(answer, &["Queues"]) {
        let name = last(&s(q, "QueueUrl"));
        let a = &q["Attributes"];
        let cells = vec![
            name.clone(),
            dash(s(a, "ApproximateNumberOfMessages")),
            dash(s(a, "ApproximateNumberOfMessagesNotVisible")),
            dash(s(a, "ApproximateNumberOfMessagesDelayed")),
            if name.ends_with(".fifo") { "fifo".into() } else { "standard".into() },
            // Where its failures go, when it sends them somewhere.
            match a["RedrivePolicy"].as_str().and_then(|p| serde_json::from_str::<Value>(p).ok()) {
                Some(policy) => dash(last(&s(&policy, "deadLetterTargetArn"))),
                None => "-".into(),
            },
            since(a, "CreatedTimestamp", now),
        ];
        t.rows.push(row(name, cells, Health::Plain, q));
    }
    t.rows.sort_by(|a, b| a.name.cmp(&b.name));
    t
}

fn user_pools(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["POOL", "ID", "CREATED", "MODIFIED"]);
    for p in items(answer, &["UserPools"]) {
        let id = s(p, "Id");
        let cells = vec![s(p, "Name"), id.clone(), since(p, "CreationDate", now), since(p, "LastModifiedDate", now)];
        t.rows.push(row(id, cells, Health::Plain, p));
    }
    t.rows.sort_by(|a, b| a.cells[0].cmp(&b.cells[0]));
    t
}

fn pool_clients(answer: &Value) -> Table {
    let mut t = Table::new(&["CLIENT", "CLIENT ID"]);
    for c in items(answer, &["UserPoolClients"]) {
        let id = s(c, "ClientId");
        t.rows.push(row(id.clone(), vec![s(c, "ClientName"), id], Health::Plain, c));
    }
    t
}

/// REST APIs and the newer HTTP and WebSocket ones, in one list.
fn apis(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["API", "ID", "TYPE", "ENDPOINT", "CREATED"]);
    for a in items(answer, &["rest"]) {
        let id = s(a, "id");
        let kinds: Vec<&str> = items(a, &["endpointConfiguration", "types"]).iter().filter_map(Value::as_str).collect();
        let cells =
            vec![s(a, "name"), id.clone(), "REST".into(), dash(kinds.join(",").to_lowercase()), since(a, "createdDate", now)];
        t.rows.push(row(id, cells, Health::Plain, a));
    }
    for a in items(answer, &["v2"]) {
        let id = s(a, "ApiId");
        let cells = vec![s(a, "Name"), id.clone(), s(a, "ProtocolType"), dash(s(a, "ApiEndpoint")), since(a, "CreatedDate", now)];
        t.rows.push(row(id, cells, Health::Plain, a));
    }
    t.rows.sort_by(|a, b| a.cells[0].cmp(&b.cells[0]));
    t
}

/// Stages of either generation: they differ only in their capitals.
fn stages(answer: &Value, now: i64) -> Table {
    let mut t = Table::new(&["STAGE", "DEPLOYMENT", "UPDATED", "AUTO DEPLOY"]);
    for st in items(answer, &["item"]) {
        let name = s(st, "stageName");
        t.rows.push(row(
            name.clone(),
            vec![name, dash(s(st, "deploymentId")), since(st, "lastUpdatedDate", now), "-".into()],
            Health::Plain,
            st,
        ));
    }
    for st in items(answer, &["Items"]) {
        let name = s(st, "StageName");
        let auto = if st["AutoDeploy"].as_bool().unwrap_or(false) { "yes" } else { "no" };
        let cells = vec![name.clone(), dash(s(st, "DeploymentId")), since(st, "LastUpdatedDate", now), auto.into()];
        t.rows.push(row(name, cells, Health::Plain, st));
    }
    t
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const NOW: i64 = 1_795_244_645 + 7_200; // two hours after the stamps below
    const THEN: &str = "2026-11-21T07:04:05+00:00";

    #[test]
    fn the_identity_keeps_the_readable_end_of_the_arn() {
        let v = json!({ "Account": "123456789012", "Arn": "arn:aws:sts::123456789012:assumed-role/Admin/nico" });
        assert_eq!(identity(&v), Identity { account: "123456789012".into(), who: "assumed-role/Admin/nico".into() });
    }

    #[test]
    fn lambdas_show_what_they_run_on() {
        let answer = json!({ "Functions": [
            { "FunctionName": "resize", "Runtime": "python3.12", "MemorySize": 512, "Timeout": 30, "CodeSize": 2048, "LastModified": "2026-11-21T07:04:05.000+0000" },
            { "FunctionName": "api", "PackageType": "Image", "MemorySize": 1024, "Timeout": 5, "CodeSize": 0, "LastModified": THEN },
        ]});
        let t = build(&Listing::Functions, &answer, NOW);
        // Sorted by name; an image has no runtime, and says so.
        assert_eq!(t.rows[0].cells, ["api", "image", "1024MB", "5s", "0 B", "2h"]);
        assert_eq!(t.rows[1].cells, ["resize", "python3.12", "512MB", "30s", "2.0 KB", "2h"]);
    }

    #[test]
    fn distributions_say_whether_they_are_out() {
        let answer = json!({ "DistributionList": { "Items": [
            { "Id": "E1", "Status": "Deployed", "Enabled": true, "DomainName": "d1.cloudfront.net",
              "Aliases": { "Items": ["www.example.com", "example.com"] }, "Comment": "site", "LastModifiedTime": THEN },
            { "Id": "E2", "Status": "InProgress", "Enabled": true, "DomainName": "d2.cloudfront.net", "Aliases": { "Quantity": 0 }, "Comment": "" },
            { "Id": "E3", "Status": "Deployed", "Enabled": false, "DomainName": "d3.cloudfront.net" },
        ]}});
        let t = build(&Listing::Distributions, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["E1", "Deployed", "www.example.com,example.com", "d1.cloudfront.net", "site", "2h"]);
        assert_eq!(t.rows[0].health, Health::Ok);
        assert_eq!((t.rows[1].cells[1].as_str(), t.rows[1].health), ("InProgress", Health::Warn));
        assert_eq!((t.rows[2].cells[1].as_str(), t.rows[2].health), ("Disabled", Health::Plain));
    }

    #[test]
    fn deployments_come_newest_first_with_why_they_failed() {
        let answer = json!({ "deploymentsInfo": [
            { "deploymentId": "d-OLD", "deploymentGroupName": "prod", "status": "Succeeded",
              "createTime": "2026-11-20T07:04:05+00:00", "completeTime": "2026-11-20T07:09:05+00:00", "creator": "user" },
            { "deploymentId": "d-NEW", "deploymentGroupName": "prod", "status": "Failed", "createTime": THEN,
              "creator": "autoscaling", "errorInformation": { "message": "The overall deployment failed\nbecause too many instances failed" } },
        ]});
        let t = build(&Listing::Deployments { application: "api".into() }, &answer, NOW);
        assert_eq!(t.rows[0].name, "d-NEW");
        assert_eq!(t.rows[0].health, Health::Bad);
        assert_eq!(t.rows[0].cells[6], "The overall deployment failed because too many instances failed");
        assert_eq!((t.rows[1].cells[4].as_str(), t.rows[1].health), ("5m", Health::Ok));
    }

    #[test]
    fn a_bucket_level_lists_folders_then_objects_by_their_short_names() {
        let answer = json!({
            "CommonPrefixes": [{ "Prefix": "img/icons/" }],
            "Contents": [
                { "Key": "img/", "Size": 0 },
                { "Key": "img/logo.png", "Size": 5120, "StorageClass": "STANDARD", "LastModified": THEN },
            ],
        });
        let t = build(&Listing::Objects { bucket: "assets".into(), prefix: "img/".into() }, &answer, NOW);
        assert_eq!(t.rows.len(), 2, "the folder's own placeholder is left out");
        // The name keeps the whole key, for going in; the cell is short.
        assert_eq!((t.rows[0].name.as_str(), t.rows[0].cells[0].as_str()), ("img/icons/", "icons/"));
        assert_eq!(t.rows[1].cells, ["logo.png", "5.0 KB", "standard", "2h"]);
    }

    #[test]
    fn instances_are_found_by_their_name_tag() {
        let answer = json!({ "Reservations": [{ "Instances": [
            { "InstanceId": "i-0b", "State": { "Name": "stopped" }, "InstanceType": "t3.micro", "Placement": { "AvailabilityZone": "eu-west-3a" },
              "Tags": [{ "Key": "Name", "Value": "worker" }], "LaunchTime": THEN },
            { "InstanceId": "i-0a", "State": { "Name": "running" }, "InstanceType": "m6i.large", "PrivateIpAddress": "10.0.1.5",
              "PublicIpAddress": "15.1.2.3", "Placement": { "AvailabilityZone": "eu-west-3b" },
              "Tags": [{ "Key": "env", "Value": "prod" }, { "Key": "Name", "Value": "bastion" }], "LaunchTime": THEN },
        ]}]});
        let t = build(&Listing::Instances, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["i-0a", "bastion", "running", "m6i.large", "10.0.1.5", "15.1.2.3", "eu-west-3b", "2h"]);
        assert_eq!(t.rows[0].health, Health::Ok);
        assert_eq!((t.rows[1].cells[4].as_str(), t.rows[1].health), ("-", Health::Plain));
    }

    #[test]
    fn dates_are_read_as_text_or_as_seconds() {
        let v = json!({ "text": THEN, "number": 1_795_244_645.25, "numeric": "1795244645", "none": null });
        for key in ["text", "number", "numeric"] {
            assert_eq!(since(&v, key, NOW), "2h", "{key}");
        }
        assert_eq!(since(&v, "none", NOW), "-");
        assert_eq!(last("arn:aws:ecs:eu-west-3:1:task/prod/0123abcd"), "0123abcd");
        assert_eq!(last("arn:aws:sts::1:assumed-role/Admin/nico"), "nico");
    }

    #[test]
    fn a_pipeline_shows_where_it_is_stuck() {
        let answer = json!({ "stageStates": [
            { "stageName": "Source", "actionStates": [{ "actionName": "Checkout", "latestExecution": { "status": "Succeeded", "lastStatusChange": THEN, "summary": "fix: typo" } }] },
            { "stageName": "Deploy", "actionStates": [{ "actionName": "Prod", "latestExecution": { "status": "Failed", "lastStatusChange": THEN,
                "errorDetails": { "message": "Deployment d-BBB222 failed" } } }] },
            { "stageName": "Notify", "latestExecution": { "status": "InProgress" } },
        ]});
        let t = build(&Listing::PipelineState { pipeline: "web".into() }, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["Source", "Checkout", "Succeeded", "2h", "fix: typo"]);
        assert_eq!((t.rows[1].name.as_str(), t.rows[1].health), ("Deploy/Prod", Health::Bad));
        assert_eq!(t.rows[1].cells[4], "Deployment d-BBB222 failed");
        assert_eq!((t.rows[2].cells[1].as_str(), t.rows[2].health), ("-", Health::Warn));
    }

    #[test]
    fn builds_come_newest_first_with_a_short_commit() {
        let answer = json!({ "builds": [
            { "id": "web:1", "buildNumber": 41, "buildStatus": "SUCCEEDED", "currentPhase": "COMPLETED", "startTime": "2026-11-20T07:04:05+00:00",
              "endTime": "2026-11-20T07:07:05+00:00", "resolvedSourceVersion": "0123456789abcdef0123456789abcdef01234567", "initiator": "codepipeline/web" },
            { "id": "web:2", "buildNumber": 42, "buildStatus": "FAILED", "currentPhase": "BUILD", "startTime": THEN, "sourceVersion": "refs/heads/main", "initiator": "nicolas" },
        ]});
        let t = build(&Listing::Builds { project: "web".into() }, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["42", "FAILED", "BUILD", "refs/heads/main", "2h", "-", "nicolas"]);
        assert_eq!(t.rows[0].health, Health::Bad);
        assert_eq!(t.rows[1].cells, ["41", "SUCCEEDED", "COMPLETED", "0123456", "1d2h", "3m", "web"]);
    }

    #[test]
    fn an_ecs_service_short_of_its_tasks_is_a_warning() {
        let answer = json!({ "services": [
            { "serviceName": "web", "status": "ACTIVE", "desiredCount": 3, "runningCount": 3, "pendingCount": 0, "launchType": "FARGATE",
              "taskDefinition": "arn:aws:ecs:eu-west-3:1:task-definition/web:42", "deployments": [{ "rolloutState": "COMPLETED" }], "createdAt": THEN },
            { "serviceName": "api", "status": "ACTIVE", "desiredCount": 4, "runningCount": 2, "pendingCount": 2,
              "capacityProviderStrategy": [{ "capacityProvider": "FARGATE_SPOT" }], "taskDefinition": "arn:aws:ecs:eu-west-3:1:task-definition/api:7",
              "deployments": [{ "rolloutState": "IN_PROGRESS" }] },
            { "serviceName": "cron", "status": "ACTIVE", "desiredCount": 1, "runningCount": 0, "deployments": [{ "rolloutState": "FAILED" }] },
        ]});
        let t = build(&Listing::Services { cluster: "prod".into() }, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["api", "ACTIVE", "4", "2", "2", "FARGATE_SPOT", "api:7", "IN_PROGRESS", "-"]);
        assert_eq!((t.rows[0].health, t.rows[1].health, t.rows[2].health), (Health::Warn, Health::Bad, Health::Ok));
        assert_eq!(t.rows[2].cells[6], "web:42");
    }

    #[test]
    fn a_task_is_named_by_the_end_of_its_arn() {
        let answer = json!({ "tasks": [
            { "taskArn": "arn:aws:ecs:eu-west-3:1:task/prod/0123abcd", "lastStatus": "RUNNING", "desiredStatus": "RUNNING", "healthStatus": "HEALTHY",
              "containers": [{ "name": "app" }, { "name": "envoy" }], "cpu": "512", "memory": "1024", "startedAt": THEN },
            { "taskArn": "arn:aws:ecs:eu-west-3:1:task/prod/4567ef", "lastStatus": "STOPPED", "desiredStatus": "STOPPED", "stoppedReason": "Essential container in task exited" },
        ]});
        let t = build(&Listing::Tasks { cluster: "prod".into(), service: "api".into() }, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["0123abcd", "RUNNING", "RUNNING", "HEALTHY", "2", "512", "1024", "2h", ""]);
        assert_eq!((t.rows[1].health, t.rows[1].cells[8].as_str()), (Health::Plain, "Essential container in task exited"));
    }

    #[test]
    fn a_queue_shows_how_full_it_is_and_where_failures_go() {
        let answer = json!({ "Queues": [
            { "QueueUrl": "https://sqs.eu-west-3.amazonaws.com/1/orders.fifo", "Attributes": { "ApproximateNumberOfMessages": "12",
              "ApproximateNumberOfMessagesNotVisible": "3", "ApproximateNumberOfMessagesDelayed": "0", "CreatedTimestamp": "1795244645",
              "RedrivePolicy": "{\"deadLetterTargetArn\":\"arn:aws:sqs:eu-west-3:1:orders-dlq.fifo\",\"maxReceiveCount\":5}" } },
            { "QueueUrl": "https://sqs.eu-west-3.amazonaws.com/1/mail", "Attributes": null },
        ]});
        let t = build(&Listing::Queues, &answer, NOW);
        assert_eq!(t.rows[1].cells, ["orders.fifo", "12", "3", "0", "fifo", "orders-dlq.fifo", "2h"]);
        // A queue that didn't answer keeps its line.
        assert_eq!(t.rows[0].cells, ["mail", "-", "-", "-", "standard", "-", "-"]);
    }

    #[test]
    fn both_generations_of_api_gateway_share_a_list() {
        let answer = json!({
            "rest": [{ "id": "r9", "name": "legacy", "createdDate": THEN, "endpointConfiguration": { "types": ["REGIONAL"] } }],
            "v2": [{ "ApiId": "a1b2", "Name": "bff", "ProtocolType": "HTTP", "ApiEndpoint": "https://a1b2.execute-api.eu-west-3.amazonaws.com", "CreatedDate": THEN }],
        });
        let t = build(&Listing::Apis, &answer, NOW);
        assert_eq!(t.rows[0].cells, ["bff", "a1b2", "HTTP", "https://a1b2.execute-api.eu-west-3.amazonaws.com", "2h"]);
        assert_eq!(t.rows[1].cells, ["legacy", "r9", "REST", "regional", "2h"]);
        let old = build(
            &Listing::Stages { api: "r9".into(), v2: false },
            &json!({ "item": [{ "stageName": "prod", "deploymentId": "d1", "lastUpdatedDate": THEN }] }),
            NOW,
        );
        assert_eq!(old.rows[0].cells, ["prod", "d1", "2h", "-"]);
        let new = build(
            &Listing::Stages { api: "a1b2".into(), v2: true },
            &json!({ "Items": [{ "StageName": "$default", "AutoDeploy": true }] }),
            NOW,
        );
        assert_eq!(new.rows[0].cells, ["$default", "-", "-", "yes"]);
    }

    #[test]
    fn databases_and_the_rest_read_their_own_words() {
        let dbs = build(
            &Listing::Databases,
            &json!({ "DBInstances": [
                { "DBInstanceIdentifier": "main-1", "Engine": "aurora-postgresql", "EngineVersion": "16.4", "DBInstanceClass": "db.r7g.large",
                  "DBInstanceStatus": "available", "MultiAZ": false, "AvailabilityZone": "eu-west-3a", "DBClusterIdentifier": "main", "Endpoint": { "Address": "main-1.x.rds.amazonaws.com" } },
                { "DBInstanceIdentifier": "old", "Engine": "mysql", "EngineVersion": "8.0", "DBInstanceClass": "db.t3.micro", "DBInstanceStatus": "storage-full", "MultiAZ": true },
            ]}),
            NOW,
        );
        assert_eq!(
            dbs.rows[0].cells,
            [
                "main-1",
                "aurora-postgresql",
                "16.4",
                "db.r7g.large",
                "available",
                "no",
                "eu-west-3a",
                "main",
                "main-1.x.rds.amazonaws.com"
            ]
        );
        assert_eq!((dbs.rows[0].health, dbs.rows[1].health), (Health::Ok, Health::Bad));
        let params = build(
            &Listing::Parameters,
            &json!({ "Parameters": [{ "Name": "/prod/db/password", "Type": "SecureString", "Tier": "Standard",
            "LastModifiedDate": THEN, "LastModifiedUser": "arn:aws:sts::1:assumed-role/Admin/nico" }] }),
            NOW,
        );
        assert_eq!(params.rows[0].cells, ["/prod/db/password", "SecureString", "Standard", "2h", "nico", ""]);
        let runs = build(
            &Listing::Executions { machine: "arn".into(), name: "nightly".into() },
            &json!({ "executions": [
            { "name": "run-2", "status": "TIMED_OUT", "startDate": THEN, "stopDate": "2026-11-21T08:04:05+00:00" }] }),
            NOW,
        );
        assert_eq!(
            (runs.rows[0].cells.clone(), runs.rows[0].health),
            (vec!["run-2".to_string(), "TIMED_OUT".into(), "2h".into(), "1h".into()], Health::Bad)
        );
        let pools = build(
            &Listing::UserPools,
            &json!({ "UserPools": [{ "Id": "eu-west-3_AbC", "Name": "customers", "CreationDate": THEN }] }),
            NOW,
        );
        assert_eq!((pools.rows[0].name.as_str(), pools.rows[0].cells[0].as_str()), ("eu-west-3_AbC", "customers"));
    }

    #[test]
    fn the_pickers_mark_what_is_in_use() {
        let t = profiles(&["dev".to_string(), "prod".to_string()], Some("prod"));
        assert_eq!(t.rows[1].cells, ["prod", "(*)"]);
        assert_eq!((t.rows[0].health, t.rows[1].health), (Health::Plain, Health::Ok));
        let r = regions(Some("eu-west-3"));
        assert_eq!(r.rows[1].cells, ["eu-west-3", "Paris", "(*)"]);
    }

    #[test]
    fn an_empty_answer_is_an_empty_table_with_its_columns() {
        for listing in [Listing::Functions, Listing::Buckets, Listing::Instances, Listing::Applications] {
            let t = build(&listing, &Value::Null, NOW);
            assert!(t.rows.is_empty());
            assert!(!t.columns.is_empty());
        }
    }
}
