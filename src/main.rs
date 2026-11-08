use clap::{ArgGroup, Args, Parser, Subcommand, ValueEnum};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, HashMap},
    env,
    ffi::OsStr,
    fmt,
    fs::{self, File, OpenOptions},
    io::{self, BufRead, BufReader, Read, Seek, SeekFrom, Write},
    net::{TcpListener, TcpStream},
    path::{Component, Path, PathBuf},
    process::{Child, Command as ProcessCommand, ExitStatus, Output, Stdio},
    str::FromStr,
    sync::OnceLock,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

const DEFAULT_HOME: &str = ".milhouse";
pub(crate) const ACTIVE_CHILD_PID_FILE: &str = "active-child.pid";
const PRINCIPLES: &str = "Principles: DRY, lean, simple, hard cuts, fail fast, no fallbacks, compile-time type safety, strict enums, no loose strings, minimal mutable state and side effects, minimal nil checks and force-unwraps, CLI-first (prove behavior via CLI before UI), changes must simplify.";
const GRADE_SCHEMA: &str = r#"{"type":"object","required":["score","citations"],"properties":{"score":{"type":"number","minimum":0,"maximum":100},"citations":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}}},"additionalProperties":false}"#;
const DASH_PORT: u16 = 8471;
const BODY_CAP: usize = 65_536;
const RUN_STORAGE_BUDGET_BYTES: u64 = 1 << 30;
const RUN_CACHE_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);
mod bench;
mod dashboard;
mod engine;
mod grade;
mod spend;

#[derive(Parser)]
#[command(name = "mil", about = "Milhouse — a contract-driven task runner")]
struct Cli {
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    Plan(PlanArgs),
    Build(BuildArgs),
    Contract(ContractArgs),
    Models,
    Config(ModelConfigArgs),
    #[command(hide = true)]
    Advise(AdviseArgs),
    #[command(hide = true)]
    Prepare(PrepareArgs),
    #[command(hide = true)]
    Execute(ExecuteArgs),
    Resume(ResumeArgs),
    Cancel(CancelArgs),
    Bench(BenchArgs),
    Grade(GradeArgs),
    Dashboard(DashboardArgs),
    Setup(SetupArgs),
    Doctor(DoctorArgs),
    Spend(SpendArgs),
    Runs(RunsArgs),
    Sanity(SanityArgs),
    #[command(hide = true)]
    Verdict(VerdictArgs),
    #[command(hide = true)]
    Verify(VerifyArgs),
    #[command(hide = true)]
    Codex(CodexArgs),
    #[command(hide = true)]
    Validate(ValidateArgs),
    #[command(hide = true)]
    Inspect(InspectArgs),
    #[command(hide = true)]
    Install,
}

#[derive(Args)]
struct RunsArgs {
    #[command(subcommand)]
    command: RunsCommand,
}

#[derive(Subcommand)]
enum RunsCommand {
    Status,
    Maintain {
        #[arg(long, value_enum)]
        pressure: StoragePressure,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum StoragePressure {
    Healthy,
    Warning,
    Critical,
}

impl StoragePressure {
    fn as_str(self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

#[derive(Args)]
struct ContractArgs {
    #[command(subcommand)]
    command: ContractCommand,
}

#[derive(Subcommand)]
enum ContractCommand {
    Validate {
        #[arg(long)]
        contract: PathBuf,
    },
    Render {
        #[arg(long)]
        contract: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

#[derive(Args)]
struct BuildArgs {
    #[arg(long)]
    workdir: PathBuf,
    #[arg(long)]
    contract: PathBuf,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long, value_name = "AGENT")]
    /// Override the saved build profile for this run
    model: Option<ModelProfile>,
}

#[derive(Args)]
struct PlanArgs {
    #[arg(long, default_value = ".")]
    workdir: PathBuf,
    #[arg(required = true)]
    task: Vec<String>,
    #[arg(long, value_name = "AGENT")]
    /// Override the saved plan profile for this session
    main: Option<ModelProfile>,
    #[arg(long, value_name = "AGENT")]
    advisor: Vec<ModelProfile>,
    #[arg(long, default_value = "contract.md")]
    contract: PathBuf,
    /// Additional binding context files supplied to the planning session
    #[arg(long = "context")]
    context: Vec<PathBuf>,
    /// Attempt a small, self-contained issue without a user conversation
    #[arg(long, requires = "issue")]
    autonomous: bool,
    /// Linear issue identifier used for logged autonomous planning
    #[arg(long, requires = "autonomous")]
    issue: Option<String>,
}

#[derive(Args)]
struct PrepareArgs {
    #[arg(long)]
    workdir: PathBuf,
    #[arg(long)]
    contract: PathBuf,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct AdviseArgs {
    #[arg(long, default_value = ".")]
    workdir: PathBuf,
    #[arg(long, value_name = "AGENT")]
    profile: ModelProfile,
    #[arg(long)]
    prompt: PathBuf,
    #[arg(long)]
    output: PathBuf,
}

#[derive(Args)]
struct ExecuteArgs {
    #[arg(long)]
    run: String,
    #[arg(long, value_name = "AGENT")]
    /// Override the prepared build profile
    model: Option<ModelProfile>,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct ModelConfigArgs {
    #[arg(long, value_name = "AGENT")]
    plan: Option<ModelProfile>,
    #[arg(long, value_name = "AGENT")]
    autoplan_review: Option<ModelProfile>,
    #[arg(long, value_name = "AGENT")]
    build: Option<ModelProfile>,
    #[arg(long, value_name = "AGENT")]
    verify: Option<ModelProfile>,
}

#[derive(Args)]
struct SetupArgs {
    #[arg(long)]
    json: bool,
    /// Env file that defines ANTHROPIC_API_KEY; stored in config.json for all Anthropic calls
    #[arg(long)]
    anthropic_key_file: Option<PathBuf>,
}

#[derive(Args)]
#[command(group(
    ArgGroup::new("resume_prompt")
        .required(false)
        .args(["prompt", "text"])
))]
struct ResumeArgs {
    run: String,
    #[arg(long)]
    prompt: Option<PathBuf>,
    #[arg(long)]
    text: Option<String>,
    #[arg(long)]
    id: Option<String>,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long, value_name = "AGENT")]
    /// Override the saved build profile for this continuation
    model: Option<ModelProfile>,
}

#[derive(Args)]
struct CodexArgs {
    #[arg(long)]
    workdir: PathBuf,
    #[arg(long)]
    prompt: PathBuf,
    #[arg(long)]
    output: PathBuf,
    #[arg(long)]
    model: Option<String>,
}

#[derive(Args)]
struct VerdictArgs {
    verdict: PathBuf,
}

#[derive(Args)]
struct VerifyArgs {
    verifier: PathBuf,
}

#[derive(Args)]
struct ValidateArgs {
    #[arg(long)]
    input: PathBuf,
}

#[derive(Args)]
struct InspectArgs {
    #[arg(long)]
    run: String,
}

#[derive(Args)]
struct CancelArgs {
    run: String,
}

#[derive(Args)]
struct GradeArgs {
    run_dir: PathBuf,
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct DoctorArgs {
    #[arg(long)]
    json: bool,
}

#[derive(Args)]
struct DashboardArgs {
    #[arg(long, default_value_t = DASH_PORT)]
    port: u16,
}

#[derive(Args)]
struct SpendArgs {
    #[command(subcommand)]
    command: SpendCommand,
}

#[derive(Subcommand)]
enum SpendCommand {
    Refresh(SpendOutputArgs),
    Status(SpendOutputArgs),
    Gate(SpendOutputArgs),
}

#[derive(Args)]
struct SpendOutputArgs {
    #[arg(long)]
    output: Option<PathBuf>,
}

#[derive(Args)]
struct BenchArgs {
    #[command(subcommand)]
    command: Option<BenchCommand>,
    #[arg(default_value = "all")]
    task: String,
    #[arg(long, default_value = "v2")]
    bank: String,
    #[arg(long, default_value = "milhouse")]
    variant: String,
    #[arg(long)]
    #[arg(long, value_name = "AGENT")]
    model: Option<ModelProfile>,
    #[arg(long)]
    cleanup: bool,
}

#[derive(Args)]
struct SanityArgs {
    #[arg(long, default_value = "mil-sanity")]
    id: String,
    #[arg(
        long,
        default_value = "# Goal\n\nWrite the words `hi` to a markdown file named `hi.md` in the workdir. Keep the change limited to that file.\n\n## Acceptance criteria\n\n- `hi.md` exists at the workdir root.\n- Its complete contents are exactly `hi` followed by one newline.\n- Verification records the command and output proving both requirements."
    )]
    text: String,
    #[arg(long)]
    output: Option<PathBuf>,
    #[arg(long, value_name = "AGENT")]
    model: Option<ModelProfile>,
    #[arg(long)]
    keep: bool,
}

#[derive(Subcommand)]
enum BenchCommand {
    Ref {
        task: String,
        #[arg(long, default_value = "v2")]
        bank: String,
    },
    Gold {
        task: String,
        #[arg(long, default_value = "v2")]
        bank: String,
        #[arg(long, default_value_t = 1)]
        runs: u32,
    },
    Calibrate,
}

