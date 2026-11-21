//! Reading a Kubernetes cluster for Qell's cluster view.
//!
//! Everything goes through `kubectl`, on purpose: it already knows the
//! kubeconfig, the contexts and the credential plugins (`aws eks get-token`
//! and friends), so Qell doesn't hold credentials or speak to the API
//! server itself. Nothing is installed and nothing runs on its own; the
//! view asks, this crate runs one command and parses what comes back.

pub mod tables;

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

pub use tables::{age, Health, Metrics, Row, Table, Usage};

/// How long one `kubectl` call may take before it is given up on. A
/// cluster behind a dead VPN would otherwise hang the worker.
const TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `kubectl` isn't on the PATH.
    NotInstalled,
    /// It ran and said no (no cluster, no permission, unknown resource).
    Failed(String),
    /// It took longer than [`TIMEOUT`].
    TimedOut,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled => write!(
                f,
                "kubectl isn't installed, and Qell reads your cluster through it. Install it the way you install your other \
                 tools (brew install kubectl, or your distribution's package), then press r. Qell never installs anything \
                 for you."
            ),
            Error::TimedOut => write!(f, "kubectl didn't answer within {}s. Is the cluster reachable?", TIMEOUT.as_secs()),
            Error::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// One kind the view can list, with the short names people type.
pub struct Kind {
    pub name: &'static str,
    pub aliases: &'static [&'static str],
    /// Whether it lives in a namespace (nodes and namespaces don't).
    pub namespaced: bool,
}

/// The kinds `:` completes. Anything else you type is passed to kubectl
/// as it is, so custom resources work too, with name and age for columns.
pub const KINDS: &[Kind] = &[
    Kind { name: "pods", aliases: &["po", "pod"], namespaced: true },
    Kind { name: "deployments", aliases: &["deploy", "deployment"], namespaced: true },
    Kind { name: "statefulsets", aliases: &["sts", "statefulset"], namespaced: true },
    Kind { name: "daemonsets", aliases: &["ds", "daemonset"], namespaced: true },
    Kind { name: "services", aliases: &["svc", "service"], namespaced: true },
    Kind { name: "ingresses", aliases: &["ing", "ingress"], namespaced: true },
    Kind { name: "configmaps", aliases: &["cm", "configmap"], namespaced: true },
    Kind { name: "secrets", aliases: &["secret"], namespaced: true },
    Kind { name: "jobs", aliases: &["job"], namespaced: true },
    Kind { name: "cronjobs", aliases: &["cj", "cronjob"], namespaced: true },
    Kind { name: "persistentvolumeclaims", aliases: &["pvc"], namespaced: true },
    Kind { name: "events", aliases: &["ev", "event"], namespaced: true },
    Kind { name: "nodes", aliases: &["no", "node"], namespaced: false },
    Kind { name: "namespaces", aliases: &["ns", "namespace"], namespaced: false },
    Kind { name: "persistentvolumes", aliases: &["pv"], namespaced: false },
];

/// The kind a typed word means (`po` → `pods`), or the word itself.
pub fn resolve(word: &str) -> String {
    let word = word.trim().to_lowercase();
    KINDS.iter().find(|k| k.name == word || k.aliases.contains(&word.as_str())).map(|k| k.name.to_string()).unwrap_or(word)
}

/// Whether a kind lives in a namespace. Unknown kinds are assumed to.
pub fn is_namespaced(kind: &str) -> bool {
    KINDS.iter().find(|k| k.name == kind).map(|k| k.namespaced).unwrap_or(true)
}

/// What to list: the kind, where, and how it was narrowed down.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Listing {
    pub kind: String,
    /// `None` means every namespace (`-A`).
    pub namespace: Option<String>,
    /// `-l app=api`, for the pods of a deployment or a service.
    pub selector: Option<String>,
    /// `--field-selector spec.nodeName=…`, for the pods of a node.
    pub field_selector: Option<String>,
    /// One pod's containers instead of a kubectl listing.
    pub containers_of: Option<String>,
}

