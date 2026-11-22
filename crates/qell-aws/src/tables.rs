//! Turning the CLI's JSON into the columns worth a line.

use qell_table::{ago, bytes, Health, Row, Table};
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
            ago(&s(f, "LastModified"), now),
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
            ago(&s(d, "LastModifiedTime"), now),
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
        t.rows.push(row(id.clone(), vec![id, status, ago(&s(i, "CreateTime"), now)], health, i));
    }
    t
}

fn applications(answer: &Value) -> Table {
    let mut t = Table::new(&["APPLICATION"]);
    for a in items(answer, &["applications"]) {
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
    all.sort_by_key(|d| std::cmp::Reverse(qell_table::epoch_seconds(&s(d, "createTime")).unwrap_or(0)));
    for d in all {
        let id = s(d, "deploymentId");
        let status = s(d, "status");
        let health = match status.as_str() {
            "Succeeded" => Health::Ok,
            "Failed" | "Stopped" => Health::Bad,
            "" => Health::Plain,
            _ => Health::Warn,
        };
        let took = match (qell_table::epoch_seconds(&s(d, "createTime")), qell_table::epoch_seconds(&s(d, "completeTime"))) {
            (Some(start), Some(end)) => qell_table::age(end - start),
            _ => "-".to_string(),
        };
        // Why it failed is the line people come here for.
        let message = d["errorInformation"]["message"].as_str().unwrap_or("").replace('\n', " ");
        let cells = vec![
            id.clone(),
            s(d, "deploymentGroupName"),
            status,
            ago(&s(d, "createTime"), now),
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
        t.rows.push(row(name.clone(), vec![name, ago(&s(b, "CreationDate"), now)], Health::Plain, b));
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
            ago(&s(o, "LastModified"), now),
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
                ago(&s(i, "LaunchTime"), now),
            ];
            t.rows.push(row(id, cells, health, i));
        }
    }
    // By name, the way people look for a machine.
    t.rows.sort_by(|a, b| a.cells[1].cmp(&b.cells[1]));
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