#[derive(Debug, Error)]
enum AppError {
    #[error("{0}")]
    Input(String),
    #[error("{0}")]
    External(String),
    #[error("{0}")]
    Stale(String),
    #[error("{0}")]
    StorageBudget(String),
    #[error("{0}")]
    ProcessesAlive(String),
    #[error("{0}")]
    ProcessIdentityUncertain(String),
    #[error("{0}")]
    Invariant(String),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl AppError {
    fn code(&self) -> i32 {
        match self {
            AppError::Input(_) | AppError::Json(_) => 1,
            AppError::External(_)
            | AppError::Stale(_)
            | AppError::StorageBudget(_)
            | AppError::ProcessesAlive(_)
            | AppError::ProcessIdentityUncertain(_)
            | AppError::Io(_) => 2,
            AppError::Invariant(_) => 3,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct Config {
    home: PathBuf,
    grade_floor: f64,
    #[serde(alias = "daily_cost_warning_usd")]
    daily_cost_cap_usd: f64,
    lock_path: PathBuf,
    plan_profile: ModelProfile,
    autoplan_review_profile: ModelProfile,
    build_profile: ModelProfile,
    verify_profile: ModelProfile,
    anthropic_key_file: Option<PathBuf>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Model {
    #[default]
    Fable,
    Opus,
    Codex,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(transparent)]
struct ModelProfile(String);

impl ModelProfile {
    fn named(id: &str) -> Self {
        Self(id.to_string())
    }

    fn id(&self) -> &str {
        &self.0
    }

    fn details(&self) -> Result<&'static AgentProfile, AppError> {
        agent_catalog()?
            .agents
            .iter()
            .find(|agent| agent.id == self.0)
            .ok_or_else(|| AppError::Input(format!("unknown Agent Bridge profile: {}", self.0)))
    }

    fn is_codex(&self) -> Result<bool, AppError> {
        Ok(self.details()?.provider == AgentProvider::Codex)
    }
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self::named("opus-5-5-high-subscription")
    }
}

impl FromStr for ModelProfile {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let value = value.trim();
        if value.is_empty() {
            Err("agent profile cannot be empty".into())
        } else {
            Ok(Self::named(value))
        }
    }
}

impl fmt::Display for ModelProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.id())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentCatalog {
    scale: Value,
    agents: Vec<AgentProfile>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AgentProfile {
    id: String,
    provider: AgentProvider,
    model: String,
    auth: AgentAuth,
    effort: AgentEffort,
    intelligence: f64,
    taste: f64,
    cost: u8,
    #[serde(default, rename = "context")]
    _context: Option<Value>,
    #[serde(rename = "use")]
    recommended_use: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AgentProvider {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AgentAuth {
    Subscription,
    Api,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum AgentEffort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

impl AgentEffort {
    fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

static AGENT_CATALOG: OnceLock<Result<AgentCatalog, String>> = OnceLock::new();

impl Model {
    fn profile(self) -> ModelProfile {
        ModelProfile::named(match self {
            Self::Fable => "fable-5-1-high-api",
            Self::Opus => "opus-5-5-high-subscription",
            Self::Codex => "sol-6.1-high-subscription",
        })
    }

    fn details(self) -> Result<&'static AgentProfile, AppError> {
        self.profile().details()
    }

    fn id(self) -> Result<&'static str, AppError> {
        Ok(&self.details()?.model)
    }

    fn tier(self) -> &'static str {
        match self {
            Model::Fable => "fable",
            Model::Opus => "claude",
            Model::Codex => "codex",
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            home: PathBuf::from(env::var("MILHOUSE_HOME").unwrap_or_else(|_| {
                let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
                format!("{home}/{DEFAULT_HOME}")
            })),
            grade_floor: 60.0,
            daily_cost_cap_usd: 0.0,
            lock_path: PathBuf::from("/tmp/milhouse.lock"),
            plan_profile: ModelProfile::named("opus-5-5-high-subscription"),
            autoplan_review_profile: ModelProfile::named("opus-5-5-high-subscription"),
            build_profile: default_build_profile(),
            verify_profile: default_verify_profile(),
            anthropic_key_file: None,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct TaskInput {
    id: String,
    prompt: String,
    #[serde(default)]
    metadata: Value,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct RunSpec {
    id: String,
    #[serde(default)]
    short_id: Option<u64>,
    run_dir: PathBuf,
    workdir: PathBuf,
    prompt: String,
    #[serde(default = "default_build_profile")]
    build_profile: ModelProfile,
    #[serde(default = "default_verify_profile")]
    verify_profile: ModelProfile,
    #[serde(default)]
    resume_from: Option<PathBuf>,
    #[serde(default)]
    resume_mode: Option<ResumeMode>,
}

fn default_build_profile() -> ModelProfile {
    ModelProfile::named("sol-6.1-low-subscription")
}

fn default_verify_profile() -> ModelProfile {
    ModelProfile::named("sol-6.1-high-subscription")
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ResumeMode {
    #[serde(alias = "reuse_rubric")]
    ReuseContract,
}

#[derive(Debug, Serialize)]
struct RunOutput {
    id: String,
    status: String,
    run_dir: PathBuf,
    result_path: PathBuf,
    grade_score: Option<f64>,
    cost: Option<f64>,
    metadata: Value,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Verdict {
    run_id: String,
    status: VerdictStatus,
    summary: String,
    criteria_met: bool,
    contract: PathBuf,
    evidence: Vec<PathBuf>,
    note: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct VerifierVerdict {
    passed: bool,
    notes: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VerdictStatus {
    Green,
    HitRunLimit,
    NeedsUser,
    Failed,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RunStatus {
    Green,
    HitRunLimit,
    NeedsUser,
    Failed,
    FailedStale,
    FailedStorageBudget,
    Canceled,
}

impl RunStatus {
    fn as_str(self) -> &'static str {
        match self {
            RunStatus::Green => "green",
            RunStatus::HitRunLimit => "hit_run_limit",
            RunStatus::NeedsUser => "needs_user",
            RunStatus::Failed => "failed",
            RunStatus::FailedStale => "failed_stale",
            RunStatus::FailedStorageBudget => "failed_storage_budget",
            RunStatus::Canceled => "canceled",
        }
    }
}

impl From<VerdictStatus> for RunStatus {
    fn from(status: VerdictStatus) -> Self {
        match status {
            VerdictStatus::Green => RunStatus::Green,
            VerdictStatus::HitRunLimit => RunStatus::HitRunLimit,
            VerdictStatus::NeedsUser => RunStatus::NeedsUser,
            VerdictStatus::Failed => RunStatus::Failed,
        }
    }
}

#[derive(Default)]
struct RunCost {
    codex: Option<f64>,
    unknown: bool,
}

impl RunCost {
    fn total(&self) -> Option<f64> {
        self.codex
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(err) = run_cli(cli) {
        eprintln!("mil: {err}");
        std::process::exit(err.code());
    }
}

fn run_cli(cli: Cli) -> Result<(), AppError> {
    if let CommandKind::Contract(args) = &cli.command {
        return match &args.command {
            ContractCommand::Validate { contract } => {
                let text = read_contract(contract)?;
                write_out(
                    None,
                    &json!({"ok": true, "contract": contract, "bytes": text.len()}),
                )
            }
            ContractCommand::Render { contract, output } => {
                write_out(None, &render_contract_file(contract, output.as_deref())?)
            }
        };
    }
    let root = repo_root()?;
    let mut config = load_config(cli.config.as_deref())?;
    match cli.command {
        CommandKind::Build(args) => {
            let output = args.output.clone();
            let workdir = required_dir(args.workdir.clone(), "--workdir")?;
            set_build_profile(&mut config, args.model.clone())?;
            write_out(
                output,
                &engine::run_task(
                    &root,
                    &workdir,
                    &config,
                    task_from_contract(args.contract, args.id)?,
                )?,
            )
        }
        CommandKind::Plan(args) => {
            let workdir = required_dir(args.workdir.clone(), "--workdir")?;
            write_out(
                None,
                &engine::plan_contract(&root, &workdir, &config, args)?,
            )
        }
        CommandKind::Contract(_) => unreachable!(),
        CommandKind::Models => write_out(None, agent_catalog()?),
        CommandKind::Config(args) => write_out(None, &configure_models(&config, args)?),
        CommandKind::Advise(args) => {
            let workdir = required_dir(args.workdir.clone(), "--workdir")?;
            write_out(None, &engine::advise(&workdir, &config, args)?)
        }
        CommandKind::Prepare(args) => {
            let output = args.output.clone();
            let workdir = required_dir(args.workdir.clone(), "--workdir")?;
            write_out(
                output,
                &engine::prepare_task(
                    &root,
                    &workdir,
                    &config,
                    task_from_contract(args.contract, args.id)?,
                )?,
            )
        }
        CommandKind::Execute(args) => {
            let output = args.output.clone();
            set_build_profile(&mut config, args.model.clone())?;
            write_out(output, &engine::execute_run(&root, &config, &args.run)?)
        }
        CommandKind::Setup(args) => setup(&config, &args),
        CommandKind::Resume(args) => {
            let output = args.output.clone();
            set_build_profile(&mut config, args.model.clone())?;
            write_out(output, &engine::resume_task(&root, &config, args)?)
        }
        CommandKind::Codex(args) => {
            let model = args
                .model
                .map(Ok)
                .unwrap_or_else(|| Model::Codex.id().map(str::to_string))?;
            write_out(
                None,
                &engine::codex_once(
                    &required_dir(args.workdir, "--workdir")?,
                    &required_model(model)?,
                    &args.prompt,
                    &args.output,
                )?,
            )
        }
        CommandKind::Cancel(args) => write_out(None, &cancel_run(&config, &args.run)?),
        CommandKind::Verdict(args) => write_out(None, &validate_verdict_file(&args.verdict)?),
        CommandKind::Verify(args) => write_out(None, &validate_verifier_file(&args.verifier)?),
        CommandKind::Validate(args) => write_out(None, &validate_task(read_task(&args.input)?)?),
        CommandKind::Inspect(args) => write_out(None, &inspect_run(&config, &args.run)?),
        CommandKind::Grade(args) => write_out(
            args.output.clone(),
            &grade::grade_run(&config, args.run_dir)?,
        ),
        CommandKind::Doctor(args) => doctor(&root, &config, args.json),
        CommandKind::Dashboard(args) => dashboard::dashboard(&root, &config, args.port),
        CommandKind::Spend(args) => match args.command {
            SpendCommand::Refresh(out) => write_out(out.output, &spend::refresh(&config)?),
            SpendCommand::Status(out) => write_out(out.output, &spend::status(&config)?),
            SpendCommand::Gate(out) => write_out(out.output, &spend::gate(&config)?),
        },
        CommandKind::Runs(args) => write_out(
            None,
            &match args.command {
                RunsCommand::Status => maintain_runs(&config, None)?,
                RunsCommand::Maintain { pressure } => maintain_runs(&config, Some(pressure))?,
            },
        ),
        CommandKind::Bench(args) => {
            set_build_profile(&mut config, args.model.clone())?;
            bench::bench(&root, &config, args)
        }
        CommandKind::Sanity(args) => {
            set_build_profile(&mut config, args.model.clone())?;
            write_out(args.output.clone(), &engine::sanity(&root, &config, args)?)
        }
        CommandKind::Install => install(&root),
    }
}

fn repo_root() -> Result<PathBuf, AppError> {
    if let Ok(root) = env::var("MILHOUSE_REPO_ROOT") {
        return Ok(PathBuf::from(root));
    }
    let manifest_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if manifest_root.exists() {
        return Ok(manifest_root);
    }
    let exe = env::current_exe()?;
    exe.parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .ok_or_else(|| AppError::Invariant("missing repo root".into()))
}

fn load_config(path: Option<&Path>) -> Result<Config, AppError> {
    let p = path
        .map(Path::to_path_buf)
        .unwrap_or_else(|| Config::default().home.join("config.json"));
    if !p.exists() {
        return Ok(Config::default());
    }
    let mut value: Value = serde_json::from_str(&fs::read_to_string(p)?)?;
    if let Some(config) = value.as_object_mut() {
        config.remove("autopilot_enabled");
        config.remove("codex_model");
    }
    let config: Config = serde_json::from_value(value)?;
    validate_phase_profiles(&config)?;
    Ok(config)
}

fn required_dir(path: PathBuf, name: &str) -> Result<PathBuf, AppError> {
    if !path.is_dir() {
        return Err(AppError::Input(format!(
            "{name} is not a directory: {}",
            path.display()
        )));
    }
    Ok(path.canonicalize()?)
}

fn set_build_profile(config: &mut Config, requested: Option<ModelProfile>) -> Result<(), AppError> {
    if let Some(profile) = requested {
        validate_execution_profile("build", &profile)?;
        config.build_profile = profile;
    }
    Ok(())
}

fn required_model(model: String) -> Result<String, AppError> {
    let model = model.trim().to_string();
    if model.is_empty() {
        return Err(AppError::Input("--codex-model cannot be empty".into()));
    }
    Ok(model)
}

fn read_task(path: &Path) -> Result<TaskInput, AppError> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn task_from_contract(contract: PathBuf, id: Option<String>) -> Result<TaskInput, AppError> {
    let prompt = read_contract(&contract)?;
    Ok(TaskInput {
        id: id.unwrap_or_else(|| format!("task-{}", epoch())),
        prompt,
        metadata: json!({}),
    })
}

fn read_contract(path: &Path) -> Result<String, AppError> {
    let text = fs::read_to_string(path)
        .map_err(|e| AppError::Input(format!("invalid contract {}: {e}", path.display())))?;
    validate_build_contract_text(&text)?;
    Ok(text)
}

fn render_contract_file(contract: &Path, output: Option<&Path>) -> Result<Value, AppError> {
    let markdown = read_contract(contract)?;
    let output = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| contract.with_extension("html"));
    if output.exists() && output.canonicalize()? == contract.canonicalize()? {
        return Err(AppError::Input(
            "contract HTML output cannot overwrite contract.md".into(),
        ));
    }
    let source_hash = format!("{:016x}", seahash::hash(markdown.as_bytes()));
    fs::write(
        &output,
        contract_html(&markdown, &contract.display().to_string(), &source_hash),
    )?;
    fs::set_permissions(&output, fs::metadata(contract)?.permissions())?;
    Ok(json!({
        "ok": true,
        "contract": contract,
        "html": output,
        "source_hash": source_hash,
    }))
}

/// Approved stylesheet for the rendered contract (see contract-mock reference).
const CONTRACT_CSS: &str = r#"
  :root {
    --paper: #F3F6F6;
    --ink: #16242E;
    --muted: #5B707C;
    --faint: #8AA0AA;
    --hairline: #C9D5D9;
    --magenta: #B02E68;
    --magenta-soft: #B02E6818;
    --pass: #2F7D4F;
    --never: #A33B2E;
    --mono-bg: #E8EEEF;
  }
  @media (prefers-color-scheme: dark) {
    :root {
      --paper: #10181D; --ink: #D9E2E6; --muted: #8AA0AA; --faint: #5B707C;
      --hairline: #2A3942; --magenta: #E0559B; --magenta-soft: #E0559B22;
      --pass: #57B380; --never: #D4695A; --mono-bg: #1D2A33;
    }
  }
  :root[data-theme="dark"] {
    --paper: #10181D; --ink: #D9E2E6; --muted: #8AA0AA; --faint: #5B707C;
    --hairline: #2A3942; --magenta: #E0559B; --magenta-soft: #E0559B22;
    --pass: #57B380; --never: #D4695A; --mono-bg: #1D2A33;
  }
  :root[data-theme="light"] {
    --paper: #F3F6F6; --ink: #16242E; --muted: #5B707C; --faint: #8AA0AA;
    --hairline: #C9D5D9; --magenta: #B02E68; --magenta-soft: #B02E6818;
    --pass: #2F7D4F; --never: #A33B2E; --mono-bg: #E8EEEF;
  }
  html { scroll-behavior: smooth; }
  @media (prefers-reduced-motion: reduce) { html { scroll-behavior: auto; } }
  body {
    background: var(--paper); color: var(--ink);
    font-family: Charter, "Bitstream Charter", Cambria, Georgia, serif;
    font-size: 17px; line-height: 1.5; margin: 0;
  }
  .display { font-family: "Avenir Next Condensed", "Futura", "Avenir Next", "Helvetica Neue", sans-serif; }
  .mono { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-variant-numeric: tabular-nums; }

  #route-progress { position: fixed; top: 0; left: 0; height: 3px; width: 0; background: var(--magenta); z-index: 30; }

  nav.topbar {
    position: sticky; top: 0; z-index: 20;
    display: flex; align-items: baseline; gap: 1.5rem;
    padding: 0.7rem 1.25rem;
    background: color-mix(in srgb, var(--paper) 88%, transparent);
    backdrop-filter: blur(8px);
    border-bottom: 1px solid var(--hairline);
    overflow-x: auto; white-space: nowrap;
  }
  nav.topbar .brand { font-weight: 600; font-size: 0.8rem; letter-spacing: 0.14em; text-transform: uppercase; color: var(--muted); flex-shrink: 0; }
  nav.topbar a { color: var(--ink); text-decoration: none; font-size: 0.8rem; letter-spacing: 0.1em; text-transform: uppercase; }
  nav.topbar a:hover, nav.topbar a:focus-visible { color: var(--magenta); }
  nav.topbar a:focus-visible { outline: 2px solid var(--magenta); outline-offset: 3px; }

  main { max-width: 46rem; margin: 0 auto; padding: 0 1.25rem 6rem; }

  header.hero { padding: 4rem 0 2rem; }
  .eyebrow { font-size: 0.75rem; letter-spacing: 0.18em; text-transform: uppercase; color: var(--magenta); font-weight: 600; margin-bottom: 1.2rem; }
  h1.thesis {
    font-size: clamp(2rem, 5.5vw, 3.2rem); line-height: 1.06; font-weight: 600;
    letter-spacing: -0.01em; margin: 0 0 1rem; text-wrap: balance;
  }
  h1.thesis em { font-style: normal; box-shadow: inset 0 -0.45em var(--magenta-soft); }
  .meta { display: flex; flex-wrap: wrap; gap: 0.5rem 2rem; margin-top: 1.6rem; padding-top: 1rem; border-top: 1px solid var(--hairline); }
  .meta div { font-size: 0.8rem; }
  .meta .k { color: var(--muted); letter-spacing: 0.12em; text-transform: uppercase; font-size: 0.68rem; display: block; }
  .meta a { color: var(--ink); }
  a { color: var(--magenta); text-decoration-thickness: 1px; text-underline-offset: 3px; }

  section { margin-top: 2.8rem; }
  h2 {
    font-size: 0.85rem; letter-spacing: 0.16em; text-transform: uppercase; font-weight: 600;
    display: flex; align-items: center; gap: 0.75rem; margin: 0 0 1rem;
  }
  h2::after { content: ""; height: 1px; background: var(--hairline); flex: 1; }

  /* Thesis tree: L1 = parent thesis, L2 = detail, L3 = proof */
  ul.tree { list-style: none; margin: 0; padding: 0; }
  ul.tree > li { padding: 0.7rem 0; border-bottom: 1px solid var(--hairline); }
  ul.tree > li:last-child { border-bottom: none; }
  .t1 { font-size: 1.08rem; font-weight: 700; margin: 0; }
  .t2 { font-size: 0.9rem; color: var(--muted); margin: 0.25rem 0 0; max-width: 38rem; }
  .t3 {
    font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 0.75rem;
    color: var(--faint); margin: 0.35rem 0 0; overflow-x: auto; white-space: nowrap;
  }
  .t3::before { content: "proof "; color: var(--pass); letter-spacing: 0.08em; }
  .t3.expect::before { content: "expect "; color: var(--magenta); }
  .tid {
    font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 0.72rem;
    color: var(--faint); margin-right: 0.55rem;
  }

  /* L4: builder detail — collapsed at human altitude */
  details.l4 { margin-top: 0.45rem; }
  details.l4 summary {
    cursor: pointer; list-style: none; display: inline-block;
    font-size: 0.68rem; letter-spacing: 0.12em; text-transform: uppercase;
    color: var(--faint); border: 1px solid var(--hairline); border-radius: 4px;
    padding: 0.15rem 0.55rem;
    font-family: "Avenir Next Condensed", "Futura", "Avenir Next", "Helvetica Neue", sans-serif;
  }
  details.l4 summary::-webkit-details-marker { display: none; }
  details.l4 summary::before { content: "+ "; }
  details.l4[open] summary::before { content: "− "; }
  details.l4 summary:focus-visible { outline: 2px solid var(--magenta); outline-offset: 2px; }
  details.l4 .d {
    margin: 0.5rem 0 0.2rem; padding: 0.6rem 0.8rem;
    background: var(--mono-bg); border-radius: 6px;
    font-size: 0.82rem; color: var(--muted); overflow-x: auto;
  }
  details.l4 .d ul { margin: 0; padding-left: 1.1rem; }
  details.l4 .d li { margin-bottom: 0.3rem; }
  details.l4 .d li:last-child { margin-bottom: 0; }
  details.l4 .d code { font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 0.75rem; }
  #expand-toggle {
    margin-left: auto; flex-shrink: 0; cursor: pointer;
    background: none; border: 1px solid var(--hairline); border-radius: 4px;
    color: var(--muted); font-size: 0.68rem; letter-spacing: 0.12em; text-transform: uppercase;
    padding: 0.2rem 0.6rem;
    font-family: "Avenir Next Condensed", "Futura", "Avenir Next", "Helvetica Neue", sans-serif;
  }
  #expand-toggle:hover, #expand-toggle:focus-visible { color: var(--magenta); border-color: var(--magenta); }

  .kind { font-size: 0.68rem; letter-spacing: 0.14em; text-transform: uppercase; font-weight: 600; margin-right: 0.5rem; }
  .kind.meet { color: var(--pass); }
  .kind.never { color: var(--never); }
  .kind.hold { color: var(--muted); }

  .pair { display: grid; grid-template-columns: 1fr 1fr; gap: 0 2rem; }
  @media (max-width: 560px) { .pair { grid-template-columns: 1fr; } }

  .deflist { display: flex; flex-wrap: wrap; gap: 0.4rem; margin-top: 0.4rem; }
  .deflist span {
    font-family: ui-monospace, "SF Mono", Menlo, monospace; font-size: 0.75rem;
    background: var(--mono-bg); border-radius: 4px; padding: 0.15rem 0.5rem;
  }

  footer.colophon {
    margin-top: 4rem; padding-top: 1.1rem; border-top: 1px solid var(--hairline);
    font-size: 0.78rem; color: var(--muted);
    display: flex; justify-content: space-between; flex-wrap: wrap; gap: 0.5rem;
  }

  /* Unknown / extra H2 sections: collapsed at the end for backward compat. */
  details.tech { margin-top: 1.5rem; border: 1px solid var(--hairline); border-radius: 8px; }
  details.tech > summary {
    cursor: pointer; padding: 0.6rem 0.9rem;
    font-family: "Avenir Next Condensed", "Futura", "Avenir Next", "Helvetica Neue", sans-serif;
    font-size: 0.8rem; letter-spacing: 0.1em; text-transform: uppercase; color: var(--muted);
  }
  details.tech .section { padding: 0 0.9rem 0.9rem; }
  details.tech .section code { font-family: ui-monospace, "SF Mono", Menlo, monospace; }
"#;

/// Progress bar + builder-detail expand-all toggle (from the mock reference).
const CONTRACT_JS: &str = r#"
  const toggle = document.getElementById('expand-toggle');
  toggle.addEventListener('click', () => {
    const all = document.querySelectorAll('details.l4');
    const anyClosed = [...all].some(d => !d.open);
    all.forEach(d => d.open = anyClosed);
    toggle.textContent = anyClosed ? 'Human view' : 'Builder detail';
  });

  const bar = document.getElementById('route-progress');
  addEventListener('scroll', () => {
    const h = document.documentElement;
    const max = h.scrollHeight - h.clientHeight;
    bar.style.width = (max > 0 ? (h.scrollTop / max) * 100 : 0) + '%';
  }, { passive: true });
"#;

/// Canonical H2 sections, in render order. Anything else is an unknown/extra
/// section rendered collapsed at the end (backward compat).
const CONTRACT_SECTIONS: [&str; 5] = ["Why", "Decisions", "Boundaries", "Shipping", "Acceptance"];

/// Deterministic render of the thesis-tree `contract.md` to a self-contained
/// HTML page. H1 = hero thesis; the canonical H2s render as always-visible
/// thesis-tree sections; unknown H2s collapse into `details.tech` at the end.
fn contract_html(markdown: &str, source: &str, source_hash: &str) -> String {
    let mut h1 = String::new();
    let mut sections: Vec<(String, Vec<String>)> = Vec::new();
    let mut current: Option<(String, Vec<String>)> = None;
    for line in markdown.lines() {
        if let Some(rest) = line.strip_prefix("## ") {
            if let Some(prev) = current.take() {
                sections.push(prev);
            }
            current = Some((rest.trim().to_string(), Vec::new()));
        } else if let Some(rest) = line.strip_prefix("# ") {
            if h1.is_empty() {
                h1 = rest.trim().to_string();
            }
        } else if let Some((_, body)) = current.as_mut() {
            body.push(line.to_string());
        }
    }
    if let Some(prev) = current.take() {
        sections.push(prev);
    }

    let mut nav = String::from(
        "<nav class=\"topbar display\" aria-label=\"Contract sections\">\n\
         <span class=\"brand\">Build contract</span>\n",
    );
    let mut main = String::new();
    let mut tech = String::new();
    for (title, body) in &sections {
        if CONTRACT_SECTIONS.contains(&title.as_str()) {
            let id = title.to_ascii_lowercase();
            nav.push_str(&format!(
                "<a href=\"#{id}\">{}</a>\n",
                escape_html(title)
            ));
            main.push_str(&format!(
                "<section id=\"{id}\">\n<h2 class=\"display\">{}</h2>\n<ul class=\"tree\">\n{}</ul>\n</section>\n",
                escape_html(title),
                contract_tree_nodes(body),
            ));
        } else {
            tech.push_str(&format!(
                "<details class=\"tech\"><summary>{}</summary><div class=\"section\">{}</div></details>\n",
                escape_html(title),
                render_markdown(&body.join("\n")),
            ));
        }
    }
    nav.push_str("<button id=\"expand-toggle\" type=\"button\">Builder detail</button>\n</nav>\n");

    format!(
        "<!doctype html>\n<html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>Build contract</title>\n<style>{css}</style></head><body>\n\
         <div id=\"route-progress\"></div>\n{nav}<main>\n\
         <header class=\"hero\">\n<p class=\"eyebrow display\">Build contract</p>\n\
         <h1 class=\"thesis display\">{hero}</h1>\n</header>\n\
         {main}{tech}\
         <footer class=\"colophon\"><span>Rendered from <span class=\"mono\">{source}</span></span>\
         <span class=\"mono\">source_hash {hash}</span></footer>\n\
         </main>\n<script>{js}</script></body></html>\n",
        css = CONTRACT_CSS,
        js = CONTRACT_JS,
        nav = nav,
        hero = inline_markdown(&h1),
        main = main,
        tech = tech,
        source = escape_html(source),
        hash = escape_html(source_hash),
    )
}

/// Parse a section body into thesis-tree `<li>` nodes. A node is a top-level
/// `- ` list item; its continuation lines carry the L2 detail, `proof:`/`expect:`
/// lines, and a `builder detail:` nested list (collapsed L4).
fn contract_tree_nodes(body: &[String]) -> String {
    let mut groups: Vec<Vec<&str>> = Vec::new();
    for line in body {
        if line.starts_with("- ") {
            groups.push(vec![line.as_str()]);
        } else if let Some(group) = groups.last_mut() {
            group.push(line.as_str());
        }
    }
    groups.iter().map(|g| contract_node(g)).collect()
}

fn contract_node(lines: &[&str]) -> String {
    let first = &lines[0][2..];
    let (chip, thesis, first_detail) = match parse_criterion(first) {
        Some((tid, kind, rest)) => {
            let (thesis, trailing) = split_thesis(&rest);
            (
                format!(
                    "<span class=\"tid\">{tid}</span><span class=\"kind {kind} display\">{kind}</span>"
                ),
                thesis,
                trailing,
            )
        }
        None => {
            let (thesis, trailing) = split_thesis(first);
            (String::new(), thesis, trailing)
        }
    };

    let mut details: Vec<String> = Vec::new();
    if !first_detail.is_empty() {
        details.push(first_detail);
    }
    let mut proof: Option<String> = None;
    let mut expect: Option<String> = None;
    let mut l4: Vec<String> = Vec::new();
    let mut in_l4 = false;
    for line in &lines[1..] {
        let t = line.trim();
        if t.is_empty() {
            continue;
        }
        if t.strip_prefix("- ").unwrap_or(t).trim() == "builder detail:" {
            in_l4 = true;
        } else if let Some(rest) = t.strip_prefix("proof:") {
            proof = Some(rest.trim().to_string());
        } else if let Some(rest) = t.strip_prefix("expect:") {
            expect = Some(rest.trim().to_string());
        } else if in_l4 {
            if let Some(item) = t.strip_prefix("- ") {
                l4.push(item.trim().to_string());
            }
        } else {
            details.push(t.to_string());
        }
    }

    let mut out = format!(
        "<li>\n<p class=\"t1\">{chip}{}</p>\n",
        inline_markdown(&thesis)
    );
    if !details.is_empty() {
        out.push_str(&format!(
            "<p class=\"t2\">{}</p>\n",
            inline_markdown(&details.join(" "))
        ));
    }
    if let Some(proof) = proof {
        out.push_str(&format!("<p class=\"t3\">{}</p>\n", escape_html(&proof)));
    }
    if let Some(expect) = expect {
        out.push_str(&format!(
            "<p class=\"t3 expect\">{}</p>\n",
            escape_html(&expect)
        ));
    }
    if !l4.is_empty() {
        out.push_str("<details class=\"l4\"><summary>Builder detail</summary><div class=\"d\"><ul>");
        for item in &l4 {
            out.push_str(&format!("<li>{}</li>", inline_markdown(item)));
        }
        out.push_str("</ul></div></details>\n");
    }
    out.push_str("</li>\n");
    out
}

/// `A<n> (<kind>): <rest>` -> (id, kind, rest) for meet/never/hold; else None.
fn parse_criterion(line: &str) -> Option<(String, String, String)> {
    let line = line.trim();
    let (id, after) = line.split_once(" (")?;
    if !id.starts_with('A') || id.len() < 2 || !id[1..].bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (kind, rest) = after.split_once("):")?;
    let kind = kind.trim();
    if !matches!(kind, "meet" | "never" | "hold") {
        return None;
    }
    Some((id.to_string(), kind.to_string(), rest.trim_start().to_string()))
}

/// Split a leading `**thesis**` off a node line; trailing text is the detail.
fn split_thesis(text: &str) -> (String, String) {
    let text = text.trim();
    if let Some(rest) = text.strip_prefix("**") {
        if let Some(end) = rest.find("**") {
            return (rest[..end].trim().to_string(), rest[end + 2..].trim().to_string());
        }
    }
    (text.to_string(), String::new())
}

/// Render an inline markdown fragment (code -> mono span, links sanitized).
fn inline_markdown(fragment: &str) -> String {
    let rendered = render_markdown(fragment);
    let trimmed = rendered.trim();
    let inner = trimmed
        .strip_prefix("<p>")
        .and_then(|s| s.strip_suffix("</p>"))
        .unwrap_or(trimmed);
    inner
        .replace("<code>", "<span class=\"mono\">")
        .replace("</code>", "</span>")
}

/// Render markdown to HTML with raw-HTML neutralized and unsafe URLs stripped.
fn render_markdown(markdown: &str) -> String {
    use pulldown_cmark::{html, Options, Parser};
    let mut out = String::new();
    html::push_html(
        &mut out,
        Parser::new_ext(markdown, Options::all()).map(sanitize_event),
    );
    out
}

fn sanitize_event(event: pulldown_cmark::Event<'_>) -> pulldown_cmark::Event<'_> {
    use pulldown_cmark::{Event, Tag};
    match event {
        Event::Html(value) | Event::InlineHtml(value) => Event::Text(value),
        Event::Start(Tag::Link {
            link_type,
            dest_url,
            title,
            id,
        }) if unsafe_markdown_url(&dest_url) => Event::Start(Tag::Link {
            link_type,
            dest_url: "#".into(),
            title,
            id,
        }),
        Event::Start(Tag::Image {
            link_type,
            dest_url,
            title,
            id,
        }) if unsafe_markdown_url(&dest_url) => Event::Start(Tag::Image {
            link_type,
            dest_url: "#".into(),
            title,
            id,
        }),
        other => other,
    }
}

fn escape_html(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn unsafe_markdown_url(url: &str) -> bool {
    let url = url.trim_start().to_ascii_lowercase();
    ["javascript:", "data:", "vbscript:"]
        .iter()
        .any(|scheme| url.starts_with(scheme))
}

fn validate_contract_text(text: &str) -> Result<(), AppError> {
    if text.trim().len() < 200 {
        return Err(AppError::Input("contract.md is too small".into()));
    }
    if !text.to_ascii_lowercase().contains("acceptance") {
        return Err(AppError::Input(
            "contract.md must include acceptance criteria".into(),
        ));
    }
    Ok(())
}

fn validate_build_contract_text(text: &str) -> Result<(), AppError> {
    validate_contract_text(text)?;
    if !text.lines().next().is_some_and(|line| {
        line.starts_with("# ") && line.trim_end().ends_with('.') && !line.starts_with("## ")
    }) {
        return Err(AppError::Input(
            "contract.md must open with a one-sentence `# ` thesis".into(),
        ));
    }
    for heading in ["Why", "Decisions", "Boundaries", "Shipping", "Acceptance"] {
        if contract_section(text, heading).is_none() {
            return Err(AppError::Input(format!(
                "contract.md is missing `## {heading}`"
            )));
        }
    }
    let acceptance = contract_section(text, "Acceptance").unwrap();
    let criteria: Vec<&str> = acceptance
        .lines()
        .filter(|line| parse_criterion_marker(line).is_some())
        .collect();
    if criteria.is_empty() {
        return Err(AppError::Input(
            "contract.md acceptance criteria must use `- A<n> (meet|never|hold): **thesis.**`"
                .into(),
        ));
    }
    for marker in ["proof:", "expect:"] {
        let found = acceptance
            .lines()
            .filter(|line| line.trim().starts_with("- ") && line.contains(marker))
            .count();
        if found < criteria.len() {
            return Err(AppError::Input(format!(
                "every acceptance criterion needs a `{marker}` line ({found}/{} found)",
                criteria.len()
            )));
        }
    }
    if !text.contains("https://linear.app/") {
        return Err(AppError::Input(
            "contract.md must link its Linear Wayfinder map".into(),
        ));
    }
    Ok(())
}

fn parse_criterion_marker(line: &str) -> Option<(&str, &str)> {
    let rest = line.trim_start().strip_prefix("- A")?;
    let (number, rest) = rest.split_once(" (")?;
    if number.is_empty() || !number.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (kind, _) = rest.split_once("): ")?;
    matches!(kind, "meet" | "never" | "hold").then_some((number, kind))
}

fn contract_section<'a>(text: &'a str, heading: &str) -> Option<&'a str> {
    let marker = format!("## {heading}");
    let mut offset = 0;
    let mut start = None;
    for segment in text.split_inclusive('\n') {
        let line = segment.trim_end_matches(['\r', '\n']);
        if start.is_none() && line == marker {
            start = Some(offset + segment.len());
        } else if let Some(start) = start {
            if line.starts_with("## ") {
                return Some(text[start..offset].trim());
            }
        }
        offset += segment.len();
    }
    start.map(|start| text[start..].trim())
}


#[cfg(test)]
fn complete_contract_fixture() -> String {
    "# The outcome ships complete and verified.\n\n## Why\n\n- **The behavior is currently absent.** Users need this outcome to exist.\n\n## Decisions\n\n- **Use the existing path.** Settled on the [Wayfinder map](https://linear.app/example/issue/TASK-1/test).\n\n## Boundaries\n\n- **The builder owns the how.** Implementation and test design beyond the named proofs.\n- **Not in this build.** Unrelated work.\n\n## Shipping\n\n- **The change ships after verification.** Revert the commit if it fails.\n\n## Acceptance\n\n- A1 (meet): **The behavior works with durable proof.**\n  - proof: run-check\n  - expect: exit 0\n".into()
}

fn validate_phase_profiles(config: &Config) -> Result<(), AppError> {
    config.plan_profile.details()?;
    config.autoplan_review_profile.details()?;
    for (phase, profile) in [
        ("build", &config.build_profile),
        ("verify", &config.verify_profile),
    ] {
        validate_execution_profile(phase, profile)?;
    }
    Ok(())
}

fn validate_execution_profile(phase: &str, profile: &ModelProfile) -> Result<(), AppError> {
    let details = profile.details()?;
    if details.provider != AgentProvider::Codex || details.auth != AgentAuth::Subscription {
        return Err(AppError::Input(format!(
            "{phase} profile must use a Codex subscription: {}",
            profile.id()
        )));
    }
    Ok(())
}

fn load_agent_catalog() -> Result<AgentCatalog, String> {
    let output = ProcessCommand::new(agent_bridge_bin()?)
        .arg("agents")
        .output()
        .map_err(|e| format!("agent-bridge agents failed: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "agent-bridge agents exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let catalog: AgentCatalog = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("invalid agent-bridge agents output: {e}"))?;
    if catalog.agents.is_empty() {
        return Err("agent-bridge agents returned no agents".into());
    }
    let mut ids = BTreeMap::new();
    for agent in &catalog.agents {
        if agent.id.trim().is_empty()
            || agent.model.trim().is_empty()
            || agent.recommended_use.trim().is_empty()
        {
            return Err("agent-bridge agent fields cannot be empty".into());
        }
        if ids.insert(&agent.id, ()).is_some() {
            return Err(format!("duplicate agent-bridge profile: {}", agent.id));
        }
        for (grade, value) in [
            ("intelligence", agent.intelligence),
            ("taste", agent.taste),
            ("cost", f64::from(agent.cost)),
        ] {
            if !(1.0..=10.0).contains(&value) {
                return Err(format!("invalid {grade} grade for {}: {value}", agent.id));
            }
        }
    }
    Ok(catalog)
}

fn agent_bridge_bin() -> Result<PathBuf, String> {
    let home = env::var("HOME").map_err(|_| "HOME is unset".to_string())?;
    Ok(PathBuf::from(home).join(".local/bin/agent-bridge"))
}

fn agent_catalog() -> Result<&'static AgentCatalog, AppError> {
    AGENT_CATALOG
        .get_or_init(load_agent_catalog)
        .as_ref()
        .map_err(|e| AppError::External(e.clone()))
}

fn model_menu() -> Result<Value, AppError> {
    Ok(Value::Array(
        agent_catalog()?
            .agents
            .iter()
            .map(|agent| {
                let codex = agent.provider == AgentProvider::Codex;
                json!({
                    "profile": agent.id,
                    "model": agent.model,
                    "auth": agent.auth,
                    "effort": agent.effort,
                    "harness": agent.provider,
                    "intelligence": agent.intelligence,
                    "taste": agent.taste,
                    "cost": agent.cost,
                    "use": agent.recommended_use,
                    "phases": if codex { vec!["plan", "build", "verify"] } else { vec!["plan"] },
                })
            })
            .collect(),
    ))
}

fn model_profile(id: &str) -> Option<ModelProfile> {
    agent_catalog()
        .ok()?
        .agents
        .iter()
        .any(|agent| agent.id == id)
        .then(|| ModelProfile::named(id))
}

fn configure_models(current: &Config, args: ModelConfigArgs) -> Result<Value, AppError> {
    let mut config = current.clone();
    let changed = args.plan.is_some()
        || args.autoplan_review.is_some()
        || args.build.is_some()
        || args.verify.is_some();
    if let Some(profile) = args.plan {
        config.plan_profile = profile;
    }
    if let Some(profile) = args.autoplan_review {
        config.autoplan_review_profile = profile;
    }
    if let Some(profile) = args.build {
        config.build_profile = profile;
    }
    if let Some(profile) = args.verify {
        config.verify_profile = profile;
    }
    validate_phase_profiles(&config)?;
    let path = config.home.join("config.json");
    if changed {
        fs::create_dir_all(&config.home)?;
        atomic_write(&path, &(serde_json::to_string_pretty(&config)? + "\n"))?;
    }
    Ok(json!({"ok": true, "changed": changed, "config": config_json(&config)?, "path": path}))
}

fn prompt_with_context(mut prompt: String, paths: &[PathBuf]) -> Result<String, AppError> {
    for path in paths {
        let context = fs::read_to_string(path)
            .map_err(|e| AppError::Input(format!("invalid --context {}: {e}", path.display())))?;
        if context.trim().is_empty() {
            return Err(AppError::Input(format!(
                "--context is empty: {}",
                path.display()
            )));
        }
        prompt.push_str(&format!(
            "\n\n## Additional context: {}\n\n{}\n",
            path.display(),
            context.trim()
        ));
    }
    Ok(prompt)
}

fn validate_task(task: TaskInput) -> Result<Value, AppError> {
    if task.id.trim().is_empty() {
        return Err(AppError::Input("task id is empty".into()));
    }
    if !task
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(AppError::Input(format!("unsafe task id: {}", task.id)));
    }
    if task.prompt.trim().is_empty() {
        return Err(AppError::Input("prompt is empty".into()));
    }
    Ok(json!({"ok": true, "id": task.id}))
}

fn read_run_spec(run_dir: &Path) -> Result<RunSpec, AppError> {
    let path = run_dir.join("run.json");
    let raw = fs::read_to_string(&path)
        .map_err(|e| AppError::Input(format!("invalid {}: {e}", path.display())))?;
    let run = serde_json::from_str(&raw)
        .map_err(|e| AppError::Input(format!("invalid {}: {e}", path.display())))?;
    validate_run_spec(run_dir, run)
        .map_err(|e| AppError::Input(format!("invalid {}: {e}", path.display())))
}

fn validate_run_spec(run_dir: &Path, run: RunSpec) -> Result<RunSpec, AppError> {
    validate_task(TaskInput {
        id: run.id.clone(),
        prompt: run.prompt.clone(),
        metadata: json!({}),
    })?;
    let expected = run_dir.canonicalize()?;
    let declared = required_dir(run.run_dir.clone(), "run.json run_dir")?;
    if declared != expected {
        return Err(AppError::Input(format!(
            "run.json run_dir {} does not match {}",
            run.run_dir.display(),
            run_dir.display()
        )));
    }
    required_dir(run.workdir.clone(), "run.json workdir")?;
    validate_execution_profile("run.json build", &run.build_profile)?;
    validate_execution_profile("run.json verify", &run.verify_profile)?;
    Ok(run)
}

fn validate_verdict_file(path: &Path) -> Result<Value, AppError> {
    let run_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(env::current_dir()?);
    let run = read_run_spec(&run_dir)?;
    let verdict: Verdict = serde_json::from_str(&fs::read_to_string(path)?)?;
    if verdict.run_id != run.id {
        return Err(AppError::Input(format!(
            "verdict run_id {} does not match run.json id {}",
            verdict.run_id, run.id
        )));
    }
    if verdict.contract != Path::new("contract.md") {
        return Err(AppError::Input(
            "verdict contract must be contract.md".into(),
        ));
    }
    if (verdict.status == VerdictStatus::Green) != verdict.criteria_met {
        return Err(AppError::Input(
            "verdict status green must match criteria_met true".into(),
        ));
    }
    if !run_dir.join(&verdict.contract).is_file() {
        return Err(AppError::Input(format!(
            "missing contract: {}",
            run_dir.join(&verdict.contract).display()
        )));
    }
    if verdict.criteria_met && verdict.evidence.is_empty() {
        return Err(AppError::Input(
            "criteria_met true requires at least one evidence path".into(),
        ));
    }
    let run_root = run_dir.canonicalize()?;
    for evidence in &verdict.evidence {
        if evidence.is_absolute()
            || evidence
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_)))
        {
            return Err(AppError::Input(format!(
                "evidence path outside run dir: {}",
                evidence.display()
            )));
        }
        let evidence_path = run_dir.join(evidence);
        if !evidence_path.exists() {
            return Err(AppError::Input(format!(
                "missing evidence: {}",
                evidence_path.display()
            )));
        }
        if !evidence_path.canonicalize()?.starts_with(&run_root) {
            return Err(AppError::Input(format!(
                "evidence path outside run dir: {}",
                evidence.display()
            )));
        }
    }
    Ok(json!({"ok": true, "run_dir": run_dir, "run": run, "verdict": verdict}))
}

fn validate_verifier_file(path: &Path) -> Result<Value, AppError> {
    let run_dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or(env::current_dir()?);
    read_run_spec(&run_dir)?;
    let verifier: VerifierVerdict = serde_json::from_str(&fs::read_to_string(path)?)?;
    if verifier.notes.trim().is_empty() {
        return Err(AppError::Input("verifier notes are empty".into()));
    }
    Ok(json!({"ok": true, "run_dir": run_dir, "verifier": verifier}))
}

fn inspect_run(config: &Config, run: &str) -> Result<Value, AppError> {
    let path = PathBuf::from(run);
    let run_dir = if path.is_dir() {
        path
    } else {
        find_run(&config.home, run)?
    };
    read_run_spec(&run_dir)?;
    let result_path = run_dir.join("result.json");
    Ok(serde_json::from_str(&fs::read_to_string(result_path)?)?)
}

fn cancel_run(config: &Config, run: &str) -> Result<Value, AppError> {
    cancel_run_with_grace(config, run, Duration::from_secs(2))
}

fn cancel_run_with_grace(config: &Config, run: &str, grace: Duration) -> Result<Value, AppError> {
    let run_dir = resolve_run(&config.home, run)?;
    let spec = read_run_spec(&run_dir)?;
    let _state = lock_with_grace(&run_dir.join("run.json"), grace)?;
    if run_dir.join("sentinel").is_file() || run_dir.join("result.json").is_file() {
        return Err(AppError::Input(format!("run already terminal: {run}")));
    }
    let (pid, child_pid) = stop_run_processes(&run_dir, grace)?;
    if run_dir.join("sentinel").is_file() || run_dir.join("result.json").is_file() {
        return Err(AppError::Input(format!("run already terminal: {run}")));
    }
    let result = terminal_result(
        RunStatus::Canceled,
        spec.id,
        "canceled by mil cancel",
        &run_dir,
    );
    atomic_write(
        &run_dir.join("result.json"),
        &(serde_json::to_string_pretty(&result)? + "\n"),
    )?;
    fs::write(run_dir.join("sentinel"), "canceled\n")?;
    let _ = fs::remove_file(run_dir.join("run.pid"));
    let _ = fs::remove_file(run_dir.join(ACTIVE_CHILD_PID_FILE));
    Ok(json!({
        "ok": true,
        "run": run_dir,
        "status": "canceled",
        "pid_signaled": pid > 1,
        "child_pid_signaled": child_pid > 1
    }))
}

fn maintain_runs(config: &Config, pressure: Option<StoragePressure>) -> Result<Value, AppError> {
    maintain_runs_with_retention(config, pressure, RUN_CACHE_RETENTION)
}

fn maintain_runs_with_retention(
    config: &Config,
    pressure: Option<StoragePressure>,
    retention: Duration,
) -> Result<Value, AppError> {
    let runs_dir = config.home.join("runs");
    let mut runs = if runs_dir.is_dir() {
        fs::read_dir(&runs_dir)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .collect::<Vec<_>>()
    } else {
        Vec::new()
    };
    runs.sort();

    let clean = pressure.is_some_and(|value| !matches!(value, StoragePressure::Healthy));
    let mut terminal = 0_u64;
    let mut active = 0_u64;
    let mut retained = 0_u64;
    let mut removed_bytes = 0_u64;
    let mut removed = Vec::new();
    let mut candidates = Vec::new();

    for run in &runs {
        let metadata = fs::symlink_metadata(run)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(AppError::Input(format!(
                "run entry is not a real directory: {}",
                run.display()
            )));
        }
        let result = run.join("result.json");
        if !result.is_file() {
            if run_pid(run).is_some_and(process_alive) {
                active += 1;
            } else {
                retained += 1;
            }
            continue;
        }
        terminal += 1;
        let age = fs::metadata(&result)?
            .modified()?
            .elapsed()
            .unwrap_or(Duration::ZERO);
        if age < retention {
            retained += 1;
            continue;
        }

        let cache_dirs = owned_run_cache_dirs(&run.join("evidence"))?;
        if !clean || cache_dirs.is_empty() {
            for cache in &cache_dirs {
                let relative = cache.strip_prefix(run).map_err(|_| {
                    AppError::Invariant(format!("cache escaped run: {}", cache.display()))
                })?;
                candidates.push(json!({
                    "run": run.file_name().and_then(OsStr::to_str).unwrap_or(""),
                    "path": relative,
                    "bytes": directory_bytes(cache)?,
                }));
            }
            retained += 1;
            continue;
        }
        let mut run_removed = Vec::new();
        for cache in cache_dirs {
            let bytes = directory_bytes(&cache)?;
            fs::remove_dir_all(&cache)?;
            removed_bytes = removed_bytes.saturating_add(bytes);
            let relative = cache
                .strip_prefix(run)
                .map_err(|_| {
                    AppError::Invariant(format!("cache escaped run: {}", cache.display()))
                })?
                .to_path_buf();
            removed.push(relative.display().to_string());
            run_removed.push(json!({"path": relative, "bytes": bytes}));
        }
        atomic_write(
            &run.join("maintenance.json"),
            &(serde_json::to_string_pretty(&json!({
                "maintained_at": iso_now(),
                "removed": run_removed,
            }))? + "\n"),
        )?;
    }

    Ok(json!({
        "ok": true,
        "pressure": pressure.map(StoragePressure::as_str).unwrap_or("status"),
        "runs": runs.len(),
        "terminal": terminal,
        "active": active,
        "retained": retained,
        "removed": removed,
        "removedBytes": removed_bytes,
        "candidates": candidates,
        "retentionDays": retention.as_secs() / (24 * 60 * 60),
    }))
}

fn owned_run_cache_dirs(evidence: &Path) -> Result<Vec<PathBuf>, AppError> {
    if !evidence.exists() {
        return Ok(Vec::new());
    }
    let metadata = fs::symlink_metadata(evidence)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(AppError::Input(format!(
            "run evidence is not a real directory: {}",
            evidence.display()
        )));
    }
    let root = evidence.canonicalize()?;
    let mut out = Vec::new();
    fn walk(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), AppError> {
        for entry in fs::read_dir(dir)? {
            let path = entry?.path();
            let metadata = fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                continue;
            }
            if !metadata.is_dir() {
                continue;
            }
            if is_rebuildable_cache_dir(&path)? {
                let canonical = path.canonicalize()?;
                if !canonical.starts_with(root) {
                    return Err(AppError::Invariant(format!(
                        "cache escaped evidence: {}",
                        path.display()
                    )));
                }
                out.push(path);
            } else {
                walk(root, &path, out)?;
            }
        }
        Ok(())
    }
    walk(&root, evidence, &mut out)?;
    out.sort();
    Ok(out)
}

/// Names a build tool owns outright; anything inside is rebuildable by definition.
const REBUILDABLE_CACHE_NAMES: [&str; 4] =
    ["DerivedData", "SourcePackages", ".build", "node_modules"];
/// Directories Xcode only ever writes inside DerivedData.
const DERIVED_DATA_MARKERS: [&str; 3] = [
    "ModuleCache.noindex",
    "Index.noindex",
    "CompilationCache.noindex",
];
/// Files a compiler module cache holds; anything else means the directory is not one.
const MODULE_CACHE_SUFFIXES: [&str; 3] = [".pcm", ".swiftmodule", ".timestamp"];

/// Decides whether a directory holds rebuildable build data rather than run evidence.
///
/// A name alone never decides it: run evidence is free to be called `derived` or
/// `packages`, so every rule below demands the layout the owning tool actually writes.
/// Symlinks are never followed and their presence disqualifies the whole-contents rules.
fn is_rebuildable_cache_dir(path: &Path) -> Result<bool, AppError> {
    let name = path.file_name().and_then(OsStr::to_str).unwrap_or("");
    if REBUILDABLE_CACHE_NAMES.contains(&name) {
        return Ok(true);
    }
    if name == "Caches"
        && path
            .parent()
            .and_then(Path::file_name)
            .and_then(OsStr::to_str)
            == Some("Library")
    {
        return Ok(true);
    }
    let listing = DirListing::read(path)?;
    // Xcode DerivedData: a marker directory, or the build root beside its manifest.
    if listing
        .dirs
        .iter()
        .any(|d| DERIVED_DATA_MARKERS.contains(&d.as_str()))
        || (listing.has_dir("Build") && listing.has_file("info.plist"))
    {
        return Ok(true);
    }
    // SwiftPM workspace: resolved dependency state beside the clones it describes.
    if listing.has_file("workspace-state.json")
        && (listing.has_dir("checkouts") || listing.has_dir("repositories"))
    {
        return Ok(true);
    }
    // SwiftPM repository mirrors: nothing but bare clones of upstream packages.
    if listing.exclusively_dirs()
        && listing
            .dirs
            .iter()
            .all(|d| is_bare_git_repository(&path.join(d)))
    {
        return Ok(true);
    }
    // Compiler module cache: nothing but precompiled modules and their timestamps.
    if listing.exclusively_files()
        && listing
            .files
            .iter()
            .any(|f| f.ends_with(".pcm") || f.ends_with(".swiftmodule"))
        && listing
            .files
            .iter()
            .all(|f| MODULE_CACHE_SUFFIXES.iter().any(|s| f.ends_with(s)))
    {
        return Ok(true);
    }
    Ok(false)
}

/// One directory's immediate children, split by kind, with symlinks counted but never followed.
struct DirListing {
    dirs: Vec<String>,
    files: Vec<String>,
    symlinks: usize,
}

impl DirListing {
    fn read(path: &Path) -> Result<Self, AppError> {
        let mut listing = Self {
            dirs: Vec::new(),
            files: Vec::new(),
            symlinks: 0,
        };
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            let metadata = fs::symlink_metadata(entry.path())?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if metadata.file_type().is_symlink() {
                listing.symlinks += 1;
            } else if metadata.is_dir() {
                listing.dirs.push(name);
            } else {
                listing.files.push(name);
            }
        }
        Ok(listing)
    }

    fn has_dir(&self, name: &str) -> bool {
        self.dirs.iter().any(|d| d == name)
    }

    fn has_file(&self, name: &str) -> bool {
        self.files.iter().any(|f| f == name)
    }

    fn exclusively_dirs(&self) -> bool {
        self.symlinks == 0 && self.files.is_empty() && !self.dirs.is_empty()
    }

    fn exclusively_files(&self) -> bool {
        self.symlinks == 0 && self.dirs.is_empty() && !self.files.is_empty()
    }
}

fn is_bare_git_repository(path: &Path) -> bool {
    let kind = |child: &str, want_dir: bool| {
        fs::symlink_metadata(path.join(child))
            .is_ok_and(|m| !m.file_type().is_symlink() && m.is_dir() == want_dir)
    };
    kind("HEAD", false) && kind("objects", true) && kind("refs", true)
}

fn resolve_run(home: &Path, run: &str) -> Result<PathBuf, AppError> {
    let path = PathBuf::from(run);
    if path.is_dir() {
        return Ok(path);
    }
    find_run(home, run)
}

fn find_run(home: &Path, run: &str) -> Result<PathBuf, AppError> {
    let mut matches = fs::read_dir(home.join("runs"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(OsStr::to_str)
                .is_some_and(|n| n.contains(run))
        })
        .collect::<Vec<_>>();
    matches.sort();
    if let Some(exact) = matches.iter().find(|p| {
        p.file_name()
            .and_then(OsStr::to_str)
            .is_some_and(|n| n == run)
    }) {
        return Ok(exact.clone());
    }
    match matches.len() {
        0 => Err(AppError::Input(format!("run not found: {run}"))),
        1 => Ok(matches.remove(0)),
        _ => Err(AppError::Input(format!(
            "run is ambiguous: {run}; matches: {}",
            matches
                .iter()
                .filter_map(|p| p.file_name().and_then(OsStr::to_str))
                .collect::<Vec<_>>()
                .join(", ")
        ))),
    }
}

fn lock(path: &Path) -> Result<File, AppError> {
    lock_with_grace(path, Duration::ZERO)
}

fn lock_with_grace(path: &Path, grace: Duration) -> Result<File, AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(false)
        .open(path)?;
    let deadline = Instant::now() + grace;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                return Err(AppError::External("lock held".into()))
            }
            Err(error) => return Err(error.into()),
        }
    }
}