impl Listing {
    pub fn new(kind: &str, namespace: Option<String>) -> Self {
        Self { kind: kind.to_string(), namespace, ..Default::default() }
    }

    /// The line the view shows above the table.
    pub fn title(&self) -> String {
        if let Some(pod) = &self.containers_of {
            return format!("containers of {pod}");
        }
        let mut title = self.kind.clone();
        if let Some(sel) = &self.selector {
            title.push_str(&format!("  -l {sel}"));
        }
        if let Some(f) = &self.field_selector {
            title.push_str(&format!("  {f}"));
        }
        title
    }
}

/// Runs `kubectl`. The binary is configurable because people keep it in
/// odd places (asdf, a version manager, a wrapper script).
#[derive(Debug, Clone)]
pub struct Client {
    pub bin: String,
    /// The context to use, or the kubeconfig's current one when `None`.
    pub context: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self { bin: "kubectl".to_string(), context: None }
    }
}

/// A plural kind as one of it, for labels: `pods` → `pod`, `ingresses` →
/// `ingress`, `networkpolicies` → `networkpolicy`.
pub fn singular(kind: &str) -> String {
    let stem = kind.split('.').next().unwrap_or(kind);
    if let Some(head) = stem.strip_suffix("ies").filter(|h| !h.is_empty()) {
        return format!("{head}y");
    }
    for ending in ["ses", "xes", "ches", "shes"] {
        if stem.ends_with(ending) {
            return stem[..stem.len() - 2].to_string();
        }
    }
    stem.strip_suffix('s').unwrap_or(stem).to_string()
}

/// What one object is called, for the commands below.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Object {
    pub kind: String,
    pub name: String,
    pub namespace: Option<String>,
}

impl Object {
    /// `pod/api-0`, the way kubectl and k9s write it.
    pub fn slash(&self) -> String {
        format!("{}/{}", singular(&self.kind), self.name)
    }

    fn args(&self) -> Vec<String> {
        let mut args = vec![self.kind.clone(), self.name.clone()];
        if let Some(ns) = &self.namespace {
            args.extend(["-n".to_string(), ns.clone()]);
        }
        args
    }
}

impl Client {
    pub fn new(bin: impl Into<String>) -> Self {
        Self { bin: bin.into(), context: None }
    }

