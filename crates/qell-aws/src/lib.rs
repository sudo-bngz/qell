//! Reading an AWS account for Qell's AWS view.
//!
//! Everything goes through the `aws` CLI, for the reason the cluster view
//! goes through kubectl: it already knows your profiles, your SSO
//! sessions and your assumed roles, so Qell holds no credentials and
//! signs nothing itself. Nothing is installed and nothing runs on its
//! own; the view asks, this crate runs one command and reads the answer.
//!
//! AWS has no one verb for everything the way Kubernetes has, so each
//! kind of thing the view lists is spelled out here: the command, where
//! its items are in the answer, and the columns worth a line.

pub mod tables;

use std::process::{Command, Stdio};
use std::time::Duration;

pub use qell_table::{Health, Row, Table};
use serde_json::Value;

/// The CLI is slow to start and some calls page for a while.
const TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// `aws` isn't on the PATH.
    NotInstalled,
    /// It ran and said no (expired session, no permission, wrong region).
    Failed(String),
    TimedOut,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::NotInstalled => write!(
                f,
                "The aws CLI isn't installed, and Qell reads your account through it. Install it the way you install \
                 your other tools (brew install awscli, or AWS's own installer), then press r. Qell never installs \
                 anything for you."
            ),
            Error::TimedOut => write!(f, "aws didn't answer within {}s.", TIMEOUT.as_secs()),
            Error::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// What the view is listing, and how it got there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listing {
    Functions,
    Distributions,
    /// The invalidations of one distribution.
    Invalidations {
        distribution: String,
    },
    /// CodeDeploy applications.
    Applications,
    /// The latest deployments of one application.
    Deployments {
        application: String,
    },
    Buckets,
    /// One level of a bucket: its folders and the objects beside them.
    Objects {
        bucket: String,
        prefix: String,
    },
    Instances,
    Pipelines,
    /// Where one pipeline stands: its stages and their actions.
    PipelineState {
        pipeline: String,
    },
    BuildProjects,
    /// The latest builds of one CodeBuild project.
    Builds {
        project: String,
    },
    Roles,
    /// The policies attached to one role.
    RolePolicies {
        role: String,
    },
    Users,
    /// ECS clusters.
    Clusters,
    Services {
        cluster: String,
    },
    Tasks {
        cluster: String,
        service: String,
    },
    /// RDS instances, Aurora's included.
    Databases,
    /// Aurora clusters.
    DbClusters,
    /// Parameter Store: names and types, never values.
    Parameters,
    StateMachines,
    /// The latest executions of one state machine.
    Executions {
        machine: String,
        name: String,
    },
    Queues,
    UserPools,
    /// The app clients of one Cognito user pool.
    PoolClients {
        pool: String,
    },
    /// REST, HTTP and WebSocket APIs together.
    Apis,
    Stages {
        api: String,
        v2: bool,
    },
    /// The profiles in `~/.aws/config`: Qell's own table, no call made.
    Profiles,
    /// The regions to pick from, likewise.
    Regions,
}

impl Listing {
    /// The name on the table's frame.
    pub fn title(&self) -> String {
        match self {
            Listing::Functions => "Lambda".into(),
            Listing::Distributions => "CloudFront".into(),
            Listing::Invalidations { distribution } => format!("Invalidations({distribution})"),
            Listing::Applications => "CodeDeploy".into(),
            Listing::Deployments { application } => format!("Deployments({application})"),
            Listing::Buckets => "S3".into(),
            Listing::Objects { bucket, prefix } => format!("s3://{bucket}/{prefix}"),
            Listing::Instances => "EC2".into(),
            Listing::Pipelines => "CodePipeline".into(),
            Listing::PipelineState { pipeline } => format!("Pipeline({pipeline})"),
            Listing::BuildProjects => "CodeBuild".into(),
            Listing::Builds { project } => format!("Builds({project})"),
            Listing::Roles => "IAM roles".into(),
            Listing::RolePolicies { role } => format!("Policies({role})"),
            Listing::Users => "IAM users".into(),
            Listing::Clusters => "ECS".into(),
            Listing::Services { cluster } => format!("Services({cluster})"),
            Listing::Tasks { service, .. } => format!("Tasks({service})"),
            Listing::Databases => "RDS".into(),
            Listing::DbClusters => "Aurora".into(),
            Listing::Parameters => "Parameters".into(),
            Listing::StateMachines => "Step Functions".into(),
            Listing::Executions { name, .. } => format!("Executions({name})"),
            Listing::Queues => "SQS".into(),
            Listing::UserPools => "Cognito".into(),
            Listing::PoolClients { pool } => format!("Clients({pool})"),
            Listing::Apis => "API Gateway".into(),
            Listing::Stages { api, .. } => format!("Stages({api})"),
            Listing::Profiles => "Profiles".into(),
            Listing::Regions => "Regions".into(),
        }
    }