pub(crate) fn doctor_checks(root: &Path, config: &Config) -> Vec<Value> {
    vec![
        check_file("codex_loop_prompt", &root.join("prompts/codex-loop.md")),
        check_file("verify_prompt", &root.join("prompts/verify.md")),
        check_cmd("claude"),
        check_cmd("codex"),
        check_agent_bridge(),
        check_file("config_home", &config.home),
    ]
}

fn doctor(root: &Path, config: &Config, json_out: bool) -> Result<(), AppError> {
    report_checks("doctor", doctor_checks(root, config), json_out)
}

// setup: fail-fast auth readiness for the three agent lanes. Never prints secret values.
pub(crate) fn setup_checks() -> Vec<Value> {
    vec![
        check_anthropic_key(),
        check_login(
            "claude",
            "claude_login",
            "install Claude Code and run `claude login`",
        ),
        check_login(
            "codex",
            "codex_login",
            "install Codex and run `codex login`",
        ),
    ]
}

fn setup(config: &Config, args: &SetupArgs) -> Result<(), AppError> {
    if let Some(path) = &args.anthropic_key_file {
        engine::read_env_key(path, "ANTHROPIC_API_KEY")?;
        let config_path = config.home.join("config.json");
        fs::create_dir_all(&config.home)?;
        let mut cur: Value = fs::read_to_string(&config_path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_else(|| json!({}));
        cur["anthropic_key_file"] = json!(path);
        fs::write(&config_path, serde_json::to_string(&cur)? + "\n")?;
    }
    report_checks("setup", setup_checks(), args.json)
}

fn report_checks(name: &str, checks: Vec<Value>, json_out: bool) -> Result<(), AppError> {
    let ok = checks.iter().all(|c| c["ok"].as_bool() == Some(true));
    let body = json!({"ok": ok, "checks": checks});
    if json_out {
        write_out(None, &body)?;
    } else {
        eprintln!("{}", serde_json::to_string_pretty(&body)?);
    }
    if ok {
        Ok(())
    } else {
        Err(AppError::External(format!("{name} failed")))
    }
}

fn check_anthropic_key() -> Value {
    let from_env = env::var("ANTHROPIC_API_KEY")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    let from_file = engine::anthropic_key_file()
        .and_then(|p| engine::read_env_key(&p, "ANTHROPIC_API_KEY"))
        .is_ok();
    let ok = from_env || from_file;
    json!({
        "name": "anthropic_api_key",
        "ok": ok,
        "hint": if ok { Value::Null } else { json!("set ANTHROPIC_API_KEY (or point ANTHROPIC_KEY_FILE at an env file that defines it)") }
    })
}

fn check_login(cmd: &str, name: &str, hint: &str) -> Value {
    let ok = ProcessCommand::new(cmd)
        .arg("--help")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    json!({"name": name, "ok": ok, "hint": if ok { Value::Null } else { json!(hint) }})
}

fn check_file(name: &str, path: &Path) -> Value {
    json!({"name": name, "ok": path.exists(), "path": path})
}
fn check_agent_bridge() -> Value {
    match agent_bridge_bin() {
        Ok(path) => check_file("agent_bridge", &path),
        Err(error) => json!({"name": "agent_bridge", "ok": false, "hint": error}),
    }
}
fn check_cmd(name: &str) -> Value {
    json!({"name": name, "ok": ProcessCommand::new(name).arg("--help").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok()})
}

fn install(root: &Path) -> Result<(), AppError> {
    let status = ProcessCommand::new("cargo")
        .arg("build")
        .arg("--release")
        .arg("--manifest-path")
        .arg(root.join("Cargo.toml"))
        .status()?;
    if !status.success() {
        return Err(AppError::External("cargo build failed".into()));
    }
    let home = env::var("HOME").unwrap_or_else(|_| ".".into());
    let built = root.join("target/release/mil");
    install_artifacts(root, Path::new(&home), &built)
}

const SKILL_HOSTS: [&str; 3] = [".agents", ".claude", ".codex"];
const SKILL_NAMES: [&str; 3] = ["milhouse", "mil-plan", "mil-prototype"];

fn install_artifacts(root: &Path, home: &Path, built: &Path) -> Result<(), AppError> {
    for host in SKILL_HOSTS {
        for skill in SKILL_NAMES {
            let source = root.join("skills").join(skill);
            let link = home.join(host).join("skills").join(skill);
            validate_skill_installation(&source, &link)?;
        }
    }
    for dir in ["bin", ".local/bin"] {
        let dir = home.join(dir);
        fs::create_dir_all(&dir)?;
        let installed = dir.join("mil");
        if fs::read_link(&installed).ok().as_deref() != Some(built) {
            let tmp = installed.with_extension("new");
            fs::copy(built, &tmp)?;
            fs::rename(tmp, &installed)?;
        }
    }
    for host in SKILL_HOSTS {
        let skills = home.join(host).join("skills");
        fs::create_dir_all(&skills)?;
        for skill in SKILL_NAMES {
            install_skill_link(&root.join("skills").join(skill), &skills.join(skill))?;
        }
    }
    let helper_dir = home.join(".milhouse/bin");
    fs::create_dir_all(&helper_dir)?;
    fs::copy(
        root.join("scripts/durable-command.py"),
        helper_dir.join("durable-command"),
    )?;
    println!("installed: ~/bin/mil, ~/.local/bin/mil, shared Agents/Claude/Codex skills");
    Ok(())
}

fn install_skill_link(source: &Path, link: &Path) -> Result<(), AppError> {
    validate_skill_installation(source, link)?;
    if fs::read_link(link).ok().as_deref() == Some(source) {
        return Ok(());
    }
    let name = link
        .file_name()
        .ok_or_else(|| AppError::Input(format!("skill link has no file name: {}", link.display())))?
        .to_string_lossy();
    let staged = link.with_file_name(format!(".{name}.milhouse-new-{}", std::process::id()));
    if fs::symlink_metadata(&staged).is_ok() {
        return Err(AppError::Input(format!(
            "skill install staging path already exists: {}",
            staged.display()
        )));
    }
    std::os::unix::fs::symlink(source, &staged)?;
    if let Err(error) = fs::rename(&staged, link) {
        let _ = fs::remove_file(&staged);
        return Err(error.into());
    }
    Ok(())
}

fn validate_skill_installation(source: &Path, link: &Path) -> Result<(), AppError> {
    let source_metadata = fs::symlink_metadata(source).map_err(|error| {
        AppError::Input(format!(
            "cannot inspect skill source {}: {error}",
            source.display()
        ))
    })?;
    if !source_metadata.is_dir()
        || source_metadata.file_type().is_symlink()
        || !source.join("SKILL.md").is_file()
    {
        return Err(AppError::Input(format!(
            "skill source is incomplete: {}",
            source.display()
        )));
    }
    match fs::symlink_metadata(link) {
        Ok(metadata) if !metadata.file_type().is_symlink() => {
            return Err(AppError::Input(format!(
                "refusing to replace non-symlink skill installation: {}",
                link.display()
            )));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn write_out<T: Serialize>(path: Option<PathBuf>, value: &T) -> Result<(), AppError> {
    let text = serde_json::to_string_pretty(value)? + "\n";
    if let Some(path) = path {
        fs::write(path, text)?;
    } else {
        print!("{text}");
        io::stdout().flush()?;
    }
    Ok(())
}
fn write_log(run_dir: &Path, msg: &str) -> Result<(), AppError> {
    append_text(
        &run_dir.join("milhouse.log"),
        &format!("{} [milhouse] {}\n", human_now(), msg),
    )
}
fn append_jsonl(path: &Path, value: &impl Serialize) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    append_text(path, &(serde_json::to_string(value)? + "\n"))
}
fn append_text(path: &Path, text: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    f.write_all(text.as_bytes())?;
    Ok(())
}
fn atomic_write(path: &Path, text: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    fs::rename(tmp, path)?;
    Ok(())
}
fn read_json_value(path: &Path) -> Option<Value> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}
fn read_text(path: &Path) -> Result<String, AppError> {
    Ok(fs::read_to_string(path)?)
}
fn read_cap(path: &Path, cap: usize) -> Result<String, AppError> {
    let mut f = File::open(path)?;
    let mut buf = vec![0; cap];
    let n = f.read(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf[..n]).to_string())
}
fn child_timeout(key: &str, default_secs: u64) -> Duration {
    Duration::from_secs(
        env::var(key)
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(default_secs),
    )
}
fn wait_status_with_activity(
    mut child: Child,
    timeout: Duration,
    idle_timeout: Duration,
    activity_root: &Path,
    label: &str,
) -> Result<ExitStatus, AppError> {
    let start = Instant::now();
    let mut idle_since = start;
    let mut activity = latest_modified(activity_root);
    let mut next_activity_check = start;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if start.elapsed() > timeout {
            stop_child(&mut child);
            return Err(AppError::External(format!(
                "{label} timed out after {}s",
                timeout.as_secs()
            )));
        }
        let now = Instant::now();
        if now >= next_activity_check || idle_since.elapsed() > idle_timeout {
            if activity_root.join("run.json").is_file() {
                match directory_bytes(activity_root) {
                    Ok(bytes) if bytes > RUN_STORAGE_BUDGET_BYTES => {
                        stop_child(&mut child);
                        return Err(AppError::StorageBudget(format!(
                            "{label} run storage exceeded {} bytes (used {bytes})",
                            RUN_STORAGE_BUDGET_BYTES
                        )));
                    }
                    Ok(_) => {}
                    Err(error) => {
                        stop_child(&mut child);
                        return Err(error);
                    }
                }
            }
            let modified = latest_modified(activity_root);
            if modified != activity {
                activity = modified;
                idle_since = now;
            } else if idle_since.elapsed() > idle_timeout {
                stop_child(&mut child);
                return Err(AppError::Stale(format!(
                    "{label} made no durable progress for {}s",
                    idle_timeout.as_secs()
                )));
            }
            next_activity_check = now + Duration::from_secs(5);
        }
        thread::sleep(Duration::from_millis(250));
    }
}