    /// Run kubectl with these arguments and give back its stdout.
    pub fn run(&self, args: &[&str]) -> Result<String, Error> {
        let mut cmd = Command::new(&self.bin);
        if let Some(ctx) = &self.context {
            cmd.args(["--context", ctx]);
        }
        cmd.args(args);
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::NotInstalled),
            Err(e) => return Err(Error::Failed(e.to_string())),
        };
        // `wait_with_output` has no deadline of its own, so it waits on a
        // thread and the child is killed if it overstays.
        let (tx, rx) = std::sync::mpsc::channel();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        std::thread::Builder::new()
            .name("kubectl".into())
            .spawn(move || {
                use std::io::Read;
                let mut out = String::new();
                let mut err = String::new();
                if let Some(mut s) = stdout {
                    let _ = s.read_to_string(&mut out);
                }
                if let Some(mut s) = stderr {
                    let _ = s.read_to_string(&mut err);
                }
                let _ = tx.send((out, err));
            })
            .map_err(|e| Error::Failed(e.to_string()))?;
        let output = rx.recv_timeout(TIMEOUT);
        let (out, err) = match output {
            Ok(pair) => pair,
            Err(_) => {
                let _ = child.kill();
                return Err(Error::TimedOut);
            }
        };
        let status = match child.wait() {
            Ok(s) => s,
            Err(e) => return Err(Error::Failed(e.to_string())),
        };
        if status.success() {
            Ok(out)
        } else {
            Err(Error::Failed(tidy_error(&err)))
        }
    }

    /// `kubectl describe`, as the terminal would print it.
    pub fn describe(&self, object: &Object) -> Result<String, Error> {
        let mut args = vec!["describe".to_string()];
        args.extend(object.args());
        self.run(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// The object's manifest, as YAML.
    pub fn manifest(&self, object: &Object) -> Result<String, Error> {
        let mut args = vec!["get".to_string()];
        args.extend(object.args());
        args.extend(["-o".to_string(), "yaml".to_string()]);
        self.run(&args.iter().map(String::as_str).collect::<Vec<_>>())
    }

    /// Send a manifest file back to the cluster. Returns what kubectl
    /// says it did ("deployment.apps/api configured").
    pub fn apply(&self, path: &std::path::Path) -> Result<String, Error> {
        let path = path.to_string_lossy().into_owned();
        self.run(&["apply", "-f", &path]).map(|out| out.trim().to_string())
    }

    /// The contexts in the kubeconfig, and which one is current.
    pub fn contexts(&self) -> Result<(Vec<String>, Option<String>), Error> {
        let names = self.run(&["config", "get-contexts", "-o", "name"])?;
        let current = self.run(&["config", "current-context"]).ok().map(|c| c.trim().to_string());
        let mut list: Vec<String> = names.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
        list.sort();
        Ok((list, current.filter(|c| !c.is_empty())))
    }

    /// What the pods of a namespace (or of every namespace) are using.
    /// Empty when metrics-server isn't installed; the caller decides
    /// whether to ask again.
    pub fn top_pods(&self, namespace: Option<&str>) -> Result<Metrics, Error> {
        let mut args = vec!["top".to_string(), "pods".to_string(), "--no-headers".to_string()];
        match namespace {
            Some(ns) => args.extend(["-n".to_string(), ns.to_string()]),
            None => args.push("--all-namespaces".to_string()),
        }
        let text = self.run(&args.iter().map(String::as_str).collect::<Vec<_>>())?;
        Ok(tables::parse_usage(&text, namespace.is_none()))
    }

    /// What the nodes are using.
    pub fn top_nodes(&self) -> Result<Metrics, Error> {
        let text = self.run(&["top", "nodes", "--no-headers"])?;
        Ok(tables::parse_usage(&text, false))
    }

    /// Run the listing and build its table. `now` is the time in seconds
    /// since the epoch, for the age column. `metrics` is what `kubectl
    /// top` last said, which the kinds that have usage columns show.
    pub fn list(&self, listing: &Listing, now: i64, metrics: &Metrics) -> Result<Table, Error> {
        if let Some(pod) = &listing.containers_of {
            let mut args = vec!["get", "pod", pod.as_str(), "-o", "json"];
            if let Some(ns) = &listing.namespace {
                args.extend(["-n", ns.as_str()]);
            }
            let json: serde_json::Value = parse(&self.run(&args)?)?;
            return Ok(tables::containers(&json, now));
        }
        let mut args: Vec<String> = vec!["get".into(), listing.kind.clone(), "-o".into(), "json".into()];
        let namespaced = is_namespaced(&listing.kind);
        match (&listing.namespace, namespaced) {
            (Some(ns), true) => args.extend(["-n".to_string(), ns.clone()]),
            (None, true) => args.push("--all-namespaces".to_string()),
            (_, false) => {}
        }
        if let Some(sel) = &listing.selector {
            args.extend(["-l".to_string(), sel.clone()]);
        }
        if let Some(f) = &listing.field_selector {
            args.extend(["--field-selector".to_string(), f.clone()]);
        }
        let borrowed: Vec<&str> = args.iter().map(String::as_str).collect();
        let json: serde_json::Value = parse(&self.run(&borrowed)?)?;
        let items = json["items"].as_array().cloned().unwrap_or_default();
        let across = namespaced && listing.namespace.is_none();
        Ok(tables::build(&listing.kind, &items, now, across, metrics))
    }
}

/// The kinds a cluster serves and the ones Qell knows, for completing
/// what you type after `:`. Matching ignores case; a short name brings
/// in its full kind, so `svc` offers `services`.
pub fn complete(prefix: &str, cluster_kinds: &[String]) -> Vec<String> {
    let prefix = prefix.trim().to_lowercase();
    let mut out: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if !out.iter().any(|k| k == name) {
            out.push(name.to_string());
        }
    };
    // What Qell knows comes first: those are the kinds with real columns.
    for kind in KINDS {
        if kind.name.starts_with(&prefix) || kind.aliases.iter().any(|a| a.starts_with(&prefix)) {
            add(kind.name);
        }
    }
    let mut rest: Vec<&String> = cluster_kinds.iter().filter(|k| k.to_lowercase().starts_with(&prefix)).collect();
    rest.sort();
    for kind in rest {
        add(kind);
    }
    out.truncate(12);
    out
}