    /// The two tables that pick where to look rather than look.
    pub fn is_picker(&self) -> bool {
        matches!(self, Listing::Profiles | Listing::Regions)
    }

    /// Whether the answer is the same in every region. CloudFront and the
    /// list of buckets are account-wide, and the header says so.
    pub fn global(&self) -> bool {
        matches!(
            self,
            Listing::Distributions
                | Listing::Invalidations { .. }
                | Listing::Buckets
                | Listing::Objects { .. }
                | Listing::Roles
                | Listing::RolePolicies { .. }
                | Listing::Users
        )
    }

    /// What Enter on `row` leads to, if anything.
    pub fn enter(&self, row: &Row) -> Option<Listing> {
        match self {
            Listing::Distributions => Some(Listing::Invalidations { distribution: row.name.clone() }),
            Listing::Applications => Some(Listing::Deployments { application: row.name.clone() }),
            Listing::Buckets => Some(Listing::Objects { bucket: row.name.clone(), prefix: String::new() }),
            Listing::Pipelines => Some(Listing::PipelineState { pipeline: row.name.clone() }),
            Listing::BuildProjects => Some(Listing::Builds { project: row.name.clone() }),
            Listing::Roles => Some(Listing::RolePolicies { role: row.name.clone() }),
            Listing::Clusters => Some(Listing::Services { cluster: row.name.clone() }),
            Listing::Services { cluster } => Some(Listing::Tasks { cluster: cluster.clone(), service: row.name.clone() }),
            Listing::StateMachines => {
                Some(Listing::Executions { machine: row.object["stateMachineArn"].as_str()?.to_string(), name: row.name.clone() })
            }
            Listing::UserPools => Some(Listing::PoolClients { pool: row.name.clone() }),
            Listing::Apis => Some(Listing::Stages { api: row.name.clone(), v2: row.object["ApiId"].is_string() }),
            // A folder goes one level down; an object is the end of it.
            Listing::Objects { bucket, .. } if row.name.ends_with('/') => {
                Some(Listing::Objects { bucket: bucket.clone(), prefix: row.name.clone() })
            }
            _ => None,
        }
    }

    /// What follows `aws logs` to follow a row's logs, when it has some:
    /// a function's own group, or the stream of one build.
    pub fn logs(&self, row: &Row) -> Option<Vec<String>> {
        let tail = |group: &str, rest: &[&str]| {
            let mut args = vec!["logs".to_string(), "tail".to_string(), group.to_string()];
            args.extend(rest.iter().map(|s| s.to_string()));
            args.extend(["--follow", "--format", "short"].map(str::to_string));
            args
        };
        match self {
            Listing::Functions => {
                let group = row.object["LoggingConfig"]["LogGroup"]
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("/aws/lambda/{}", row.name));
                Some(tail(&group, &["--since", "10m"]))
            }
            Listing::Builds { .. } => {
                let logs = &row.object["logs"];
                let (group, stream) = (logs["groupName"].as_str()?, logs["streamName"].as_str()?);
                // The whole build, then whatever it still says.
                Some(tail(group, &["--log-stream-names", stream, "--since", "7d"]))
            }
            _ => None,
        }
    }

    /// The `aws` arguments that open a shell on a row: Session Manager
    /// on an instance, ECS Exec in a task. `Err` says why not this one.
    pub fn shell(&self, row: &Row) -> Option<Result<Vec<String>, String>> {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        match self {
            Listing::Instances => Some(match row.object["State"]["Name"].as_str().unwrap_or("") {
                "running" => Ok(words(&["ssm", "start-session", "--target", &row.name])),
                state => Err(format!("{} is {state}; a session needs it running.", row.name)),
            }),
            Listing::Tasks { cluster, .. } => Some(match row.object["lastStatus"].as_str().unwrap_or("") {
                "RUNNING" => {
                    let mut args = words(&["ecs", "execute-command", "--cluster", cluster, "--task", &row.name]);
                    // With several containers the CLI wants one named.
                    if let Some(container) = row.object["containers"][0]["name"].as_str() {
                        args.extend(words(&["--container", container]));
                    }
                    args.extend(words(&["--interactive", "--command", "/bin/sh"]));
                    Ok(args)
                }
                state => Err(format!("{} is {state}; a shell needs it running.", row.name)),
            }),
            _ => None,
        }
    }
}