fn latest_modified(path: &Path) -> SystemTime {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return UNIX_EPOCH;
    };
    let mut latest = metadata.modified().unwrap_or(UNIX_EPOCH);
    if metadata.is_dir() {
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            latest = latest.max(latest_modified(&entry.path()));
        }
    }
    latest
}

fn stop_child(child: &mut Child) {
    kill_process_group(child.id() as i32);
    thread::sleep(Duration::from_secs(2));
    if child.try_wait().ok().flatten().is_none() {
        kill_process_group_force(child.id() as i32);
    }
    let _ = child.wait();
}
fn wait_output(mut child: Child, timeout: Duration, label: &str) -> Result<Output, AppError> {
    let start = Instant::now();
    loop {
        if child.try_wait()?.is_some() {
            return Ok(child.wait_with_output()?);
        }
        if start.elapsed() > timeout {
            kill_process_group(child.id() as i32);
            thread::sleep(Duration::from_secs(2));
            if child.try_wait()?.is_none() {
                kill_process_group_force(child.id() as i32);
            }
            let _ = child.wait();
            return Err(AppError::External(format!(
                "{label} timed out after {}s",
                timeout.as_secs()
            )));
        }
        thread::sleep(Duration::from_millis(250));
    }
}
fn kill_process_group(pid: i32) {
    unsafe {
        libc::killpg(pid, libc::SIGTERM);
    }
}
fn kill_process_group_force(pid: i32) {
    unsafe {
        libc::killpg(pid, libc::SIGKILL);
    }
}
pub(crate) fn stop_run_processes(
    run_dir: &Path,
    grace: Duration,
) -> Result<(i32, i32), AppError> {
    let mut errors = Vec::new();
    let runner_pid = match tracked_pid(run_dir, "run.pid", grace) {
        Ok(pid) => pid,
        Err(error) => {
            errors.push(error.to_string());
            0
        }
    };
    if let Err(error) = stop_process_group(runner_pid, grace) {
        errors.push(error.to_string());
    }
    let child_pid = match tracked_pid(run_dir, ACTIVE_CHILD_PID_FILE, grace) {
        Ok(pid) => pid,
        Err(error) => {
            errors.push(error.to_string());
            0
        }
    };
    if let Err(error) = stop_process_group(child_pid, grace) {
        errors.push(error.to_string());
    }
    if !errors.is_empty() {
        return Err(AppError::ProcessesAlive(format!(
            "failed to stop all run process groups: {}",
            errors.join("; ")
        )));
    }
    Ok((runner_pid, child_pid))
}
pub(crate) fn stop_process_group(pid: i32, grace: Duration) -> Result<(), AppError> {
    signal_process_group(pid, libc::SIGTERM)?;
    let deadline = Instant::now() + grace;
    while process_group_alive(pid)? && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if process_group_alive(pid)? {
        signal_process_group(pid, libc::SIGKILL)?;
    }
    let deadline = Instant::now() + grace;
    while process_group_alive(pid)? && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if process_group_alive(pid)? {
        return Err(AppError::ProcessesAlive(format!(
            "process group {pid} survived SIGKILL"
        )));
    }
    Ok(())
}
fn tracked_pid(
    run_dir: &Path,
    pid_file: &str,
    grace: Duration,
) -> Result<i32, AppError> {
    let pid_path = run_dir.join(pid_file);
    let deadline = Instant::now() + grace;
    loop {
        let mut file = match OpenOptions::new().read(true).write(true).open(&pid_path) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error.into()),
        };
        match FileExt::try_lock_shared(&file) {
            Ok(()) => {
                let mut value = String::new();
                file.read_to_string(&mut value)?;
                if let Ok(pid) = value.trim().parse::<i32>() {
                    if process_group_alive(pid)? {
                        return Err(AppError::External(format!(
                            "{pid_file} names a live process group without an identity lock"
                        )));
                    }
                }
                return Ok(0);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut value = String::new();
                if file.read_to_string(&mut value).is_ok() {
                    if let Ok(pid) = value.trim().parse::<i32>() {
                        if pid > 1 {
                            return Ok(pid);
                        }
                    }
                }
            }
            Err(error) => return Err(error.into()),
        }
        if Instant::now() >= deadline {
            return Err(AppError::External(
                format!("{pid_file} is locked without a valid pid; refusing terminal state"),
            ));
        }
        thread::sleep(Duration::from_millis(10));
    }
}
fn signal_process_group(pid: i32, signal: i32) -> Result<(), AppError> {
    if pid <= 1 {
        return Ok(());
    }
    let result = unsafe { libc::killpg(pid, signal) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(AppError::External(format!(
        "failed to signal process group {pid}: {error}"
    )))
}
fn process_group_alive(pid: i32) -> Result<bool, AppError> {
    if pid <= 1 {
        return Ok(false);
    }
    if unsafe { libc::killpg(pid, 0) } == 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(false);
    }
    if error.raw_os_error() == Some(libc::EPERM) {
        return Ok(true);
    }
    Err(AppError::External(format!(
        "failed to inspect process group {pid}: {error}"
    )))
}
fn terminal_result(status: RunStatus, issue: String, reason: &str, run_dir: &Path) -> Value {
    json!({
        "status": status.as_str(),
        "issue": issue,
        "reason": reason,
        "pushed": null,
        "claude_cost_usd": null,
        "milhouse_cost_usd": null,
        "cost_status": "unknown",
        "cost_by_tier": {"fable":0.0,"claude":0.0,"codex":0.0},
        "milhouse_cost_by_tier": {"fable":0.0,"claude":0.0,"codex":0.0},
        "cost_by_model": {},
        "milhouse_cost_by_model": {},
        "run_dir": run_dir,
        "grade_score": null,
        "variant": "milhouse-rust"
    })
}
fn read_jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}
fn walk_files(dir: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut out = Vec::new();
    fn rec(p: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
        for e in fs::read_dir(p)? {
            let p = e?.path();
            if p.is_dir() {
                rec(&p, out)?;
            } else {
                out.push(p);
            }
        }
        Ok(())
    }
    rec(dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn directory_bytes(path: &Path) -> Result<u64, AppError> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Ok(0);
    }
    if metadata.is_file() {
        return Ok(metadata.len());
    }
    let mut bytes = 0_u64;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            bytes = bytes.saturating_add(directory_bytes(&entry?.path())?);
        }
    }
    Ok(bytes)
}
fn git_toplevel(dir: &Path) -> Option<PathBuf> {
    let out = ProcessCommand::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .ok()?;
    if out.status.success() {
        Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
    } else {
        None
    }
}
fn git_short_head(dir: &Path) -> Option<String> {
    let mut heads = Vec::new();
    for repo in git_repos(dir) {
        let out = ProcessCommand::new("git")
            .arg("-C")
            .arg(&repo)
            .arg("rev-parse")
            .arg("--short")
            .arg("HEAD")
            .output()
            .ok()?;
        if out.status.success() {
            heads.push((
                repo.strip_prefix(dir)
                    .unwrap_or(&repo)
                    .display()
                    .to_string(),
                String::from_utf8_lossy(&out.stdout).trim().to_string(),
            ));
        }
    }
    match heads.as_slice() {
        [] => None,
        [(_, sha)] => Some(sha.clone()),
        _ => Some(
            heads
                .into_iter()
                .map(|(repo, sha)| format!("{repo}@{sha}"))
                .collect::<Vec<_>>()
                .join(","),
        ),
    }
}