/// What the header shows about the cluster itself. Everything here is
/// best effort: a cluster without metrics-server still gets a header.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Info {
    pub cluster: String,
    pub user: String,
    /// The API server's version, e.g. `v1.31.3`.
    pub server: String,
    /// Percentages across the nodes, when metrics-server answers.
    pub cpu: Option<u8>,
    pub mem: Option<u8>,
    /// For the `<0>`…`<9>` namespace keys.
    pub namespaces: Vec<String>,
    /// Every kind this cluster serves, CRDs included, for completion.
    pub kinds: Vec<String>,
    /// metrics-server isn't there, so don't keep asking it.
    pub no_metrics: bool,
}

impl Client {
    /// Read what the header shows. Each part is allowed to fail on its
    /// own: a cluster you can list pods on but not nodes still gets a
    /// context, a user and a version.
    pub fn info(&self, with_metrics: bool) -> Info {
        let mut info = Info::default();
        if let Ok(text) = self.run(&["config", "view", "--minify", "-o", "json"]) {
            let (cluster, user) = parse_config(&text);
            info.cluster = cluster;
            info.user = user;
        }
        // `version` exits non-zero when the cluster is out of reach, but
        // still prints the client's half, so its output is read either way.
        info.server = parse_version(&self.output(&["version", "-o", "json"]));
        if let Ok(text) = self.run(&["get", "namespaces", "-o", "json"]) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                info.namespaces = json["items"]
                    .as_array()
                    .map(|items| items.iter().filter_map(|i| i["metadata"]["name"].as_str().map(str::to_string)).collect())
                    .unwrap_or_default();
                info.namespaces.sort();
            }
        }
        // `api-resources` talks to the API server's discovery endpoint,
        // so a cluster with CRDs completes its own kinds too.
        if let Ok(text) = self.run(&["api-resources", "-o", "name"]) {
            info.kinds = text.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
            // `events.events.k8s.io` also completes as `events`.
            let bare: Vec<String> =
                info.kinds.iter().filter_map(|k| k.split_once('.')).map(|(head, _)| head.to_string()).collect();
            for name in bare {
                if !info.kinds.contains(&name) {
                    info.kinds.push(name);
                }
            }
            info.kinds.sort();
            info.kinds.dedup();
        }
        if with_metrics {
            match self.run(&["top", "nodes", "--no-headers"]) {
                Ok(text) => {
                    let (cpu, mem) = parse_top(&text);
                    info.cpu = cpu;
                    info.mem = mem;
                    info.no_metrics = cpu.is_none();
                }
                // Without metrics-server this always fails; asking again
                // every few seconds would be pure noise.
                Err(_) => info.no_metrics = true,
            }
        }
        info
    }

    /// Run and take stdout whatever the exit status, for commands that
    /// half-answer (`kubectl version` without a cluster).
    fn output(&self, args: &[&str]) -> String {
        match self.run(args) {
            Ok(text) => text,
            Err(Error::Failed(_)) | Err(Error::TimedOut) => String::new(),
            Err(Error::NotInstalled) => String::new(),
        }
    }
}

/// The cluster and user of the current context.
pub fn parse_config(json: &str) -> (String, String) {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return (String::new(), String::new()) };
    let context = &v["contexts"][0]["context"];
    let text = |key: &str| context[key].as_str().unwrap_or("").to_string();
    (text("cluster"), text("user"))
}

/// The server's version from `kubectl version -o json`, or the client's
/// when there is no server to ask.
pub fn parse_version(json: &str) -> String {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(json) else { return String::new() };
    v["serverVersion"]["gitVersion"].as_str().unwrap_or("").to_string()
}