/// The kinds `:` switches between, with the short names people type.
pub const KINDS: &[(&str, &[&str])] = &[
    ("lambda", &["fn", "functions", "lambdas"]),
    ("cloudfront", &["cf", "cdn", "distributions"]),
    ("codedeploy", &["cd", "deploy", "deployments"]),
    ("codepipeline", &["cp", "pipelines", "pipeline"]),
    ("codebuild", &["cb", "builds", "build"]),
    ("s3", &["buckets", "bucket"]),
    ("ec2", &["instances", "vm"]),
    ("ecs", &["containers", "fargate"]),
    ("rds", &["db", "databases"]),
    ("aurora", &["dbclusters"]),
    ("iam", &["roles", "role"]),
    ("users", &["iam-users"]),
    ("ssm", &["params", "parameters"]),
    ("stepfunctions", &["sfn", "states", "machines"]),
    ("sqs", &["queues", "queue"]),
    ("cognito", &["pools", "userpools"]),
    ("apigateway", &["api", "apis", "apigw"]),
];

/// The listing a typed word means, if it means one.
pub fn resolve(word: &str) -> Option<Listing> {
    let word = word.trim().to_lowercase();
    let (name, _) = KINDS.iter().find(|(name, aliases)| *name == word || aliases.contains(&word.as_str()))?;
    Some(match *name {
        "lambda" => Listing::Functions,
        "cloudfront" => Listing::Distributions,
        "codedeploy" => Listing::Applications,
        "codepipeline" => Listing::Pipelines,
        "codebuild" => Listing::BuildProjects,
        "s3" => Listing::Buckets,
        "ecs" => Listing::Clusters,
        "rds" => Listing::Databases,
        "aurora" => Listing::DbClusters,
        "iam" => Listing::Roles,
        "users" => Listing::Users,
        "ssm" => Listing::Parameters,
        "stepfunctions" => Listing::StateMachines,
        "sqs" => Listing::Queues,
        "cognito" => Listing::UserPools,
        "apigateway" => Listing::Apis,
        _ => Listing::Instances,
    })
}

/// What a half-typed word after `:` could become.
pub fn complete(prefix: &str) -> Vec<String> {
    let prefix = prefix.trim().to_lowercase();
    let mut out: Vec<String> = KINDS
        .iter()
        .filter(|(name, aliases)| name.starts_with(&prefix) || aliases.iter().any(|a| a.starts_with(&prefix)))
        .map(|(name, _)| name.to_string())
        .collect();
    // A short name typed whole means its own service, whatever else
    // starts the same way: `cd` is CodeDeploy, not CloudFront.
    if let Some((exact, _)) = KINDS.iter().find(|(name, aliases)| *name == prefix || aliases.contains(&prefix.as_str())) {
        out.retain(|name| name != exact);
        out.insert(0, exact.to_string());
    }
    out
}