fn git_repos(workdir: &Path) -> Vec<PathBuf> {
    let root = env::var_os("HOME")
        .filter(|home| Path::new(home) == workdir)
        .map(|home| PathBuf::from(home).join("Build"))
        .unwrap_or_else(|| workdir.to_path_buf());
    let mut repos = Vec::new();
    collect_git_repos(&root, &mut repos);
    repos.sort();
    repos.dedup();
    repos
}

fn collect_git_repos(dir: &Path, repos: &mut Vec<PathBuf>) {
    if dir.join(".git").is_dir() {
        repos.push(fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf()));
        return;
    }
    if dir.join(".git").is_file() {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.')
            || name.starts_with("DerivedData")
            || matches!(name.as_ref(), "node_modules" | "target" | "Pods")
        {
            continue;
        }
        collect_git_repos(&entry.path(), repos);
    }
}
fn cmd_text(cmd: &mut ProcessCommand, cap: usize) -> String {
    cmd.output()
        .ok()
        .map(|o| {
            String::from_utf8_lossy(&o.stdout)
                .chars()
                .take(cap)
                .collect()
        })
        .unwrap_or_default()
}
fn arch_sha(root: &Path) -> Result<String, AppError> {
    let mut s = String::new();
    for p in [
        root.join("src/main.rs"),
        root.join("src/engine.rs"),
        root.join("prompts/codex-loop.md"),
        root.join("prompts/verify.md"),
    ] {
        s.push_str(&fs::read_to_string(p).unwrap_or_default());
    }
    Ok(format!("{:x}", seahash::hash(s.as_bytes()))[..7].to_string())
}
fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
fn iso_now() -> String {
    chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string()
}
fn human_now() -> String {
    chrono::Local::now().format("%F %T").to_string()
}
fn compact_timestamp_local() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}
fn run_dir_epoch(base: &str) -> u64 {
    chrono::NaiveDateTime::parse_from_str(base.get(0..15).unwrap_or(""), "%Y%m%d-%H%M%S")
        .ok()
        .and_then(|d| d.and_local_timezone(chrono::Local).single())
        .map(|d| d.timestamp() as u64)
        .unwrap_or(0)
}
fn duration_secs(run: &Path) -> u64 {
    let s = fs::metadata(run.join("run.json"))
        .or_else(|_| fs::metadata(run.join("card.json")))
        .and_then(|m| m.modified())
        .ok();
    let e = fs::metadata(run.join("sentinel"))
        .or_else(|_| fs::metadata(run.join("result.json")))
        .and_then(|m| m.modified())
        .ok();
    e.and_then(|e| s.and_then(|s| e.duration_since(s).ok()))
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
fn tail_lines(path: &Path, n: usize) -> Vec<String> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter(|l| !l.trim().is_empty())
        .rev()
        .take(n)
        .map(str::to_string)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}