/// The CPU and memory percentages of `kubectl top nodes --no-headers`,
/// averaged over the nodes.
pub fn parse_top(text: &str) -> (Option<u8>, Option<u8>) {
    let mut cpu: Vec<u32> = Vec::new();
    let mut mem: Vec<u32> = Vec::new();
    for line in text.lines() {
        let fields: Vec<&str> = line.split_whitespace().collect();
        // NAME  CPU(cores)  CPU%  MEMORY(bytes)  MEMORY%
        if fields.len() < 5 {
            continue;
        }
        let percent = |f: &str| f.strip_suffix('%').and_then(|n| n.parse::<u32>().ok());
        if let (Some(c), Some(m)) = (percent(fields[2]), percent(fields[4])) {
            cpu.push(c);
            mem.push(m);
        }
    }
    let average = |v: Vec<u32>| (!v.is_empty()).then(|| (v.iter().sum::<u32>() / v.len() as u32).min(100) as u8);
    (average(cpu), average(mem))
}

fn parse(text: &str) -> Result<serde_json::Value, Error> {
    serde_json::from_str(text).map_err(|e| Error::Failed(format!("kubectl's answer wasn't JSON: {e}")))
}

/// kubectl's errors are readable already; this only drops the noise.
fn tidy_error(stderr: &str) -> String {
    let line = stderr
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with("Unable to connect to the server: dial tcp"))
        .unwrap_or_else(|| stderr.trim());
    let line = line.strip_prefix("error: ").unwrap_or(line);
    if line.is_empty() {
        "kubectl failed without saying why.".to_string()
    } else {
        line.to_string()
    }
}

/// Where a `kubectl` is, for the status line. `None` when it isn't on the
/// PATH: the view says so instead of pretending the cluster is empty.
pub fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(bin)).find(|p| p.is_file())
}