/// The regions people are usually in, and where each one is. A region
/// that isn't here (a new one, an opt-in one) is typed after `:region`.
pub const REGIONS: &[(&str, &str)] = &[
    ("eu-west-1", "Ireland"),
    ("eu-west-3", "Paris"),
    ("eu-central-1", "Frankfurt"),
    ("us-east-1", "N. Virginia"),
    ("us-east-2", "Ohio"),
    ("us-west-2", "Oregon"),
    ("eu-west-2", "London"),
    ("eu-north-1", "Stockholm"),
    ("ap-southeast-1", "Singapore"),
    ("us-west-1", "N. California"),
    ("eu-south-1", "Milan"),
    ("eu-south-2", "Spain"),
    ("eu-central-2", "Zurich"),
    ("ca-central-1", "Canada"),
    ("sa-east-1", "São Paulo"),
    ("ap-northeast-1", "Tokyo"),
    ("ap-northeast-2", "Seoul"),
    ("ap-northeast-3", "Osaka"),
    ("ap-southeast-2", "Sydney"),
    ("ap-south-1", "Mumbai"),
    ("ap-east-1", "Hong Kong"),
    ("me-south-1", "Bahrain"),
    ("me-central-1", "UAE"),
    ("af-south-1", "Cape Town"),
    ("il-central-1", "Tel Aviv"),
];

/// The regions the header offers on `<1>`…`<9>`, the one the profile is
/// set to first. Any other is typed after `:region`.
pub fn regions(configured: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = configured.into_iter().map(str::to_string).collect();
    out.extend(REGIONS.iter().map(|(r, _)| *r).filter(|r| Some(*r) != configured).map(str::to_string));
    out.truncate(9);
    out
}

/// Who the CLI is signed in as.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Identity {
    pub account: String,
    /// `assumed-role/Admin/nico`, the readable end of the ARN.
    pub who: String,
}

/// Runs `aws`. The binary is configurable for people who wrap it
/// (aws-vault, granted).
#[derive(Debug, Clone)]
pub struct Client {
    pub bin: String,
    /// `None` uses whatever the environment says, as the CLI would.
    pub profile: Option<String>,
    pub region: Option<String>,
}

impl Default for Client {
    fn default() -> Self {
        Self { bin: "aws".to_string(), profile: None, region: None }
    }
}

impl Client {
    /// The arguments every call carries: which account, where, as JSON.
    pub fn base_args(&self) -> Vec<String> {
        let mut args = Vec::new();
        if let Some(p) = &self.profile {
            args.extend(["--profile".to_string(), p.clone()]);
        }
        if let Some(r) = &self.region {
            args.extend(["--region".to_string(), r.clone()]);
        }
        args
    }