fn round2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

fn config_json(config: &Config) -> Result<Value, AppError> {
    Ok(json!({
        "grade_floor": config.grade_floor,
        "daily_cost_cap_usd": config.daily_cost_cap_usd,
        "plan_profile": config.plan_profile.id(),
        "autoplan_review_profile": config.autoplan_review_profile.id(),
        "build_profile": config.build_profile.id(),
        "verify_profile": config.verify_profile.id(),
        "agents": model_menu()?,
    }))
}
fn active_run(home: &Path) -> Option<String> {
    let mut cands = Vec::new();
    for e in fs::read_dir(home.join("runs")).ok()?.filter_map(Result::ok) {
        let p = e.path();
        if !p.join("sentinel").is_file() && run_pid(&p).is_some_and(process_alive) {
            cands.push(e.file_name().into_string().ok()?);
        }
    }
    cands.sort();
    cands.pop()
}
fn run_pid(run: &Path) -> Option<i32> {
    match tracked_pid(run, "run.pid", Duration::ZERO) {
        Ok(pid) if pid > 1 => Some(pid),
        Ok(_) => tracked_pid(run, ACTIVE_CHILD_PID_FILE, Duration::ZERO)
            .ok()
            .filter(|pid| *pid > 1),
        Err(_) => None,
    }
}
fn process_alive(pid: i32) -> bool {
    process_group_alive(pid).unwrap_or(true)
}
fn iso_epoch(ts: &str) -> u64 {
    chrono::DateTime::parse_from_rfc3339(ts)
        .map(|d| d.timestamp() as u64)
        .unwrap_or(0)
}
fn round4(v: f64) -> f64 {
    (v * 10000.0).round() / 10000.0
}
fn parity_rows(rows: &[Value]) -> Vec<Value> {
    rows.iter()
        .filter(|r| {
            r.get("variant").is_some()
                && !matches!(
                    r.get("variant").and_then(Value::as_str),
                    Some("fable-reference" | "fable-gold")
                )
        })
        .map(|r| {
            let mut row = r.clone();
            row["cost"] = attempt_cost(r);
            row
        })
        .collect()
}
fn attempt_cost(r: &Value) -> Value {
    r.get("milhouse_cost")
        .or_else(|| r.get("oneshot_cost"))
        .or_else(|| r.get("cost"))
        .cloned()
        .unwrap_or(Value::Null)
}
fn build_bench_data(rows: &[Value]) -> Value {
    json!({"versions": bank_versions_from_rows(rows)})
}
fn bank_versions_from_rows(rows: &[Value]) -> Vec<Value> {
    let mut map: BTreeMap<String, BTreeMap<String, Vec<Value>>> = BTreeMap::new();
    for r in rows {
        if let (Some(b), Some(t)) = (
            r.get("bench").and_then(Value::as_str),
            r.get("task").and_then(Value::as_str),
        ) {
            map.entry(b.to_string())
                .or_default()
                .entry(t.to_string())
                .or_default()
                .push(r.clone());
        }
    }
    map.into_iter()
        .rev()
        .map(|(version, tasks)| {
            json!({"version":version,"tasks":tasks.into_iter().map(|(task, rows)| bench_task_row(&task, &rows)).collect::<Vec<_>>()})
        })
        .collect()
}
fn bench_task_row(task: &str, rows: &[Value]) -> Value {
    let mut reference = None;
    let mut gold_runs = Vec::new();
    let mut variants = serde_json::Map::new();
    let mut latest: HashMap<String, u64> = HashMap::new();
    let holdout = rows
        .iter()
        .any(|r| r.get("holdout").and_then(Value::as_bool).unwrap_or(false));
    for r in rows {
        let variant = r.get("variant").and_then(Value::as_str).unwrap_or("");
        match variant {
            "fable-reference" => {
                if let Some(score) = r.get("score").and_then(Value::as_f64) {
                    reference = Some(reference.map_or(score, |cur: f64| cur.max(score)));
                }
            }
            "fable-gold" => gold_runs.push(bench_attempt(r)),
            "" => {}
            name => {
                let ts = iso_epoch(r.get("ts").and_then(Value::as_str).unwrap_or(""));
                if latest.get(name).is_none_or(|cur| ts >= *cur) {
                    latest.insert(name.to_string(), ts);
                    variants.insert(name.to_string(), bench_attempt(r));
                }
            }
        }
    }
    let gold_scores: Vec<f64> = gold_runs
        .iter()
        .filter_map(|r| r.get("score").and_then(Value::as_f64))
        .collect();
    let gold_costs: Vec<f64> = gold_runs
        .iter()
        .filter_map(|r| r.get("cost").and_then(Value::as_f64))
        .collect();
    json!({
        "task": task,
        "holdout": holdout,
        "reference": reference,
        "gold": {"score": median(gold_scores), "cost": median(gold_costs), "n": gold_runs.len(), "runs": gold_runs},
        "variants": variants,
    })
}
fn bench_attempt(r: &Value) -> Value {
    json!({
        "score": r.get("score").cloned().unwrap_or(Value::Null),
        "cost": attempt_cost(r),
        "grade_cost": r.get("grade_cost").cloned().unwrap_or(Value::Null),
        "ts": iso_epoch(r.get("ts").and_then(Value::as_str).unwrap_or("")),
        "worker": r.get("models").and_then(|m| m.get("worker")).cloned().unwrap_or(Value::Null),
        "work": r.get("work").cloned().unwrap_or(Value::Null),
    })
}
fn median(mut xs: Vec<f64>) -> Option<f64> {
    if xs.is_empty() {
        return None;
    }
    xs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let mid = xs.len() / 2;
    Some(round4(if xs.len().is_multiple_of(2) {
        (xs[mid - 1] + xs[mid]) / 2.0
    } else {
        xs[mid]
    }))
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;
    use std::os::unix::process::CommandExt;

    #[test]
    fn agent_catalog_accepts_bridge_context_metadata() {
        let catalog: AgentCatalog = serde_json::from_value(serde_json::json!({
            "scale": {},
            "agents": [{
                "id": "test-subscription",
                "provider": "codex",
                "model": "test-model",
                "auth": "subscription",
                "effort": "high",
                "intelligence": 7.5,
                "taste": 8.5,
                "cost": 2,
                "use": "test",
                "context": { "tokens": 200000 }
            }]
        })).unwrap();
        assert_eq!(catalog.agents[0].id, "test-subscription");
        assert_eq!(catalog.agents[0].intelligence, 7.5);
        assert_eq!(catalog.agents[0].taste, 8.5);
    }

    fn temp_run(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "milhouse-test-{name}-{}-{}",
            std::process::id(),
            epoch()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("evidence")).unwrap();
        fs::write(
            dir.join("contract.md"),
            format!("# Acceptance criteria\n\n{}", "proof\n".repeat(40)),
        )
        .unwrap();
        fs::write(dir.join("evidence/test.log"), "ok\n").unwrap();
        fs::write(
            dir.join("run.json"),
            serde_json::to_string(&RunSpec {
                id: name.into(),
                short_id: None,
                run_dir: dir.clone(),
                workdir: dir.clone(),
                prompt: "do thing".into(),
                build_profile: default_build_profile(),
                verify_profile: default_verify_profile(),
                resume_from: None,
                resume_mode: None,
            })
            .unwrap(),
        )
        .unwrap();
        dir
    }

    fn install_fixture(name: &str) -> (PathBuf, PathBuf, PathBuf) {
        let base = env::temp_dir().join(format!(
            "milhouse-install-test-{name}-{}-{}",
            std::process::id(),
            epoch()
        ));
        let root = base.join("repo");
        let home = base.join("home");
        let built = root.join("target/release/mil");
        for skill in ["milhouse", "mil-plan", "mil-prototype"] {
            let skill_dir = root.join("skills").join(skill);
            fs::create_dir_all(&skill_dir).unwrap();
            fs::write(skill_dir.join("SKILL.md"), format!("# {skill}\n")).unwrap();
        }
        fs::create_dir_all(built.parent().unwrap()).unwrap();
        fs::write(&built, "mil binary").unwrap();
        fs::create_dir_all(root.join("scripts")).unwrap();
        fs::write(
            root.join("scripts/durable-command.py"),
            "#!/usr/bin/env python3\n",
        )
        .unwrap();
        (root, home, built)
    }

    #[test]
    fn install_uses_one_skill_source_for_every_harness() {
        let (root, home, built) = install_fixture("shared-skills");
        install_artifacts(&root, &home, &built).unwrap();
        install_artifacts(&root, &home, &built).unwrap();

        for host in [".agents", ".claude", ".codex"] {
            for skill in ["milhouse", "mil-plan", "mil-prototype"] {
                assert_eq!(
                    fs::read_link(home.join(host).join("skills").join(skill)).unwrap(),
                    root.join("skills").join(skill)
                );
            }
        }
        assert_eq!(
            fs::read_to_string(home.join(".local/bin/mil")).unwrap(),
            "mil binary"
        );
        assert_eq!(
            fs::read_to_string(home.join("bin/mil")).unwrap(),
            "mil binary"
        );
    }

    #[test]
    fn install_preserves_unmanaged_skill_paths() {
        let (root, home, built) = install_fixture("preserve-unmanaged");
        let conflict = home.join(".claude/skills/mil-plan");
        fs::create_dir_all(conflict.parent().unwrap()).unwrap();
        fs::write(&conflict, "mine\n").unwrap();

        let error = install_artifacts(&root, &home, &built).unwrap_err();
        assert!(error
            .to_string()
            .contains("refusing to replace non-symlink"));
        assert_eq!(fs::read_to_string(conflict).unwrap(), "mine\n");
        assert!(!home.join(".agents/skills/mil-plan").exists());
    }

    #[test]
    fn install_repoints_only_existing_symlinks() {
        let (root, home, built) = install_fixture("repoint-symlink");
        let link = home.join(".codex/skills/mil-plan");
        fs::create_dir_all(link.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/tmp/old-mil-plan", &link).unwrap();

        install_artifacts(&root, &home, &built).unwrap();
        assert_eq!(fs::read_link(link).unwrap(), root.join("skills/mil-plan"));
    }

    #[test]
    fn autonomous_plan_requires_a_linear_issue() {
        assert!(Cli::try_parse_from([
            "mil",
            "plan",
            "--autonomous",
            "--issue",
            "TASK-1",
            "fix the bug"
        ])
        .is_ok());
        assert!(Cli::try_parse_from(["mil", "plan", "--autonomous", "fix the bug"]).is_err());
    }

    #[test]
    fn autoplan_reviewer_is_configurable() {
        assert!(Cli::try_parse_from([
            "mil",
            "config",
            "--autoplan-review",
            "opus-5-5-high-subscription"
        ])
        .is_ok());
    }

    #[test]
    fn planning_defaults_to_opus_subscription() {
        let config = Config::default();
        assert_eq!(config.plan_profile.id(), "opus-5-5-high-subscription");
        assert_eq!(
            config.autoplan_review_profile.id(),
            "opus-5-5-high-subscription"
        );
        let opus = Model::Opus.details().unwrap();
        assert_eq!(opus.model, "claude-opus-5-5");
        assert_eq!(opus.auth, AgentAuth::Subscription);
    }

    #[test]
    fn build_contract_gate_requires_complete_clear_contract() {
        let contract = complete_contract_fixture();
        assert!(validate_build_contract_text(&contract).is_ok());
        assert!(validate_build_contract_text(&contract.replace("## Shipping", "## Delivery")).is_err());
        assert!(validate_build_contract_text(&contract.replace("# The outcome", "The outcome")).is_err());
        assert!(validate_build_contract_text(&contract.replace("A1 (meet)", "A1 (maybe)")).is_err());
        assert!(validate_build_contract_text(&contract.replace("  - expect: exit 0\n", "")).is_err());
        assert!(validate_build_contract_text(&contract.replace("https://linear.app", "https://example.com")).is_err());
        assert!(Cli::try_parse_from([
            "mil",
            "contract",
            "validate",
            "--contract",
            "/tmp/contract.md"
        ])
        .is_ok());
        assert!(Cli::try_parse_from([
            "mil",
            "contract",
            "render",
            "--contract",
            "/tmp/contract.md"
        ])
        .is_ok());
    }

    #[test]
    fn contract_html_renders_thesis_tree() {
        let markdown = "\
# Every conversation, *captured verbatim*, in one history.

## Why

- **Context dies with each session.** Conversations sit in vendor logs Rolly cannot search.

## Decisions

- **Capture at wrapper boundaries.** Hermes surfaces and `rolly agent` write records.
  - builder detail:
    - `telegram` capture happens in the surface adapter.
    - A surface without a wrapper is not captured.

## Boundaries

- **The builder owns the how.** Store schema, search, capture mechanics.

## Shipping

- **Capture ships behind a flag.** Production stays untouched until the flag flips.

## Acceptance

- A1 (meet): **Every surface maps a message into the store.**
  For each surface: capture writes a byte-identical record.
  proof: capture-verify roundtrip --each-surface --expect-diffs 0
  expect: exit 0 & diffs=0 on all surfaces
  - builder detail:
    - Fixture corpus: at least 200 messages.
- A4 (never): **Content without authorization.**
  proof: capture-verify authz --as unauthorized
  expect: response=denied

## Telemetry notes

Some extra non-canonical section with a [link](https://example.com/x) and an [unsafe](javascript:alert('no')) one.
";
        let html = contract_html(markdown, "contract.md", "abc123def4560000");

        // Hero comes from the H1 thesis, with inline emphasis preserved.
        assert!(html
            .contains("<h1 class=\"thesis display\">Every conversation, <em>captured verbatim</em>, in one history.</h1>"));

        // The five canonical sections are always-visible <section>s, not <details>.
        for id in ["why", "decisions", "boundaries", "shipping", "acceptance"] {
            assert!(html.contains(&format!("<section id=\"{id}\">")), "missing section {id}");
        }

        // A criterion renders its id chip, kind tag, proof and expect lines.
        assert!(html.contains("<span class=\"tid\">A1</span>"));
        assert!(html.contains("<span class=\"kind meet display\">meet</span>"));
        assert!(html.contains("<span class=\"kind never display\">never</span>"));
        assert!(html.contains("<p class=\"t3\">capture-verify roundtrip --each-surface --expect-diffs 0</p>"));
        assert!(html.contains("<p class=\"t3 expect\">exit 0 &amp; diffs=0 on all surfaces</p>"));

        // Builder detail collapses into an l4 block.
        assert!(html.contains("<details class=\"l4\"><summary>Builder detail</summary>"));
        assert!(html.contains("Fixture corpus: at least 200 messages."));

        // Unknown H2 collapses into details.tech at the end.
        assert!(html.contains("<details class=\"tech\"><summary>Telemetry notes</summary>"));

        // URL sanitization from the old renderer is preserved.
        assert!(html.contains("href=\"https://example.com/x\""));
        assert!(!html.contains("href=\"javascript:"));

        // Hash lands in the colophon; output is deterministic.
        assert!(html.contains("source_hash abc123def4560000"));
        assert_eq!(html, contract_html(markdown, "contract.md", "abc123def4560000"));
    }

    #[test]
    fn contract_render_writes_html_and_refuses_overwrite() {
        let dir = temp_run("contract-render");
        let contract = dir.join("contract.md");
        fs::write(&contract, complete_contract_fixture()).unwrap();
        assert!(render_contract_file(&contract, Some(&contract)).is_err());
        let out = render_contract_file(&contract, None).unwrap();
        assert!(contract.with_extension("html").is_file());
        assert_eq!(out["source_hash"].as_str().unwrap().len(), 16);
    }

    #[test]
    fn extracts_last_score_json() {
        let raw =
            "noise {\"score\":1,\"citations\":[\"a\"]} tail {\"score\":88,\"citations\":[\"b\"]}";
        let v = grade::extract_score_json(raw).unwrap();
        assert_eq!(v["score"], 88);
    }

    #[test]
    fn config_validation_is_strict() {
        assert!(dashboard::valid_config("grade_floor", &json!(60)));
        assert!(!dashboard::valid_config("grade_floor", &json!(55)));
    }

    #[test]
    fn parity_rows_drop_yardsticks() {
        let rows = vec![
            json!({"variant":"oneshot","score":80,"cost":0.6,"oneshot_cost":0.4,"grade_cost":0.2}),
            json!({"variant":"fable-gold","score":90}),
            json!({"variant":"fable-reference","score":99}),
        ];
        let kept = parity_rows(&rows);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0]["variant"], "oneshot");
        assert_eq!(kept[0]["cost"], 0.4);
        assert_eq!(kept[0]["grade_cost"], 0.2);
    }

    #[test]
    fn bench_data_aggregates_runs_by_task() {
        let rows = vec![
            json!({"bench":"v1","task":"t1","variant":"fable-reference","score":90,"ts":"2026-07-01T00:00:00Z"}),
            json!({"bench":"v1","task":"t1","variant":"fable-reference","score":95,"ts":"2026-07-01T00:01:00Z"}),
            json!({"bench":"v1","task":"t1","variant":"fable-gold","score":80,"cost":1.0,"ts":"2026-07-01T00:02:00Z"}),
            json!({"bench":"v1","task":"t1","variant":"fable-gold","score":85,"cost":2.0,"ts":"2026-07-01T00:03:00Z"}),
            json!({"bench":"v1","task":"t1","variant":"oneshot","score":70,"cost":0.5,"ts":"2026-07-01T00:04:00Z","models":{"worker":"old"}}),
            json!({"bench":"v1","task":"t1","variant":"oneshot","score":75,"cost":0.6,"oneshot_cost":0.4,"grade_cost":0.2,"ts":"2026-07-01T00:05:00Z","models":{"worker":"new"}}),
            json!({"bench":"v1","task":"t1","variant":"legacy","score":65,"cost":0.7,"ts":"2026-07-01T00:06:00Z"}),
        ];
        let data = build_bench_data(&rows);
        let tasks = data["versions"][0]["tasks"].as_array().unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0]["task"], "t1");
        assert_eq!(tasks[0]["reference"], 95.0);
        assert_eq!(tasks[0]["gold"]["score"], 82.5);
        assert_eq!(tasks[0]["gold"]["cost"], 1.5);
        assert_eq!(tasks[0]["gold"]["n"], 2);
        assert_eq!(tasks[0]["variants"]["oneshot"]["score"], 75);
        assert_eq!(tasks[0]["variants"]["oneshot"]["cost"], 0.4);
        assert_eq!(tasks[0]["variants"]["oneshot"]["grade_cost"], 0.2);
        assert_eq!(tasks[0]["variants"]["oneshot"]["worker"], "new");
        assert_eq!(tasks[0]["variants"]["legacy"]["cost"], 0.7);
    }

    #[test]
    fn run_spec_is_strict() {
        let bad = json!({"id":"x","run_dir":"/tmp/x","workdir":"/tmp","prompt":"x","extra":true});
        assert!(serde_json::from_value::<RunSpec>(bad).is_err());
    }

    #[test]
    fn run_spec_accepts_resume_lineage() {
        let dir = temp_run("resume-lineage");
        fs::write(
            dir.join("run.json"),
            serde_json::to_string(&json!({
                "id": "resume-lineage",
                "run_dir": dir.clone(),
                "workdir": dir.clone(),
                "prompt": "continue",
                "resume_from": dir.clone(),
                "resume_mode": "reuse_contract"
            }))
            .unwrap(),
        )
        .unwrap();
        let run = read_run_spec(&dir).unwrap();
        assert_eq!(run.resume_mode, Some(ResumeMode::ReuseContract));

        fs::write(
            dir.join("run.json"),
            serde_json::to_string(&json!({
                "id": "resume-lineage",
                "run_dir": dir.clone(),
                "workdir": dir.clone(),
                "prompt": "continue",
                "resume_mode": "maybe"
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(read_run_spec(&dir).is_err());
    }

    #[test]
    fn task_input_is_strict() {
        let bad = json!({"id":"x","prompt":"x","extra":true});
        assert!(serde_json::from_value::<TaskInput>(bad).is_err());
    }

    #[test]
    fn read_run_spec_validates_contract() {
        let dir = temp_run("run-contract");
        assert!(read_run_spec(&dir).is_ok());

        fs::write(
            dir.join("run.json"),
            serde_json::to_string(&json!({
                "id": "run-contract",
                "run_dir": dir.clone(),
                "workdir": "/tmp/milhouse-missing-workdir",
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        let err = read_run_spec(&dir).unwrap_err().to_string();
        assert!(err.contains("run.json workdir is not a directory"));

        fs::write(
            dir.join("run.json"),
            serde_json::to_string(&json!({
                "id": "bad/id",
                "run_dir": dir.clone(),
                "workdir": env::temp_dir(),
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        let err = read_run_spec(&dir).unwrap_err().to_string();
        assert!(err.contains("unsafe task id"));
    }

    #[test]
    fn cancel_run_marks_terminal() {
        let dir = temp_run("cancel-contract");
        let out = cancel_run(&Config::default(), dir.to_str().unwrap()).unwrap();
        assert_eq!(out["status"], "canceled");
        assert_eq!(
            fs::read_to_string(dir.join("sentinel")).unwrap(),
            "canceled\n"
        );
        let result = read_json_value(&dir.join("result.json")).unwrap();
        assert_eq!(result["status"], "canceled");
        assert_eq!(result["cost_status"], "unknown");
        assert!(cancel_run(&Config::default(), dir.to_str().unwrap()).is_err());
    }

    #[test]
    fn cancel_run_does_not_overwrite_a_result_published_before_sentinel() {
        let dir = temp_run("cancel-published-result");
        fs::write(dir.join("result.json"), r#"{"status":"green"}"#).unwrap();

        assert!(cancel_run(&Config::default(), dir.to_str().unwrap()).is_err());
        assert_eq!(
            fs::read_to_string(dir.join("result.json")).unwrap(),
            r#"{"status":"green"}"#
        );
        assert!(!dir.join("sentinel").exists());
    }

    #[test]
    fn cancel_run_waits_for_startup_state_lock() {
        let dir = temp_run("cancel-startup-lock");
        let state = lock(&dir.join("run.json")).unwrap();
        let release = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            drop(state);
        });

        let out = cancel_run_with_grace(
            &Config::default(),
            dir.to_str().unwrap(),
            Duration::from_millis(250),
        );

        release.join().unwrap();
        assert!(out.is_ok(), "{out:?}");
    }

    #[test]
    fn cancel_run_stops_runner_and_active_child_before_terminalizing() {
        use std::os::unix::process::CommandExt;

        fn sleep_group() -> Child {
            let mut command = ProcessCommand::new("sleep");
            command.arg("30").process_group(0);
            command.spawn().unwrap()
        }
        let dir = temp_run("cancel-process-groups");
        let runner = sleep_group();
        let child = sleep_group();
        let runner_pid = runner.id();
        let child_pid = child.id();
        fs::write(dir.join("run.pid"), format!("{runner_pid}\n")).unwrap();
        let runner_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("run.pid"))
            .unwrap();
        runner_lock.try_lock_exclusive().unwrap();
        fs::write(dir.join(ACTIVE_CHILD_PID_FILE), format!("{child_pid}\n")).unwrap();
        let child_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join(ACTIVE_CHILD_PID_FILE))
            .unwrap();
        child_lock.try_lock_exclusive().unwrap();
        let runner_wait = thread::spawn(move || {
            let status = runner.wait_with_output().unwrap().status;
            drop(runner_lock);
            status
        });
        let child_wait = thread::spawn(move || {
            let status = child.wait_with_output().unwrap().status;
            drop(child_lock);
            status
        });

        let out = cancel_run_with_grace(
            &Config::default(),
            dir.to_str().unwrap(),
            Duration::from_millis(250),
        );
        let runner_status = runner_wait.join().unwrap();
        let child_status = child_wait.join().unwrap();

        assert!(out.is_ok(), "{out:?}");
        assert!(!runner_status.success());
        assert!(!child_status.success());
        assert!(dir.join("result.json").is_file());
        assert!(dir.join("sentinel").is_file());
        assert!(!dir.join("run.pid").exists());
        assert!(!dir.join(ACTIVE_CHILD_PID_FILE).exists());
    }

    #[test]
    fn cancel_run_does_not_signal_an_unlocked_stale_child_pid() {
        use std::os::unix::process::CommandExt;

        fn sleep_group() -> Child {
            let mut command = ProcessCommand::new("sleep");
            command.arg("30").process_group(0);
            command.spawn().unwrap()
        }
        let dir = temp_run("cancel-stale-child-pid");
        let runner = sleep_group();
        let mut unrelated = sleep_group();
        fs::write(dir.join("run.pid"), format!("{}\n", runner.id())).unwrap();
        let runner_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(false)
            .open(dir.join("run.pid"))
            .unwrap();
        runner_lock.try_lock_exclusive().unwrap();
        fs::write(
            dir.join(ACTIVE_CHILD_PID_FILE),
            format!("{}\n", unrelated.id()),
        )
        .unwrap();
        let runner_wait = thread::spawn(move || {
            let status = runner.wait_with_output().unwrap().status;
            drop(runner_lock);
            status
        });

        let out = cancel_run_with_grace(
            &Config::default(),
            dir.to_str().unwrap(),
            Duration::from_millis(250),
        );

        assert!(out.is_err(), "{out:?}");
        assert!(!runner_wait.join().unwrap().success());
        assert!(unrelated.try_wait().unwrap().is_none());
        assert!(!dir.join("result.json").exists());
        assert!(!dir.join("sentinel").exists());
        kill_process_group_force(unrelated.id() as i32);
        unrelated.wait().unwrap();
    }

    #[test]
    fn cancel_run_does_not_signal_a_stale_runner_during_concurrent_inspection() {
        use std::os::unix::process::CommandExt;

        let dir = temp_run("cancel-stale-runner-pid");
        let mut unrelated = ProcessCommand::new("sleep");
        unrelated.arg("30").process_group(0);
        let mut unrelated = unrelated.spawn().unwrap();
        fs::write(dir.join("run.pid"), format!("{}\n", unrelated.id())).unwrap();
        let inspector = OpenOptions::new()
            .read(true)
            .write(true)
            .open(dir.join("run.pid"))
            .unwrap();
        FileExt::try_lock_shared(&inspector).unwrap();

        let out = cancel_run_with_grace(
            &Config::default(),
            dir.to_str().unwrap(),
            Duration::from_millis(250),
        );

        assert!(out.is_err(), "{out:?}");
        assert!(unrelated.try_wait().unwrap().is_none());
        assert!(!dir.join("result.json").exists());
        assert!(!dir.join("sentinel").exists());
        drop(inspector);
        kill_process_group_force(unrelated.id() as i32);
        unrelated.wait().unwrap();
    }

    #[test]
    fn tracked_pid_waits_for_locked_pid_publication() {
        let dir = temp_run("pid-publication");
        let path = dir.join("run.pid");
        let identity = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .unwrap();
        identity.try_lock_exclusive().unwrap();
        let publisher = thread::spawn(move || {
            thread::sleep(Duration::from_millis(25));
            fs::write(path, format!("{}\n", std::process::id())).unwrap();
        });

        let pid = tracked_pid(&dir, "run.pid", Duration::from_millis(250)).unwrap();

        publisher.join().unwrap();
        assert_eq!(pid, std::process::id() as i32);
        drop(identity);
    }

    #[test]
    fn run_pid_uses_locked_child_after_runner_exit() {
        let dir = temp_run("active-child-only");
        let pid = dir.join(ACTIVE_CHILD_PID_FILE);
        fs::write(&pid, format!("{}\n", std::process::id())).unwrap();
        let identity = OpenOptions::new()
            .read(true)
            .write(true)
            .open(pid)
            .unwrap();
        identity.try_lock_exclusive().unwrap();

        assert_eq!(run_pid(&dir), Some(std::process::id() as i32));
    }

    #[test]
    fn child_group_remains_alive_after_its_leader_exits() {
        use std::os::unix::process::CommandExt;

        let dir = temp_run("active-child-descendant-only");
        let group_file = dir.join("group");
        let mut leader = ProcessCommand::new("sh");
        leader
            .arg("-c")
            .arg("echo $$ > \"$1\"; sleep 30 &")
            .arg("sh")
            .arg(&group_file)
            .process_group(0);
        leader.spawn().unwrap().wait().unwrap();
        let group = fs::read_to_string(group_file)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();
        let pid = dir.join(ACTIVE_CHILD_PID_FILE);
        fs::write(&pid, format!("{group}\n")).unwrap();
        let identity = OpenOptions::new()
            .read(true)
            .write(true)
            .open(pid)
            .unwrap();
        identity.try_lock_exclusive().unwrap();

        assert_eq!(run_pid(&dir), Some(group));
        assert!(process_alive(group));

        kill_process_group_force(group);
        drop(identity);
    }

    #[test]
    fn dashboard_marks_dead_pid_run_stale() {
        let dir = temp_run("dead-pid");
        fs::write(dir.join("run.pid"), "999999\n").unwrap();
        let light = dashboard::build_run_light(&dir).unwrap();
        assert_eq!(light["status"], "stale");
        assert_eq!(light["running"], false);
        assert_eq!(light["cancelable"], true);
    }

    #[test]
    fn build_requires_workdir_and_contract() {
        let err = match Cli::try_parse_from(["mil", "build", "--contract", "/tmp/c.md"]) {
            Ok(_) => panic!("build accepted missing --workdir"),
            Err(err) => err,
        };
        assert_eq!(err.kind(), clap::error::ErrorKind::MissingRequiredArgument);
        assert!(err.to_string().contains("--workdir <WORKDIR>"));
    }

    #[test]
    fn context_files_are_appended() {
        let dir = env::temp_dir().join(format!("mil-context-test-{}", epoch()));
        fs::create_dir_all(&dir).unwrap();
        let context = dir.join("context.md");
        fs::write(&context, "Preserve route playback.").unwrap();
        let prompt =
            prompt_with_context("Improve routes.".into(), std::slice::from_ref(&context)).unwrap();
        assert!(prompt.contains("Improve routes."));
        assert!(prompt.contains("Preserve route playback."));
        assert!(prompt_with_context("x".into(), &[dir.join("missing.md")]).is_err());
    }

    #[test]
    fn build_accepts_one_contract() {
        assert!(Cli::try_parse_from([
            "mil",
            "build",
            "--workdir",
            "/tmp",
            "--contract",
            "/tmp/contract.md"
        ])
        .is_ok());
    }

    #[test]
    fn build_profile_must_use_codex() {
        let mut config = Config::default();
        let luna = ModelProfile::named("luna-6-high-subscription");
        set_build_profile(&mut config, Some(luna.clone())).unwrap();
        assert_eq!(config.build_profile, luna);
        assert!(
            set_build_profile(&mut config, Some(ModelProfile::named("fable-5-1-high-api")))
                .is_err()
        );
    }

    #[test]
    fn config_ignores_only_removed_autopilot_key() {
        let dir = env::temp_dir().join(format!("milhouse-config-test-{}", epoch()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.json");
        fs::write(
            &path,
            r#"{"autopilot_enabled":true,"daily_cost_cap_usd":32}"#,
        )
        .unwrap();
        assert_eq!(load_config(Some(&path)).unwrap().daily_cost_cap_usd, 32.0);
        fs::write(&path, r#"{"unknown":true}"#).unwrap();
        assert!(load_config(Some(&path)).is_err());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn run_cost_tracks_builder_cost() {
        let mut cost = RunCost {
            codex: None,
            unknown: false,
        };
        cost.codex = Some(0.0);
        assert_eq!(cost.total(), Some(0.0));
        cost.codex = Some(2.0);
        assert_eq!(cost.total(), Some(2.0));
        assert_eq!(RunCost::default().total(), None);
    }

    #[test]
    fn verdict_is_strict() {
        let bad = json!({"run_id":"x","status":"green","summary":"ok","criteria_met":true,"contract":"contract.md","evidence":[],"note":"","extra":true});
        assert!(serde_json::from_value::<Verdict>(bad).is_err());
    }

    #[test]
    fn verifier_is_strict() {
        let bad = json!({"passed":true,"notes":"ok","extra":true});
        assert!(serde_json::from_value::<VerifierVerdict>(bad).is_err());
    }

    #[test]
    fn validates_verdict_contract() {
        let dir = temp_run("validates-verdict");
        fs::write(
            dir.join("verdict.json"),
            r#"{"run_id":"validates-verdict","status":"green","summary":"ok","criteria_met":true,"contract":"contract.md","evidence":["evidence/test.log"],"note":""}"#,
        )
        .unwrap();
        assert!(validate_verdict_file(&dir.join("verdict.json")).is_ok());
        fs::write(
            dir.join("verdict.json"),
            r#"{"run_id":"validates-verdict","status":"hit_run_limit","summary":"limit","criteria_met":false,"contract":"contract.md","evidence":[],"note":""}"#,
        )
        .unwrap();
        assert!(validate_verdict_file(&dir.join("verdict.json")).is_ok());
        fs::write(
            dir.join("verdict.json"),
            r#"{"run_id":"validates-verdict","status":"failed","summary":"failed","criteria_met":false,"contract":"contract.md","evidence":[],"note":""}"#,
        )
        .unwrap();
        assert!(validate_verdict_file(&dir.join("verdict.json")).is_ok());
    }

    #[test]
    fn validates_verifier_contract() {
        let dir = temp_run("validates-verifier");
        fs::write(
            dir.join("verifier.json"),
            r#"{"passed":false,"notes":"missing evidence"}"#,
        )
        .unwrap();
        assert!(validate_verifier_file(&dir.join("verifier.json")).is_ok());
        fs::write(dir.join("verifier.json"), r#"{"passed":false,"notes":""}"#).unwrap();
        assert!(validate_verifier_file(&dir.join("verifier.json")).is_err());
    }

    #[test]
    fn verdict_requires_matching_run_and_evidence() {
        let dir = temp_run("bad-verdict");
        fs::write(
            dir.join("verdict.json"),
            r#"{"run_id":"wrong","status":"green","summary":"ok","criteria_met":true,"contract":"contract.md","evidence":["evidence/test.log"],"note":""}"#,
        )
        .unwrap();
        assert!(validate_verdict_file(&dir.join("verdict.json")).is_err());
        fs::write(
            dir.join("verdict.json"),
            r#"{"run_id":"bad-verdict","status":"green","summary":"ok","criteria_met":true,"contract":"contract.md","evidence":[],"note":""}"#,
        )
        .unwrap();
        assert!(validate_verdict_file(&dir.join("verdict.json")).is_err());
        fs::write(
            dir.join("verdict.json"),
            r#"{"run_id":"bad-verdict","status":"green","summary":"nope","criteria_met":false,"contract":"contract.md","evidence":[],"note":""}"#,
        )
        .unwrap();
        let err = validate_verdict_file(&dir.join("verdict.json"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("criteria_met"));
    }

    #[test]
    fn run_maintenance_removes_only_owned_terminal_caches() {
        let home = temp_run("run-maintenance-home");
        let runs = home.join("runs");
        let run = runs.join("terminal");
        fs::create_dir_all(run.join("evidence/build/node_modules/pkg")).unwrap();
        fs::create_dir_all(run.join("evidence/report/Caches")).unwrap();
        fs::write(run.join("evidence/build/node_modules/pkg/data"), "cache").unwrap();
        fs::write(run.join("evidence/report/Caches/proof"), "keep").unwrap();
        symlink("/tmp", run.join("evidence/external")).unwrap();
        fs::write(run.join("result.json"), r#"{"status":"green"}"#).unwrap();
        let mut config = Config::default();
        config.home = home;

        let status = maintain_runs_with_retention(&config, None, Duration::ZERO).unwrap();
        assert_eq!(status["removedBytes"], 0);
        assert!(run.join("evidence/build/node_modules").is_dir());

        let result = maintain_runs_with_retention(
            &config,
            Some(StoragePressure::Warning),
            Duration::ZERO,
        )
        .unwrap();
        assert_eq!(result["removedBytes"], 5);
        assert!(!run.join("evidence/build/node_modules").exists());
        assert!(run.join("evidence/report/Caches/proof").is_file());
        assert!(run.join("maintenance.json").is_file());
    }

    /// Writes the directory layouts real iOS runs leave behind under `evidence/`.
    fn write_run_cache_layouts(evidence: &Path) {
        // Xcode build root: a SwiftPM workspace and a DerivedData tree under plain names.
        let xcode = evidence.join("xcode-codex4");
        fs::create_dir_all(xcode.join("packages/checkouts/pkg/Sources")).unwrap();
        fs::create_dir_all(xcode.join("packages/repositories/pkg-abc/objects")).unwrap();
        fs::create_dir_all(xcode.join("packages/repositories/pkg-abc/refs")).unwrap();
        fs::write(xcode.join("packages/workspace-state.json"), "{}").unwrap();
        fs::write(xcode.join("packages/checkouts/pkg/Sources/f.swift"), "s").unwrap();
        fs::write(xcode.join("packages/repositories/pkg-abc/HEAD"), "ref").unwrap();
        fs::create_dir_all(xcode.join("derived/Build")).unwrap();
        fs::write(xcode.join("derived/info.plist"), "p").unwrap();
        fs::write(xcode.join("derived/Build/data"), "b").unwrap();
        // Shared SwiftPM mirror cache: bare clones and nothing else.
        let mirrors = evidence.join("xcode-package-cache/repositories");
        for repo in ["a-1", "b-2"] {
            fs::create_dir_all(mirrors.join(repo).join("objects")).unwrap();
            fs::create_dir_all(mirrors.join(repo).join("refs")).unwrap();
            fs::write(mirrors.join(repo).join("HEAD"), "ref").unwrap();
        }
        // Compiler module cache.
        fs::create_dir_all(evidence.join("module-cache")).unwrap();
        fs::write(evidence.join("module-cache/x.pcm"), "mm").unwrap();
        fs::write(evidence.join("module-cache/modules.timestamp"), "").unwrap();
        // Synthetic home: reachable only through its Library cache children.
        let home = evidence.join("home/Library");
        fs::create_dir_all(home.join("Caches/org.swift.swiftpm")).unwrap();
        fs::create_dir_all(home.join("Developer/Xcode/DerivedData")).unwrap();
        fs::write(home.join("Caches/org.swift.swiftpm/f"), "c").unwrap();
        fs::write(home.join("Developer/Xcode/DerivedData/x"), "d").unwrap();
    }

    /// Writes directories whose names look like caches but hold no cache structure.
    fn write_cache_lookalikes(evidence: &Path) {
        // `derived` that is really a written-up analysis.
        fs::create_dir_all(evidence.join("derived")).unwrap();
        fs::write(evidence.join("derived/notes.md"), "finding").unwrap();
        fs::write(evidence.join("derived/change.diff"), "diff").unwrap();
        // `packages` that records which packages the contract touched.
        fs::create_dir_all(evidence.join("packages/contracts")).unwrap();
        fs::write(evidence.join("packages/index.md"), "list").unwrap();
        fs::write(evidence.join("packages/contracts/result.json"), "{}").unwrap();
        // `repositories` holding source checkouts rather than bare mirrors.
        fs::create_dir_all(evidence.join("checkouts/repositories/app/.git")).unwrap();
        fs::write(evidence.join("checkouts/repositories/app/README.md"), "src").unwrap();
        // Build logs: a `Logs/Build` pair is not a DerivedData root.
        fs::create_dir_all(evidence.join("Logs/Build")).unwrap();
        fs::write(evidence.join("Logs/Build/build.log"), "log").unwrap();
        // A transcript directory that happens to carry one precompiled module.
        fs::create_dir_all(evidence.join("transcripts")).unwrap();
        fs::write(evidence.join("transcripts/a.pcm"), "m").unwrap();
        fs::write(evidence.join("transcripts/ledger.json"), "{}").unwrap();
    }

    #[test]
    fn run_maintenance_reclaims_structural_build_caches() {
        let home = temp_run("run-cache-structure-home");
        let run = home.join("runs/terminal");
        let evidence = run.join("evidence");
        fs::create_dir_all(&evidence).unwrap();
        write_run_cache_layouts(&evidence);
        write_cache_lookalikes(&evidence);
        fs::write(run.join("result.json"), r#"{"status":"green"}"#).unwrap();
        let config = Config {
            home,
            ..Default::default()
        };

        let preview = maintain_runs_with_retention(&config, None, Duration::ZERO).unwrap();
        assert_eq!(preview["removedBytes"], 0);
        assert_eq!(preview["candidates"].as_array().unwrap().len(), 6);
        assert!(evidence.join("xcode-codex4/packages").is_dir());

        let result =
            maintain_runs_with_retention(&config, Some(StoragePressure::Critical), Duration::ZERO)
                .unwrap();
        assert_eq!(result["removedBytes"], 18);
        for reclaimed in [
            "xcode-codex4/packages",
            "xcode-codex4/derived",
            "xcode-package-cache/repositories",
            "module-cache",
            "home/Library/Caches",
            "home/Library/Developer/Xcode/DerivedData",
        ] {
            assert!(
                !evidence.join(reclaimed).exists(),
                "expected reclaim: {reclaimed}"
            );
        }
        for kept in [
            "derived/notes.md",
            "derived/change.diff",
            "packages/index.md",
            "packages/contracts/result.json",
            "checkouts/repositories/app/README.md",
            "Logs/Build/build.log",
            "transcripts/a.pcm",
            "transcripts/ledger.json",
        ] {
            assert!(evidence.join(kept).is_file(), "expected retention: {kept}");
        }
        assert!(run.join("maintenance.json").is_file());
    }

    #[test]
    fn run_cache_recognition_never_follows_symlinks() {
        let run = temp_run("run-cache-symlinks");
        let evidence = run.join("evidence");
        let outside = run.join("outside/packages");
        fs::create_dir_all(outside.join("checkouts")).unwrap();
        fs::write(outside.join("workspace-state.json"), "{}").unwrap();
        symlink(&outside, evidence.join("packages")).unwrap();
        // A mirror set is only recognized when every child is a real bare repository.
        fs::create_dir_all(evidence.join("repositories/real/objects")).unwrap();
        fs::create_dir_all(evidence.join("repositories/real/refs")).unwrap();
        fs::write(evidence.join("repositories/real/HEAD"), "ref").unwrap();
        symlink(&outside, evidence.join("repositories/linked")).unwrap();

        assert!(owned_run_cache_dirs(&evidence).unwrap().is_empty());
        assert!(outside.join("workspace-state.json").is_file());
    }

    #[test]
    fn run_budget_stops_the_child() {
        let run = temp_run("storage-budget");
        let oversized = File::create(run.join("oversized.bin")).unwrap();
        oversized.set_len(RUN_STORAGE_BUDGET_BYTES + 1).unwrap();
        let mut command = ProcessCommand::new("sh");
        command.arg("-c").arg("sleep 10").process_group(0);
        let error = wait_status_with_activity(
            command.spawn().unwrap(),
            Duration::from_secs(10),
            Duration::from_secs(10),
            &run,
            "test",
        )
        .unwrap_err();
        assert!(matches!(error, AppError::StorageBudget(_)));
    }
}