/// Seconds since the epoch, for the age column.
pub fn now() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_header_reads_the_current_context() {
        let json = r#"{"contexts":[{"name":"prod","context":{"cluster":"prod-eks","user":"nico","namespace":"web"}}]}"#;
        assert_eq!(parse_config(json), ("prod-eks".to_string(), "nico".to_string()));
        assert_eq!(parse_config("not json"), (String::new(), String::new()));
    }

    #[test]
    fn the_server_version_is_the_one_shown() {
        let json = r#"{"clientVersion":{"gitVersion":"v1.31.3"},"serverVersion":{"gitVersion":"v1.30.6-eks-1234"}}"#;
        assert_eq!(parse_version(json), "v1.30.6-eks-1234");
        // No cluster to ask: nothing rather than the client's version.
        assert_eq!(parse_version(r#"{"clientVersion":{"gitVersion":"v1.31.3"}}"#), "");
    }

    #[test]
    fn metrics_average_over_the_nodes() {
        let top = "ip-10-0-1-5   250m   10%   1234Mi   30%\nip-10-0-2-8   500m   20%   2468Mi   50%\n";
        assert_eq!(parse_top(top), (Some(15), Some(40)));
        // metrics-server missing: kubectl prints nothing useful.
        assert_eq!(parse_top("error: Metrics API not available"), (None, None));
    }

    #[test]
    fn completion_offers_known_kinds_first_then_the_clusters_own() {
        let cluster = vec!["pods".to_string(), "poddisruptionbudgets".to_string(), "widgets.example.com".to_string()];
        assert_eq!(complete("po", &cluster), vec!["pods", "poddisruptionbudgets"]);
        // A short name brings in the kind it stands for.
        assert_eq!(complete("svc", &cluster), vec!["services"]);
        assert_eq!(complete("wid", &cluster), vec!["widgets.example.com"]);
        assert!(complete("zzz", &cluster).is_empty());
        // Everything matches an empty prefix, but not endlessly.
        assert_eq!(complete("", &cluster).len(), 12);
    }

    #[test]
    fn an_object_is_named_the_way_kubectl_writes_it() {
        let pod = Object { kind: "pods".into(), name: "api-0".into(), namespace: Some("web".into()) };
        assert_eq!(pod.slash(), "pod/api-0");
        assert_eq!(pod.args(), ["pods", "api-0", "-n", "web"]);
        let node = Object { kind: "nodes".into(), name: "ip-10".into(), namespace: None };
        assert_eq!(node.slash(), "node/ip-10");
        assert_eq!(node.args(), ["nodes", "ip-10"]);
        let ingress = Object { kind: "ingresses".into(), name: "web".into(), namespace: None };
        assert_eq!(ingress.slash(), "ingress/web");
    }

    #[test]
    fn quantities_read_both_ways_kubernetes_writes_them() {
        assert_eq!(tables::cpu_quantity("250m"), Some(250));
        assert_eq!(tables::cpu_quantity("2"), Some(2000));
        assert_eq!(tables::cpu_quantity("1500000n"), Some(1));
        assert_eq!(tables::mem_quantity("128Mi"), Some(128 * 1024 * 1024));
        assert_eq!(tables::mem_quantity("1Gi"), Some(1024 * 1024 * 1024));
        assert_eq!(tables::mem_quantity("500M"), Some(500_000_000));
        assert_eq!(tables::mem_quantity("1024"), Some(1024));
        assert_eq!(tables::mem_quantity("what"), None);
    }

    #[test]
    fn top_output_becomes_usage_by_name() {
        // Pods in one namespace: NAME CPU MEMORY.
        let one = tables::parse_usage("api-0   5m   120Mi\nweb-1   1   2Gi\n", false);
        assert_eq!(one["api-0"], tables::Usage { cpu_milli: 5, mem_bytes: 120 * 1024 * 1024 });
        assert_eq!(one["web-1"].cpu_milli, 1000);
        // Across namespaces, the namespace comes first and joins the key.
        let all = tables::parse_usage("web   api-0   5m   120Mi\n", true);
        assert!(all.contains_key("web/api-0"));
        // Nodes also print percentages, which are skipped.
        let nodes = tables::parse_usage("ip-10   520m   26%   5321Mi   41%\n", false);
        assert_eq!(nodes["ip-10"], tables::Usage { cpu_milli: 520, mem_bytes: 5321 * 1024 * 1024 });
    }

    #[test]
    fn short_names_resolve() {
        assert_eq!(resolve("po"), "pods");
        assert_eq!(resolve("SVC"), "services");
        assert_eq!(resolve(" deploy "), "deployments");
        // Anything else is handed to kubectl as typed.
        assert_eq!(resolve("widgets.example.com"), "widgets.example.com");
    }

    #[test]
    fn cluster_wide_kinds_are_known() {
        assert!(!is_namespaced("nodes"));
        assert!(!is_namespaced("namespaces"));
        assert!(is_namespaced("pods"));
        assert!(is_namespaced("widgets"));
    }

    #[test]
    fn a_listing_says_what_it_is() {
        let mut l = Listing::new("pods", Some("web".into()));
        assert_eq!(l.title(), "pods");
        l.selector = Some("app=api".into());
        assert_eq!(l.title(), "pods  -l app=api");
        l.containers_of = Some("api-0".into());
        assert_eq!(l.title(), "containers of api-0");
    }

    #[test]
    fn a_missing_kubectl_is_its_own_error() {
        let client = Client::new("kubectl-that-does-not-exist");
        assert_eq!(client.run(&["version"]), Err(Error::NotInstalled));
        assert!(Error::NotInstalled.to_string().contains("install"));
    }

    #[test]
    fn errors_keep_the_line_that_matters() {
        assert_eq!(
            tidy_error("error: the server doesn't have a resource type \"widgets\"\n"),
            "the server doesn't have a resource type \"widgets\""
        );
        assert_eq!(tidy_error("   \n"), "kubectl failed without saying why.");
    }

    #[test]
    fn a_failing_command_keeps_what_it_said() {
        // `false` exists everywhere and says nothing, which is the awkward case.
        let client = Client::new("false");
        assert_eq!(client.run(&["anything"]), Err(Error::Failed("kubectl failed without saying why.".into())));
    }
}