    /// Run aws with these arguments and give back its stdout.
    pub fn run(&self, args: &[String]) -> Result<String, Error> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(self.base_args()).args(args).args(["--output", "json"]);
        // No pager and no colour codes: the answer is read, not shown.
        cmd.env("AWS_PAGER", "").env("NO_COLOR", "1");
        cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::NotInstalled),
            Err(e) => return Err(Error::Failed(e.to_string())),
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
        std::thread::Builder::new()
            .name("aws".into())
            .spawn(move || {
                use std::io::Read;
                let (mut out, mut err) = (String::new(), String::new());
                if let Some(mut s) = stdout {
                    let _ = s.read_to_string(&mut out);
                }
                if let Some(mut s) = stderr {
                    let _ = s.read_to_string(&mut err);
                }
                let _ = tx.send((out, err));
            })
            .map_err(|e| Error::Failed(e.to_string()))?;
        let Ok((out, err)) = rx.recv_timeout(TIMEOUT) else {
            let _ = child.kill();
            return Err(Error::TimedOut);
        };
        match child.wait() {
            Ok(status) if status.success() => Ok(out),
            Ok(_) => Err(Error::Failed(tidy_error(&err, self.profile.as_deref()))),
            Err(e) => Err(Error::Failed(e.to_string())),
        }
    }

    fn json(&self, args: &[String]) -> Result<Value, Error> {
        let text = self.run(args)?;
        // An empty answer is how the CLI says "nothing" for some calls.
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| Error::Failed(format!("aws's answer wasn't JSON: {e}")))
    }

    /// The profiles in `~/.aws/config`.
    pub fn profiles(&self) -> Result<Vec<String>, Error> {
        let plain = Client { bin: self.bin.clone(), profile: None, region: None };
        let mut cmd = Command::new(&plain.bin);
        cmd.args(["configure", "list-profiles"]).stdin(Stdio::null()).stderr(Stdio::null());
        let out = match cmd.output() {
            Ok(o) => o,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err(Error::NotInstalled),
            Err(e) => return Err(Error::Failed(e.to_string())),
        };
        let mut list: Vec<String> =
            String::from_utf8_lossy(&out.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
        list.sort();
        Ok(list)
    }

    /// The region the profile is set to, when it is set to one.
    pub fn configured_region(&self) -> Option<String> {
        let mut cmd = Command::new(&self.bin);
        if let Some(p) = &self.profile {
            cmd.args(["--profile", p]);
        }
        let out = cmd.args(["configure", "get", "region"]).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        Some(String::from_utf8_lossy(&out.stdout).trim().to_string()).filter(|r| !r.is_empty())
    }

    /// Who this is, which is also the cheapest way to learn that the
    /// session has expired.
    pub fn identity(&self) -> Result<Identity, Error> {
        let v = self.json(&["sts".to_string(), "get-caller-identity".to_string()])?;
        Ok(tables::identity(&v))
    }

    /// Run `aws` with these words and read its JSON.
    fn call(&self, words: &[&str]) -> Result<Value, Error> {
        self.json(&words.iter().map(|w| w.to_string()).collect::<Vec<_>>())
    }

    /// The same call for each chunk of `ids` (the APIs take a few at a
    /// time), with the arrays under `key` joined into one answer.
    fn in_chunks(&self, words: &[&str], ids: &[String], size: usize, key: &str) -> Result<Value, Error> {
        let mut all = Vec::new();
        for chunk in ids.chunks(size) {
            let mut args: Vec<String> = words.iter().map(|w| w.to_string()).collect();
            args.extend(chunk.iter().cloned());
            if let Value::Array(items) = self.json(&args)?[key].take() {
                all.extend(items);
            }
        }
        Ok(serde_json::json!({ key: all }))
    }

    /// Run the listing and build its table.
    pub fn list(&self, listing: &Listing, now: i64) -> Result<Table, Error> {
        match listing {
            Listing::Profiles => return Ok(tables::profiles(&self.profiles()?, self.profile.as_deref())),
            Listing::Regions => return Ok(tables::regions(self.region.as_deref())),
            _ => {}
        }
        Ok(tables::build(listing, &self.answer(listing)?, now))
    }

    /// What the service says about a listing. Most are one call; the
    /// services that list ids and describe them separately take two.
    fn answer(&self, listing: &Listing) -> Result<Value, Error> {
        let strings = |v: &Value| -> Vec<String> {
            v.as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_string)).collect()).unwrap_or_default()
        };
        match listing {
            Listing::Functions => self.call(&["lambda", "list-functions"]),
            Listing::Distributions => self.call(&["cloudfront", "list-distributions"]),
            Listing::Invalidations { distribution } => {
                self.call(&["cloudfront", "list-invalidations", "--distribution-id", distribution, "--max-items", "50"])
            }
            Listing::Applications => self.call(&["deploy", "list-applications"]),
            Listing::Deployments { application } => {
                let ids = strings(
                    &self.call(&["deploy", "list-deployments", "--application-name", application, "--max-items", "25"])?
                        ["deployments"],
                );
                self.in_chunks(&["deploy", "batch-get-deployments", "--deployment-ids"], &ids, 25, "deploymentsInfo")
            }
            Listing::Buckets => self.call(&["s3api", "list-buckets"]),
            Listing::Objects { bucket, prefix } => self.call(&[
                "s3api",
                "list-objects-v2",
                "--bucket",
                bucket,
                "--delimiter",
                "/",
                "--prefix",
                prefix,
                "--max-items",
                "1000",
            ]),
            Listing::Instances => self.call(&["ec2", "describe-instances"]),
            Listing::Pipelines => self.call(&["codepipeline", "list-pipelines"]),
            Listing::PipelineState { pipeline } => self.call(&["codepipeline", "get-pipeline-state", "--name", pipeline]),
            Listing::BuildProjects => self.call(&["codebuild", "list-projects"]),
            Listing::Builds { project } => {
                let ids = strings(
                    &self.call(&["codebuild", "list-builds-for-project", "--project-name", project, "--max-items", "25"])?["ids"],
                );
                self.in_chunks(&["codebuild", "batch-get-builds", "--ids"], &ids, 25, "builds")
            }
            Listing::Roles => self.call(&["iam", "list-roles"]),
            Listing::RolePolicies { role } => self.call(&["iam", "list-attached-role-policies", "--role-name", role]),
            Listing::Users => self.call(&["iam", "list-users"]),
            Listing::Clusters => {
                let arns = strings(&self.call(&["ecs", "list-clusters"])?["clusterArns"]);
                self.in_chunks(&["ecs", "describe-clusters", "--clusters"], &arns, 100, "clusters")
            }
            Listing::Services { cluster } => {
                let arns = strings(&self.call(&["ecs", "list-services", "--cluster", cluster])?["serviceArns"]);
                self.in_chunks(&["ecs", "describe-services", "--cluster", cluster, "--services"], &arns, 10, "services")
            }
            Listing::Tasks { cluster, service } => {
                let arns =
                    strings(&self.call(&["ecs", "list-tasks", "--cluster", cluster, "--service-name", service])?["taskArns"]);
                self.in_chunks(&["ecs", "describe-tasks", "--cluster", cluster, "--tasks"], &arns, 100, "tasks")
            }
            Listing::Databases => self.call(&["rds", "describe-db-instances"]),
            Listing::DbClusters => self.call(&["rds", "describe-db-clusters"]),
            Listing::Parameters => self.call(&["ssm", "describe-parameters"]),
            Listing::StateMachines => self.call(&["stepfunctions", "list-state-machines"]),
            Listing::Executions { machine, .. } => {
                self.call(&["stepfunctions", "list-executions", "--state-machine-arn", machine, "--max-items", "25"])
            }
            Listing::Queues => {
                let urls = strings(&self.call(&["sqs", "list-queues"])?["QueueUrls"]);
                Ok(serde_json::json!({ "Queues": self.queues(&urls) }))
            }
            Listing::UserPools => self.call(&["cognito-idp", "list-user-pools", "--max-results", "60"]),
            Listing::PoolClients { pool } => {
                self.call(&["cognito-idp", "list-user-pool-clients", "--user-pool-id", pool, "--max-results", "60"])
            }
            // Two generations of the service, one list.
            Listing::Apis => {
                let rest = self.call(&["apigateway", "get-rest-apis"])?;
                let v2 = self.call(&["apigatewayv2", "get-apis"])?;
                Ok(serde_json::json!({ "rest": rest["items"], "v2": v2["Items"] }))
            }
            Listing::Stages { api, v2: false } => self.call(&["apigateway", "get-stages", "--rest-api-id", api]),
            Listing::Stages { api, v2: true } => self.call(&["apigatewayv2", "get-stages", "--api-id", api]),
            Listing::Profiles | Listing::Regions => Ok(Value::Null),
        }
    }

    /// How full each queue is. SQS only says so one queue at a time, so
    /// the questions are asked a few at once; a queue that doesn't answer
    /// keeps its line, without numbers.
    fn queues(&self, urls: &[String]) -> Vec<Value> {
        const AT_ONCE: usize = 8;
        let ask = |url: &String| {
            let attributes = self
                .call(&["sqs", "get-queue-attributes", "--queue-url", url, "--attribute-names", "All"])
                .map(|mut v| v["Attributes"].take())
                .unwrap_or(Value::Null);
            serde_json::json!({ "QueueUrl": url, "Attributes": attributes })
        };
        let mut out = Vec::with_capacity(urls.len());
        for batch in urls.chunks(AT_ONCE) {
            let answers: Vec<Value> = std::thread::scope(|scope| {
                let handles: Vec<_> = batch.iter().map(|url| scope.spawn(move || ask(url))).collect();
                handles.into_iter().map(|h| h.join().unwrap_or(Value::Null)).collect()
            });
            out.extend(answers);
        }
        out
    }
}

/// The CLI's errors lead with a blank line and the operation; this keeps
/// the sentence, and says what to do about the commonest one.
fn tidy_error(stderr: &str, profile: Option<&str>) -> String {
    let line = stderr.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("aws failed without saying why.");
    let expired = ["ExpiredToken", "expired", "SSO session", "sso login", "Unable to locate credentials"];
    if expired.iter().any(|needle| stderr.contains(needle)) {
        let login = match profile {
            Some(p) => format!("aws sso login --profile {p}"),
            None => "aws sso login".to_string(),
        };
        return format!("{line}  Sign in again in a terminal ({login}), then press r.");
    }
    line.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(name: &str) -> Row {
        Row { cells: vec![name.to_string()], name: name.to_string(), health: Health::Plain, object: Value::Null }
    }

    #[test]
    fn short_names_reach_their_service() {
        assert_eq!(resolve("fn"), Some(Listing::Functions));
        assert_eq!(resolve("CF"), Some(Listing::Distributions));
        assert_eq!(resolve(" s3 "), Some(Listing::Buckets));
        assert_eq!(resolve("ec2"), Some(Listing::Instances));
        assert_eq!(resolve("dynamodb"), None);
        assert_eq!(complete("code"), vec!["codedeploy", "codepipeline", "codebuild"]);
        assert_eq!(resolve("sfn"), Some(Listing::StateMachines));
        assert_eq!(resolve("api"), Some(Listing::Apis));
        assert_eq!(resolve("params"), Some(Listing::Parameters));
        assert_eq!(complete("buck"), vec!["s3"]);
        assert_eq!(complete("cd")[0], "codedeploy");
        assert_eq!(complete("cp")[0], "codepipeline");
        assert_eq!(complete("cf"), vec!["cloudfront"]);
    }

    #[test]
    fn enter_follows_what_things_contain() {
        assert_eq!(Listing::Distributions.enter(&row("E123")), Some(Listing::Invalidations { distribution: "E123".into() }));
        assert_eq!(Listing::Applications.enter(&row("api")), Some(Listing::Deployments { application: "api".into() }));
        let bucket = Listing::Buckets.enter(&row("assets")).expect("into the bucket");
        assert_eq!(bucket, Listing::Objects { bucket: "assets".into(), prefix: String::new() });
        // A folder goes down a level, an object goes nowhere.
        assert_eq!(bucket.enter(&row("img/")), Some(Listing::Objects { bucket: "assets".into(), prefix: "img/".into() }));
        assert_eq!(bucket.enter(&row("logo.png")), None);
        assert_eq!(Listing::Functions.enter(&row("api")), None);
    }

    #[test]
    fn every_call_says_which_account_and_where() {
        let client = Client { bin: "aws".into(), profile: Some("prod".into()), region: Some("eu-west-3".into()) };
        assert_eq!(client.base_args(), ["--profile", "prod", "--region", "eu-west-3"]);
        assert!(Client::default().base_args().is_empty());
    }

    #[test]
    fn a_lambda_logs_where_it_says_or_where_lambdas_do() {
        let args = Listing::Functions.logs(&row("api")).expect("a function has logs");
        assert_eq!(args, ["logs", "tail", "/aws/lambda/api", "--since", "10m", "--follow", "--format", "short"]);
        let mut custom = row("api");
        custom.object = serde_json::json!({ "LoggingConfig": { "LogGroup": "/custom/api" } });
        assert_eq!(Listing::Functions.logs(&custom).expect("logs")[2], "/custom/api");
        assert_eq!(Listing::Buckets.logs(&row("assets")), None);
    }

    #[test]
    fn a_build_logs_from_its_own_stream() {
        let mut build = row("web:1");
        build.object = serde_json::json!({ "logs": { "groupName": "/aws/codebuild/web", "streamName": "abc" } });
        let args = Listing::Builds { project: "web".into() }.logs(&build).expect("a build has logs");
        assert_eq!(args[..5], ["logs", "tail", "/aws/codebuild/web", "--log-stream-names", "abc"]);
        // A build that never started has nowhere to read from.
        assert_eq!(Listing::Builds { project: "web".into() }.logs(&row("web:2")), None);
    }

    #[test]
    fn a_shell_goes_through_ssm_or_ecs_exec() {
        let mut vm = row("i-0a");
        vm.object = serde_json::json!({ "State": { "Name": "running" } });
        assert_eq!(
            Listing::Instances.shell(&vm),
            Some(Ok(vec!["ssm".into(), "start-session".into(), "--target".into(), "i-0a".into()]))
        );
        vm.object = serde_json::json!({ "State": { "Name": "stopped" } });
        assert!(matches!(Listing::Instances.shell(&vm), Some(Err(why)) if why.contains("stopped")));
        let tasks = Listing::Tasks { cluster: "prod".into(), service: "api".into() };
        let mut task = row("0123abcd");
        task.object = serde_json::json!({ "lastStatus": "RUNNING", "containers": [{ "name": "app" }, { "name": "envoy" }] });
        let args = tasks.shell(&task).expect("a task takes a shell").expect("it is running");
        assert_eq!(
            args,
            [
                "ecs",
                "execute-command",
                "--cluster",
                "prod",
                "--task",
                "0123abcd",
                "--container",
                "app",
                "--interactive",
                "--command",
                "/bin/sh"
            ]
        );
        assert_eq!(Listing::Functions.shell(&row("api")), None);
    }

    #[test]
    fn the_new_services_lead_where_they_should() {
        assert_eq!(Listing::Clusters.enter(&row("prod")), Some(Listing::Services { cluster: "prod".into() }));
        assert_eq!(
            Listing::Services { cluster: "prod".into() }.enter(&row("api")),
            Some(Listing::Tasks { cluster: "prod".into(), service: "api".into() })
        );
        let mut machine = row("nightly");
        machine.object = serde_json::json!({ "stateMachineArn": "arn:aws:states:eu-west-3:1:stateMachine:nightly" });
        assert!(matches!(Listing::StateMachines.enter(&machine), Some(Listing::Executions { name, .. }) if name == "nightly"));
        let mut http = row("a1b2");
        http.object = serde_json::json!({ "ApiId": "a1b2" });
        assert_eq!(Listing::Apis.enter(&http), Some(Listing::Stages { api: "a1b2".into(), v2: true }));
        assert_eq!(Listing::Apis.enter(&row("r9")), Some(Listing::Stages { api: "r9".into(), v2: false }));
        assert!(Listing::Roles.global() && !Listing::Clusters.global());
    }

    #[test]
    fn the_profiles_own_region_comes_first() {
        let r = regions(Some("eu-west-3"));
        assert_eq!(r[0], "eu-west-3");
        assert_eq!(r.iter().filter(|x| *x == "eu-west-3").count(), 1);
        assert_eq!(r.len(), 9);
        assert_eq!(regions(None)[0], "eu-west-1");
    }

    #[test]
    fn an_expired_session_says_how_to_sign_in() {
        let said =
            tidy_error("\nAn error occurred (ExpiredToken) when calling the ListFunctions operation: expired\n", Some("prod"));
        assert!(said.contains("aws sso login --profile prod"), "{said}");
        assert_eq!(tidy_error("\nAn error occurred (AccessDenied): no\n", None), "An error occurred (AccessDenied): no");
        assert_eq!(tidy_error("", None), "aws failed without saying why.");
    }

    #[test]
    fn a_missing_cli_is_its_own_error() {
        let client = Client { bin: "aws-that-does-not-exist".into(), ..Default::default() };
        assert_eq!(client.run(&["sts".into()]), Err(Error::NotInstalled));
        assert!(Error::NotInstalled.to_string().contains("brew install awscli"));
    }
}
