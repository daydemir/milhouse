use super::*;
use std::os::{
    fd::AsRawFd,
    unix::{fs::PermissionsExt, process::CommandExt},
};

pub(crate) fn run_task(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: TaskInput,
) -> Result<RunOutput, AppError> {
    let _lock = lock(&config.lock_path)?;
    run_task_locked(root, workdir, config, task)
}

pub(crate) fn sanity(
    root: &Path,
    config: &Config,
    args: SanityArgs,
) -> Result<RunOutput, AppError> {
    let base = env::temp_dir().join(format!("{}-{}", args.id, compact_timestamp_local()));
    let workdir = base.join("work");
    fs::create_dir_all(&workdir)?;
    let _cleanup = CleanupDir::new(base.clone(), !args.keep);
    let mut sanity_config = config.clone();
    sanity_config.home = base.join("home");
    sanity_config.lock_path = base.join("milhouse.lock");
    let task = TaskInput {
        id: args.id,
        prompt: args.text,
        metadata: json!({"sanity": true}),
    };
    run_task(root, &workdir, &sanity_config, task)
}

pub(crate) fn plan_contract(
    root: &Path,
    workdir: &Path,
    config: &Config,
    args: PlanArgs,
) -> Result<Value, AppError> {
    let main = args.main.unwrap_or_else(|| config.plan_profile.clone());
    let main_agent = main.details()?;
    let task_summary = args.task.join(" ");
    let task = prompt_with_context(task_summary.clone(), &args.context)?;
    if task.trim().is_empty() {
        return Err(AppError::Input("plan task text is empty".into()));
    }
    let contract = if args.contract.is_absolute() {
        args.contract
    } else {
        workdir.join(args.contract)
    };
    if contract.exists() {
        return Err(AppError::Input(format!(
            "contract already exists: {}",
            contract.display()
        )));
    }
    let parent = contract
        .parent()
        .ok_or_else(|| AppError::Input("contract path has no parent".into()))?;
    fs::create_dir_all(parent)?;

    if args.autonomous {
        let issue = args.issue.as_deref().ok_or_else(|| {
            AppError::Input("--autonomous requires --issue <LINEAR-ISSUE>".into())
        })?;
        return autonomous_plan_contract(
            root,
            workdir,
            config,
            &main,
            issue,
            task.trim(),
            &contract,
        );
    }

    let advisors = if args.advisor.is_empty() {
        "No additional worker profiles were requested.".to_string()
    } else {
        format!(
            "Additional requested worker profiles: {}.",
            args.advisor
                .iter()
                .map(|p| p.id())
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let skill = root.join("skills/mil-plan/SKILL.md");
    let caller = match main_agent.provider {
        AgentProvider::Claude => "claude",
        AgentProvider::Codex => "codex",
    };
    let prompt = planning_coordinator_prompt(
        &skill,
        &planning_session_title(&task_summary),
        task.trim(),
        main.id(),
        caller,
        workdir,
        &advisors,
        &contract,
    );
    if main_agent.provider == AgentProvider::Codex && main_agent.auth == AgentAuth::Api {
        return Err(AppError::Input(
            "mil plan requires a Codex subscription agent; use Agent Bridge directly for Codex API work"
                .into(),
        ));
    }
    let api_key =
        if main_agent.provider == AgentProvider::Claude && main_agent.auth == AgentAuth::Api {
            let key_file = config
                .anthropic_key_file
                .clone()
                .map(Ok)
                .unwrap_or_else(anthropic_key_file)?;
            Some(read_env_key(&key_file, "ANTHROPIC_API_KEY")?)
        } else {
            None
        };
    let status = if main_agent.provider == AgentProvider::Claude {
        run_interactive_claude(workdir, parent, main_agent, api_key.as_deref(), &prompt)?
    } else {
        run_interactive_codex(workdir, parent, main_agent, &prompt)?
    };
    if !status.success() {
        return Err(AppError::External(format!(
            "planning session exited {status}"
        )));
    }
    let (plan_status, contract_output) = planning_result(&contract)?;
    Ok(json!({
        "ok": true,
        "status": plan_status,
        "contract": contract_output,
        "main": main.id(),
        "advisors": args.advisor.iter().map(|p| p.id()).collect::<Vec<_>>()
    }))
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum AutonomousPlanDecision {
    Planned {
        #[serde(default)]
        pursuit_evidence: Option<Value>,
        contract_markdown: String,
    },
    NeedsUser {
        reason_markdown: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
enum AutonomousPlanReview {
    Approved {},
    NeedsUser { reason_markdown: String },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AutonomousPursuitEvidence {
    goal: String,
    current_use_evidence: Vec<CurrentUseEvidence>,
    named_concepts: Vec<NamedConceptEvidence>,
    decision_test: PursuitDecisionTest,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct CurrentUseEvidence {
    source: String,
    observation: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NamedConceptEvidence {
    name: String,
    status: NamedConceptStatus,
    live_concept: String,
    basis: String,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum NamedConceptStatus {
    Current,
    LegacyOrInternal,
    Mixed,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct PursuitDecisionTest {
    pursue_now: String,
    defer: String,
    cancel: String,
    no_build: String,
    delete_or_retire: String,
}

fn validate_autonomous_pursuit_evidence(
    value: Option<&Value>,
) -> Result<AutonomousPursuitEvidence, String> {
    let Some(value) = value else {
        return Err("planned result omitted pursuit_evidence".into());
    };
    let evidence: AutonomousPursuitEvidence = serde_json::from_value(value.clone())
        .map_err(|error| format!("pursuit_evidence has an invalid shape: {error}"))?;

    let mut missing = Vec::new();
    if evidence.goal.trim().is_empty() {
        missing.push("goal");
    }
    if evidence.current_use_evidence.is_empty() {
        missing.push("current_use_evidence");
    }
    if evidence.named_concepts.is_empty() {
        missing.push("named_concepts");
    }
    for item in &evidence.current_use_evidence {
        if item.source.trim().is_empty() || item.observation.trim().is_empty() {
            missing.push("current_use_evidence source/observation");
            break;
        }
    }
    for concept in &evidence.named_concepts {
        if concept.name.trim().is_empty()
            || concept.live_concept.trim().is_empty()
            || concept.basis.trim().is_empty()
        {
            missing.push("named_concepts name/live_concept/basis");
            break;
        }
    }
    for (name, value) in [
        (
            "decision_test.pursue_now",
            &evidence.decision_test.pursue_now,
        ),
        ("decision_test.defer", &evidence.decision_test.defer),
        ("decision_test.cancel", &evidence.decision_test.cancel),
        ("decision_test.no_build", &evidence.decision_test.no_build),
        (
            "decision_test.delete_or_retire",
            &evidence.decision_test.delete_or_retire,
        ),
    ] {
        if value.trim().is_empty() {
            missing.push(name);
        }
    }
    if missing.is_empty() {
        Ok(evidence)
    } else {
        missing.sort_unstable();
        missing.dedup();
        Err(format!("missing substantive {}", missing.join(", ")))
    }
}

fn autonomous_plan_contract(
    root: &Path,
    workdir: &Path,
    config: &Config,
    main: &ModelProfile,
    issue: &str,
    task: &str,
    contract: &Path,
) -> Result<Value, AppError> {
    let issue = issue.trim();
    if issue.is_empty() {
        return Err(AppError::Input("--issue cannot be empty".into()));
    }
    validate_unattended_plan_profile("planner", main)?;
    validate_unattended_plan_profile("reviewer", &config.autoplan_review_profile)?;
    let (run, result) = run_autoplan_agent(
        config,
        workdir,
        main,
        issue,
        "planner",
        &autonomous_plan_prompt(root, issue, task, contract),
    )?;
    match parse_autonomous_plan_decision(&result)? {
        AutonomousPlanDecision::Planned {
            pursuit_evidence,
            contract_markdown,
        } => {
            let pursuit_evidence = match validate_autonomous_pursuit_evidence(
                pursuit_evidence.as_ref(),
            ) {
                Ok(evidence) => evidence,
                Err(reason) => {
                    return Ok(json!({
                        "ok": true,
                        "status": "needs_user",
                        "contract": Value::Null,
                        "reason": format!(
                            "Autonomous pursuit evidence gate did not pass: {reason}. Guided Mil Plan must establish current use and decide pursue, defer, or cancel before a contract can be produced."
                        ),
                        "main": main.id(),
                        "run": run["run"],
                    }));
                }
            };
            let contract_markdown = canonicalize_autonomous_contract(&contract_markdown);
            validate_build_contract_text(&contract_markdown)?;
            let reviewer = &config.autoplan_review_profile;
            let (review_run, review) = run_autoplan_agent(
                config,
                workdir,
                reviewer,
                issue,
                "reviewer",
                &autonomous_review_prompt(
                    root,
                    issue,
                    task,
                    &serde_json::to_string_pretty(&pursuit_evidence)?,
                    &contract_markdown,
                ),
            )?;
            if let AutonomousPlanReview::NeedsUser { reason_markdown } =
                parse_autonomous_plan_review(&review)?
            {
                if reason_markdown.trim().is_empty() {
                    return Err(AppError::Input(
                        "autonomous reviewer returned an empty needs-user reason".into(),
                    ));
                }
                return Ok(json!({
                    "ok": true,
                    "status": "needs_user",
                    "contract": Value::Null,
                    "reason": reason_markdown.trim(),
                    "main": main.id(),
                    "reviewer": reviewer.id(),
                    "run": run["run"],
                    "review_run": review_run["run"],
                }));
            }
            let markdown = contract_markdown.trim_end().to_string() + "\n";
            let source_hash = format!("{:016x}", seahash::hash(markdown.as_bytes()));
            let html = contract_html(&markdown, &contract.display().to_string(), &source_hash);
            let temporary = contract.with_extension("md.tmp");
            let html_path = contract.with_extension("html");
            let temporary_html = contract.with_extension("html.tmp");
            for (path, contents) in [
                (&temporary, markdown.as_bytes()),
                (&temporary_html, html.as_bytes()),
            ] {
                fs::write(path, contents)?;
                fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
                File::open(path)?.sync_all()?;
            }
            fs::rename(&temporary_html, html_path)?;
            fs::rename(&temporary, contract)?;
            Ok(json!({
                "ok": true,
                "status": "planned",
                "contract": contract,
                "main": main.id(),
                "reviewer": reviewer.id(),
                "run": run["run"],
                "review_run": review_run["run"],
            }))
        }
        AutonomousPlanDecision::NeedsUser { reason_markdown } => {
            if reason_markdown.trim().is_empty() {
                return Err(AppError::Input(
                    "autonomous planner returned an empty needs-user reason".into(),
                ));
            }
            Ok(json!({
                "ok": true,
                "status": "needs_user",
                "contract": Value::Null,
                "reason": reason_markdown.trim(),
                "main": main.id(),
                "run": run["run"],
            }))
        }
    }
}

/// Repair markdown-only drift from autonomous planners before applying the strict contract gate.
///
/// The acceptance grammar is intentionally strict, but list markers carry no planning judgment.
/// Models occasionally emit the exact criterion/proof/expect nodes without their Markdown bullets;
/// canonicalizing those markers keeps a semantically complete plan from becoming a human incident.
fn canonicalize_autonomous_contract(contract: &str) -> String {
    let mut in_acceptance = false;
    contract
        .lines()
        .map(|line| {
            if line == "## Acceptance" {
                in_acceptance = true;
                return line.to_string();
            }
            if in_acceptance && line.starts_with("## ") {
                in_acceptance = false;
            }
            if !in_acceptance {
                return line.to_string();
            }

            let trimmed = line.trim_start();
            if !trimmed.starts_with("- ") {
                let candidate = format!("- {trimmed}");
                if parse_criterion_marker(&candidate).is_some() {
                    return format!("- {trimmed}");
                }
                if ["proof:", "expect:", "builder detail:"]
                    .iter()
                    .any(|marker| trimmed.starts_with(marker))
                {
                    return format!("  - {trimmed}");
                }
            }
            line.to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn validate_unattended_plan_profile(role: &str, profile: &ModelProfile) -> Result<(), AppError> {
    let details = profile.details()?;
    if profile.id() != "opus-5-5-high-subscription"
        || details.provider != AgentProvider::Claude
        || details.auth != AgentAuth::Subscription
    {
        return Err(AppError::Input(format!(
            "autonomous {role} must use opus-5-5-high-subscription; manual profiles are not unattended defaults: {}",
            profile.id()
        )));
    }
    Ok(())
}

fn run_autoplan_agent(
    config: &Config,
    workdir: &Path,
    profile: &ModelProfile,
    issue: &str,
    role: &str,
    prompt: &str,
) -> Result<(Value, String), AppError> {
    let caller = match profile.details()?.provider {
        AgentProvider::Claude => "claude",
        AgentProvider::Codex => "codex",
    };
    let user = env::var("MIL_AGENT_USER")
        .or_else(|_| env::var("USER"))
        .map_err(|_| AppError::Input("set MIL_AGENT_USER for the planning runner".into()))?;
    let prompt_dir = config.home.join("autonomous-plans");
    fs::create_dir_all(&prompt_dir)?;
    fs::set_permissions(&prompt_dir, fs::Permissions::from_mode(0o700))?;
    let prompt_path = prompt_dir.join(format!("{issue}-{role}-{}.md", epoch()));
    fs::write(&prompt_path, prompt)?;
    fs::set_permissions(&prompt_path, fs::Permissions::from_mode(0o600))?;
    let output = ProcessCommand::new("rolly")
        .args(["agent", "run", "--agent", profile.id(), "--mode", "read"])
        .args(["--caller", caller, "--user", user.as_str(), "--workdir"])
        .arg(workdir)
        .arg("--prompt")
        .arg(&prompt_path)
        .args(["--issue", issue])
        .output();
    let _ = fs::remove_file(&prompt_path);
    let output = output.map_err(|error| {
        AppError::External(format!(
            "autonomous {role} requires the Rolly CLI for logged agent execution: {error}"
        ))
    })?;
    if !output.status.success() {
        return Err(AppError::External(format!(
            "autonomous {role} exited {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let run: Value = serde_json::from_slice(&output.stdout).map_err(|error| {
        AppError::External(format!(
            "Rolly autonomous {role} returned invalid JSON: {error}"
        ))
    })?;
    let result_path = run["result"]
        .as_str()
        .ok_or_else(|| AppError::Invariant("Rolly agent result path missing".into()))?
        .to_string();
    Ok((run, fs::read_to_string(result_path)?))
}

fn parse_autonomous_plan_decision(text: &str) -> Result<AutonomousPlanDecision, AppError> {
    serde_json::from_str(text.trim()).map_err(|error| {
        AppError::Input(format!(
            "autonomous planner must return exactly one decision JSON object: {error}"
        ))
    })
}

fn parse_autonomous_plan_review(text: &str) -> Result<AutonomousPlanReview, AppError> {
    serde_json::from_str(text.trim()).map_err(|error| {
        AppError::Input(format!(
            "autonomous reviewer must return exactly one decision JSON object: {error}"
        ))
    })
}

fn autonomous_plan_prompt(root: &Path, issue: &str, task: &str, contract: &Path) -> String {
    format!(
        r#"Role: autonomous small-issue planner.

Inspect Linear issue {issue}, its relevant repository context, and the binding task below. Do not implement, delegate, mutate Linear, ask the user a live question, or create a Wayfinder map or child issues.

First read and pass the pursuit and current-use evidence gate at {pursuit_evidence_format}. Then decide whether this issue is safe for autonomous planning. It must be genuinely small and self-contained: one normal builder session, an obvious intended behavior, no unresolved product/UX/architecture/scope choice, no prototype, research, external login, or human evidence needed. When in doubt, stop for guided planning.

If safe, write the complete acceptance contract described by {contract_format}. It must stand alone for a builder that has not seen this planning run, preserve the issue's intended scope, name verification, and include acceptance criteria. Return the exact `planned` JSON shape specified by the pursuit evidence gate, with the complete contract in `contract_markdown`.

If any judgment, missing evidence, ambiguity, access, research, prototype, or larger decomposition is needed, return a short durable explanation and the smallest concrete questions or next step. Return exactly:
{{"state":"needs_user","reason_markdown":"<why guided Mil Plan is needed>"}}

No markdown fences or text outside that single JSON object.

Issue: {issue}
Requested task: {task}
Intended contract path (do not write it yourself): {contract}
"#,
        contract_format = root
            .join("skills/mil-plan/references/contract-format.md")
            .display(),
        pursuit_evidence_format = root
            .join("skills/mil-plan/references/pursuit-evidence.md")
            .display(),
        contract = contract.display(),
    )
}

fn autonomous_review_prompt(
    root: &Path,
    issue: &str,
    task: &str,
    pursuit_evidence: &str,
    contract: &str,
) -> String {
    format!(
        r#"Role: independent autonomous-plan reviewer.

Inspect Linear issue {issue}, relevant repository context, the requested task, and the proposed contract below. Work read-only. Do not implement, delegate, mutate Linear, ask the user, repair the contract, or invent missing decisions.

Independently apply the pursuit and current-use evidence gate at {pursuit_evidence_format}; verify the proposed evidence against the task and repository instead of accepting it by assertion. Approve only if that gate passes, this is genuinely small and self-contained, needs no product/UX/architecture/scope/research/prototype/access judgment, and the proposed contract fully satisfies {contract_format} with concrete acceptance criteria and verification. When in doubt, require guided planning.

If safe and complete, return exactly:
{{"state":"approved"}}

Otherwise return exactly:
{{"state":"needs_user","reason_markdown":"<short durable reason and smallest questions or next step>"}}

No markdown fences or text outside that single JSON object.

Issue: {issue}
Requested task: {task}

Proposed pursuit evidence:
{pursuit_evidence}

Proposed contract:
{contract}
"#,
        contract_format = root
            .join("skills/mil-plan/references/contract-format.md")
            .display(),
        pursuit_evidence_format = root
            .join("skills/mil-plan/references/pursuit-evidence.md")
            .display(),
    )
}

#[allow(clippy::too_many_arguments)]
fn planning_coordinator_prompt(
    skill: &Path,
    session_title: &str,
    task: &str,
    profile: &str,
    caller: &str,
    workdir: &Path,
    advisors: &str,
    contract: &Path,
) -> String {
    format!(
        "{}\n\nRole: coordinator. Do not perform planning judgment yourself. Read and follow the skill at {} and its referenced files exactly.\n\nTask: {}\n\nUse `rolly agent` for every mapper, ticket thinker, research worker, contract writer, and critic. Default worker profile: {}; prototype routing may override it exactly as orchestration.md specifies. Worker caller: {}. Worker workdir: {}. {}\n\nKeep this one user conversation active across the entire map. Relay worker questions verbatim and continue while any worker or question is actionable. Use {} as the final contract path only when the Wayfinder map is clear; stop earlier only when the user exits, every worker is blocked, or the human accepts a documented cancel/defer recommendation.",
        session_title,
        skill.display(),
        task,
        profile,
        caller,
        workdir.display(),
        advisors,
        contract.display()
    )
}

fn planning_session_title(task: &str) -> String {
    let normalized = task.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = normalized.chars();
    let brief = chars.by_ref().take(52).collect::<String>();
    format!(
        "Plan · {}{}",
        brief.trim_end(),
        if chars.next().is_some() { "…" } else { "" }
    )
}

pub(crate) fn advise(workdir: &Path, config: &Config, args: AdviseArgs) -> Result<Value, AppError> {
    let profile = args.profile.details()?;
    let mut cmd = ProcessCommand::new(agent_bridge_bin().map_err(AppError::External)?);
    cmd.args(["run", "--agent", args.profile.id(), "--mode", "read"])
        .arg("--workdir")
        .arg(workdir)
        .arg("--prompt")
        .arg(&args.prompt)
        .arg("--output")
        .arg(&args.output);
    if profile.provider == AgentProvider::Claude && profile.auth == AgentAuth::Api {
        let key_file = config
            .anthropic_key_file
            .clone()
            .map(Ok)
            .unwrap_or_else(anthropic_key_file)?;
        cmd.env(
            "ANTHROPIC_API_KEY",
            read_env_key(&key_file, "ANTHROPIC_API_KEY")?,
        );
    }
    let result = cmd.output()?;
    if !result.status.success() {
        return Err(AppError::External(format!(
            "agent-bridge advisor exited {}: {}",
            result.status,
            String::from_utf8_lossy(&result.stderr).trim()
        )));
    }
    Ok(serde_json::from_slice(&result.stdout)?)
}

fn run_interactive_claude(
    workdir: &Path,
    contract_parent: &Path,
    profile: &AgentProfile,
    api_key: Option<&str>,
    prompt: &str,
) -> Result<ExitStatus, AppError> {
    let mut cmd = ProcessCommand::new("claude");
    cmd.current_dir(workdir)
        .args(interactive_claude_args(contract_parent, profile, prompt));
    for key in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDECODE",
    ] {
        cmd.env_remove(key);
    }
    if profile.auth == AgentAuth::Api {
        cmd.env(
            "ANTHROPIC_API_KEY",
            api_key.ok_or_else(|| AppError::Invariant("missing Fable API key".into()))?,
        );
    }
    Ok(cmd.status()?)
}

fn interactive_claude_args(
    contract_parent: &Path,
    profile: &AgentProfile,
    prompt: &str,
) -> Vec<std::ffi::OsString> {
    [
        "--model".into(),
        profile.model.clone().into(),
        "--effort".into(),
        profile.effort.as_str().into(),
        "--add-dir".into(),
        contract_parent.as_os_str().to_owned(),
        "--".into(),
        prompt.into(),
    ]
    .into()
}

fn run_interactive_codex(
    workdir: &Path,
    contract_parent: &Path,
    profile: &AgentProfile,
    prompt: &str,
) -> Result<ExitStatus, AppError> {
    let mut cmd = ProcessCommand::new("codex");
    cmd.current_dir(workdir).args(interactive_codex_args(
        workdir,
        contract_parent,
        profile,
        prompt,
    ));
    for key in ["OPENAI_API_KEY", "OPENAI_BASE_URL", "CODEX_API_KEY"] {
        cmd.env_remove(key);
    }
    Ok(cmd.status()?)
}

fn interactive_codex_args(
    workdir: &Path,
    contract_parent: &Path,
    profile: &AgentProfile,
    prompt: &str,
) -> Vec<std::ffi::OsString> {
    vec![
        "--cd".into(),
        workdir.as_os_str().to_owned(),
        "--add-dir".into(),
        contract_parent.as_os_str().to_owned(),
        "--model".into(),
        profile.model.clone().into(),
        "--config".into(),
        format!("model_reasoning_effort=\"{}\"", profile.effort.as_str()).into(),
        "--search".into(),
        "--".into(),
        prompt.into(),
    ]
}

fn planning_result(contract: &Path) -> Result<(&'static str, Value), AppError> {
    if contract.exists() {
        render_contract_file(contract, None)?;
        Ok(("planned", json!(contract)))
    } else {
        Ok(("planning", Value::Null))
    }
}

fn run_task_locked(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: TaskInput,
) -> Result<RunOutput, AppError> {
    validate_task(task.clone())?;
    let run_dir = init_run_dir(config, workdir, &task, None, None)?;
    write_frozen_contract(&run_dir, &task.prompt)?;
    let status = run_owned(root, workdir, config, &task, &run_dir)?;
    let result_path = run_dir.join("result.json");
    let result: Value = serde_json::from_str(&fs::read_to_string(&result_path)?)?;
    Ok(RunOutput {
        id: task.id,
        status: status.as_str().to_string(),
        run_dir,
        result_path,
        grade_score: result.get("grade_score").and_then(Value::as_f64),
        cost: result.get("claude_cost_usd").and_then(Value::as_f64),
        metadata: task.metadata,
    })
}

struct CleanupDir {
    path: PathBuf,
    enabled: bool,
}

impl CleanupDir {
    fn new(path: PathBuf, enabled: bool) -> Self {
        Self { path, enabled }
    }
}

impl Drop for CleanupDir {
    fn drop(&mut self) {
        if self.enabled {
            let _ = fs::remove_dir_all(&self.path);
        }
    }
}

pub(crate) fn resume_task(
    root: &Path,
    config: &Config,
    args: ResumeArgs,
) -> Result<RunOutput, AppError> {
    let _lock = lock(&config.lock_path)?;
    let old_run_dir = resolve_run(&config.home, &args.run)?;
    let old = read_run_spec(&old_run_dir)?;
    let workdir = required_dir(old.workdir.clone(), "prior run workdir")?;
    let mode = ResumeMode::ReuseContract;
    let extra = resume_extra(args.prompt, args.text)?;
    let task = TaskInput {
        id: args
            .id
            .unwrap_or_else(|| format!("resume-{}-{}", old.id, epoch())),
        prompt: resume_prompt(&old_run_dir, &old, mode, extra.as_deref()),
        metadata: json!({"resume_from": old_run_dir, "resume_mode": mode}),
    };
    validate_task(task.clone())?;
    let run_dir = init_run_dir(
        config,
        &workdir,
        &task,
        Some(old_run_dir.clone()),
        Some(mode),
    )?;
    seed_resume_history(&run_dir, &old_run_dir, mode, extra.as_deref())?;
    seed_reused_contract(&old_run_dir, &run_dir)?;
    record_continuation(&old_run_dir, &run_dir)?;
    let status = run_owned(root, &workdir, config, &task, &run_dir)?;
    let result_path = run_dir.join("result.json");
    let result: Value = serde_json::from_str(&fs::read_to_string(&result_path)?)?;
    Ok(RunOutput {
        id: task.id,
        status: status.as_str().to_string(),
        run_dir,
        result_path,
        grade_score: result.get("grade_score").and_then(Value::as_f64),
        cost: result.get("claude_cost_usd").and_then(Value::as_f64),
        metadata: task.metadata,
    })
}

// Hidden durability seam used by Rolly autopilot before the long build process.
pub(crate) fn prepare_task(
    _root: &Path,
    workdir: &Path,
    config: &Config,
    task: TaskInput,
) -> Result<RunOutput, AppError> {
    let _lock = lock(&config.lock_path)?;
    validate_task(task.clone())?;
    let run_dir = init_run_dir(config, workdir, &task, None, None)?;
    write_frozen_contract(&run_dir, &task.prompt)?;
    let _ = fs::remove_file(run_dir.join("run.pid"));
    write_log(
        &run_dir,
        &format!(
            "prepared: contract frozen, awaiting execute in {}",
            run_dir.display()
        ),
    )?;
    sync_run_state(&run_dir)?;
    Ok(RunOutput {
        id: task.id,
        status: "planned".into(),
        run_dir: run_dir.clone(),
        result_path: run_dir.join("result.json"),
        grade_score: None,
        cost: None,
        metadata: task.metadata,
    })
}

// Hidden continuation of a prepared contract used by Rolly autopilot.
pub(crate) fn execute_run(root: &Path, config: &Config, run: &str) -> Result<RunOutput, AppError> {
    let _lock = lock(&config.lock_path)?;
    let run_dir = if run == "latest" {
        latest_run(&config.home)?
    } else {
        resolve_run(&config.home, run)?
    };
    let spec = read_run_spec(&run_dir)?;
    let mut run_config = config.clone();
    run_config.build_profile = spec.build_profile.clone();
    run_config.verify_profile = spec.verify_profile.clone();
    let workdir = required_dir(spec.workdir.clone(), "run workdir")?;
    if run_dir.join("sentinel").is_file() {
        return Err(AppError::Input(format!(
            "run already terminal: {}",
            run_dir.display()
        )));
    }
    let task = TaskInput {
        id: spec.id.clone(),
        prompt: spec.prompt.clone(),
        metadata: json!({}),
    };
    let status = run_owned(root, &workdir, &run_config, &task, &run_dir)?;
    let result_path = run_dir.join("result.json");
    let result: Value = serde_json::from_str(&fs::read_to_string(&result_path)?)?;
    Ok(RunOutput {
        id: task.id,
        status: status.as_str().to_string(),
        run_dir,
        result_path,
        grade_score: result.get("grade_score").and_then(Value::as_f64),
        cost: result.get("claude_cost_usd").and_then(Value::as_f64),
        metadata: task.metadata,
    })
}

fn latest_run(home: &Path) -> Result<PathBuf, AppError> {
    let mut runs: Vec<PathBuf> = fs::read_dir(home.join("runs"))?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.join("run.json").is_file())
        .collect();
    runs.sort();
    runs.pop()
        .ok_or_else(|| AppError::Input("no runs found".into()))
}

fn init_run_dir(
    config: &Config,
    workdir: &Path,
    task: &TaskInput,
    resume_from: Option<PathBuf>,
    resume_mode: Option<ResumeMode>,
) -> Result<PathBuf, AppError> {
    fs::create_dir_all(config.home.join("runs"))?;
    let short_id = next_short_id(&config.home)?;
    let run_dir =
        config
            .home
            .join("runs")
            .join(format!("{}-{}", compact_timestamp_local(), task.id));
    fs::create_dir_all(run_dir.join("evidence"))?;
    let run_dir = run_dir.canonicalize()?;
    fs::write(run_dir.join("related-cards.json"), "[]\n")?;
    let spec = RunSpec {
        id: task.id.clone(),
        short_id: Some(short_id),
        run_dir: run_dir.clone(),
        workdir: workdir.to_path_buf(),
        prompt: task.prompt.clone(),
        build_profile: config.build_profile.clone(),
        verify_profile: config.verify_profile.clone(),
        resume_from,
        resume_mode,
    };
    fs::write(
        run_dir.join("run.json"),
        serde_json::to_string_pretty(&spec)? + "\n",
    )?;
    read_run_spec(&run_dir)?;
    append_run_index(&config.home, &spec)?;
    Ok(run_dir)
}

fn next_short_id(home: &Path) -> Result<u64, AppError> {
    fs::create_dir_all(home)?;
    let path = home.join("run-counter.json");
    let current = read_json_value(&path)
        .map(|v| {
            v.get("last")
                .and_then(Value::as_u64)
                .ok_or_else(|| AppError::Input(format!("invalid {}", path.display())))
        })
        .transpose()?
        .unwrap_or_else(|| existing_run_count(home));
    let next = current + 1;
    atomic_write(
        &path,
        &(serde_json::to_string_pretty(&json!({"last": next}))? + "\n"),
    )?;
    Ok(next)
}

fn existing_run_count(home: &Path) -> u64 {
    fs::read_dir(home.join("runs"))
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|e| e.path().is_dir())
                .count() as u64
        })
        .unwrap_or(0)
}

fn append_run_index(home: &Path, spec: &RunSpec) -> Result<(), AppError> {
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(home.join("run-index.jsonl"))?;
    writeln!(
        file,
        "{}",
        serde_json::to_string(&json!({
            "short_id": spec.short_id,
            "id": spec.id,
            "run_dir": spec.run_dir,
            "workdir": spec.workdir,
            "created_at": iso_now()
        }))?
    )?;
    Ok(())
}

fn resume_extra(prompt: Option<PathBuf>, text: Option<String>) -> Result<Option<String>, AppError> {
    match (prompt, text) {
        (Some(path), None) => Ok(Some(fs::read_to_string(path)?)),
        (None, Some(text)) => Ok(Some(text)),
        (None, None) => Ok(None),
        _ => Err(AppError::Input(
            "use at most one of --prompt or --text".into(),
        )),
    }
}

fn resume_prompt(
    old_run_dir: &Path,
    old: &RunSpec,
    mode: ResumeMode,
    extra: Option<&str>,
) -> String {
    let mode_text = match mode {
        ResumeMode::ReuseContract => {
            "Reuse the prior contract.md exactly. Continue execution; do not redo completed work."
        }
    };
    let mut prompt = format!(
        "Resume prior Milhouse run:\n\n{}\n\nOriginal prompt:\n{}\n\n{mode_text}\n",
        old_run_dir.display(),
        old.prompt
    );
    if let Some(extra) = extra.map(str::trim).filter(|s| !s.is_empty()) {
        prompt.push_str("\nAdditional resume instructions:\n");
        prompt.push_str(extra);
        prompt.push('\n');
    }
    prompt
}

fn seed_resume_history(
    run_dir: &Path,
    old_run_dir: &Path,
    mode: ResumeMode,
    extra: Option<&str>,
) -> Result<(), AppError> {
    let prior_status = read_json_value(&old_run_dir.join("result.json"))
        .and_then(|v| v.get("status").and_then(Value::as_str).map(str::to_string))
        .or_else(|| {
            read_text(&old_run_dir.join("sentinel"))
                .ok()
                .map(|s| s.trim().into())
        })
        .unwrap_or_else(|| "unknown".into());
    let old_history = tail_lines(&old_run_dir.join("run-history.md"), 200).join("\n");
    let mut text = format!(
        "# Run History\n\n## Resume Context\nprior_run: {}\nprior_status: {}\nresume_mode: {:?}\nprior_evidence: {}/evidence\n\n",
        old_run_dir.display(),
        prior_status,
        mode,
        old_run_dir.display()
    );
    if !old_history.is_empty() {
        text.push_str("## Prior Run History Tail\n");
        text.push_str(&old_history);
        text.push_str("\n\n");
    }
    if let Some(extra) = extra.map(str::trim).filter(|s| !s.is_empty()) {
        text.push_str("## Additional Resume Instructions\n");
        text.push_str(extra);
        text.push_str("\n\n");
    }
    fs::write(run_dir.join("run-history.md"), text).map_err(AppError::Io)
}

fn seed_reused_contract(old_run_dir: &Path, run_dir: &Path) -> Result<(), AppError> {
    fs::copy(old_run_dir.join("contract.md"), run_dir.join("contract.md"))?;
    validate_frozen_contract(run_dir).map(|_| ())
}

fn write_frozen_contract(run_dir: &Path, text: &str) -> Result<(), AppError> {
    validate_contract_text(text)?;
    fs::write(run_dir.join("contract.md"), text)?;
    Ok(())
}

fn record_continuation(old_run_dir: &Path, run_dir: &Path) -> Result<(), AppError> {
    sync_run_state(run_dir)?;
    let path = old_run_dir.join("continuation.json");
    durable_atomic_write(
        &path,
        &(serde_json::to_string_pretty(&json!({ "run_dir": run_dir }))? + "\n"),
    )
}

fn sync_run_state(run_dir: &Path) -> Result<(), AppError> {
    for entry in fs::read_dir(run_dir)? {
        let path = entry?.path();
        if path.is_file() {
            File::open(path)?.sync_all()?;
        }
    }
    File::open(run_dir)?.sync_all()?;
    if let Some(parent) = run_dir.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn durable_atomic_write(path: &Path, text: &str) -> Result<(), AppError> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, text)?;
    File::open(&tmp)?.sync_all()?;
    fs::rename(tmp, path)?;
    if let Some(parent) = path.parent() {
        File::open(parent)?.sync_all()?;
    }
    Ok(())
}

fn run_engine(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
) -> Result<RunStatus, AppError> {
    write_log(
        run_dir,
        &format!(
            "run {} variant=milhouse-rust contract=contract.md executor={} run_dir={}",
            task.id,
            config.build_profile.id(),
            run_dir.display()
        ),
    )?;
    if let Ok(stub) = env::var("MILHOUSE_ENGINE_STUB") {
        fs::copy(stub, run_dir.join("verdict.json"))?;
        return finalize_run(root, workdir, config, task, run_dir, RunCost::default(), 0);
    }
    let cost = match execute_contract(root, workdir, config, task, run_dir) {
        Ok(cost) => cost,
        Err(error @ (AppError::ProcessesAlive(_) | AppError::ProcessIdentityUncertain(_))) => {
            return Err(error)
        }
        Err(AppError::Stale(reason)) => {
            write_log(run_dir, &format!("engine stalled: {reason}"))?;
            return finalize_stale_run(config, task, run_dir, &reason);
        }
        Err(AppError::StorageBudget(reason)) => {
            write_log(
                run_dir,
                &format!("engine storage budget exceeded: {reason}"),
            )?;
            return finalize_storage_budget_run(config, task, run_dir, &reason);
        }
        Err(err) => {
            write_log(run_dir, &format!("engine failed: {err}"))?;
            return finalize_run(root, workdir, config, task, run_dir, RunCost::default(), 0);
        }
    };
    finalize_run(root, workdir, config, task, run_dir, cost, 0)
}

fn execute_contract(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
) -> Result<RunCost, AppError> {
    let mut cost = RunCost::default();
    let contract_hash = validate_frozen_contract(run_dir)?;
    init_run_history(run_dir)?;
    if run_dir.join("verdict.json").exists() {
        return Err(AppError::External(
            "contract preparation wrote verdict.json; refusing to execute builder".into(),
        ));
    }
    run_codex_loop(root, workdir, config, task, run_dir, &cost)?;
    cost.codex = Some(0.0);
    write_progress(root, config, task, run_dir, &cost, "finalizing", 0)?;
    ensure_contract_unchanged(run_dir, contract_hash)?;
    if let Err(err) = validate_verdict_file(&run_dir.join("verdict.json")) {
        write_log(
            run_dir,
            &format!("no valid verdict after codex phase: {err}"),
        )?;
    }
    Ok(cost)
}

fn run_codex_loop(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
    cost: &RunCost,
) -> Result<(), AppError> {
    let builder = config.build_profile.details()?;
    let max = env::var("MILHOUSE_CODEX_MAX")
        .ok()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(20);
    let mut prev_blocker: Option<BlockerFingerprint> = None;
    for attempt in 1..=max {
        let prompt = build_codex_loop_prompt(root, run_dir, attempt)?;
        let prompt_path = run_dir.join(format!("codex-loop-{attempt}.md"));
        let jsonl_path = run_dir.join(format!("codex-session-{attempt}.jsonl"));
        let stderr_path = run_dir.join(format!("codex-stderr-{attempt}.log"));
        let last_path = run_dir.join(format!("codex-output-{attempt}.txt"));
        fs::write(&prompt_path, prompt)?;
        write_progress(root, config, task, run_dir, cost, "codex_loop", attempt)?;
        let status = run_codex_exec(
            workdir,
            &builder.model,
            builder.effort.as_str(),
            &prompt_path,
            &jsonl_path,
            &stderr_path,
            &last_path,
        )?;
        if !status.success() {
            if transient_codex_failure(&jsonl_path, &stderr_path, &last_path) {
                append_run_history(
                    run_dir,
                    &format!(
                        "## Harness {attempt}\nCodex hit a transient provider-capacity error after partial work. Continue from the current workdir state.\n\n"
                    ),
                )?;
                continue;
            }
            return Err(AppError::External(format!("codex exited {status}")));
        }
        let verifier = run_verifier(root, workdir, config, run_dir, attempt).map_err(|err| {
            append_run_history(
                run_dir,
                &format!("## Verifier {attempt}\nstatus: infrastructure_error\nnotes: {err}\n\n"),
            )
            .ok();
            AppError::External(format!(
                "verifier infrastructure failed; builder was not resumed: {err}"
            ))
        })?;
        write_progress(root, config, task, run_dir, cost, "verifier", attempt)?;
        if verifier.passed {
            match write_green_verdict(run_dir) {
                Ok(()) => return Ok(()),
                Err(err) => append_run_history(
                    run_dir,
                    &format!(
                        "## Harness {attempt}\nVerifier passed, but Milhouse could not write a valid green verdict: {err}\n\n"
                    ),
                )?,
            }
        } else {
            // Same unfixable external blocker (missing credential, external wait, absent
            // decision) twice in a row means grinding the remaining attempts is pointless —
            // stop for a human instead of burning them all, then burning them again on resume.
            let blocker = blocker_fingerprint(&verifier.notes);
            if prev_blocker
                .as_ref()
                .is_some_and(|previous| blockers_equivalent(previous, &blocker))
            {
                write_needs_user_verdict(run_dir, verifier.notes.trim().to_string())?;
                return Ok(());
            }
            prev_blocker = Some(blocker);
        }
    }
    write_stopped_verdict(
        run_dir,
        format!("Builder loop hit the {max} attempt limit before the verifier passed contract.md."),
    )?;
    Ok(())
}

fn run_codex_exec(
    workdir: &Path,
    codex_model: &str,
    effort: &str,
    prompt: &Path,
    jsonl: &Path,
    stderr: &Path,
    last: &Path,
) -> Result<std::process::ExitStatus, AppError> {
    let mut cmd = codex_cmd(workdir);
    cmd.args(["exec", "--cd"])
        .arg(workdir)
        .args(codex_common_args(last, codex_model, effort, true))
        .arg("-");
    run_codex_child(cmd, prompt, jsonl, stderr, last.parent())
}

pub(crate) fn codex_once(
    workdir: &Path,
    codex_model: &str,
    prompt: &Path,
    output: &Path,
) -> Result<Value, AppError> {
    let prompt_text = fs::read_to_string(prompt)
        .map_err(|e| AppError::Input(format!("invalid --prompt {}: {e}", prompt.display())))?;
    if prompt_text.trim().is_empty() {
        return Err(AppError::Input("--prompt is empty".into()));
    }
    if let Some(parent) = output.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let jsonl = sibling_with_suffix(output, ".jsonl");
    let stderr = sibling_with_suffix(output, ".stderr.log");
    let mut cmd = codex_cmd(workdir);
    cmd.args(["exec", "--cd"])
        .arg(workdir)
        .args(codex_common_args(output, codex_model, "high", true))
        .arg("-");
    let status = run_codex_child(cmd, prompt, &jsonl, &stderr, None)?;
    if !status.success() {
        return Err(AppError::External(format!(
            "codex exited with status {status}; stderr: {}",
            stderr.display()
        )));
    }
    Ok(json!({
        "ok": true,
        "model": codex_model,
        "workdir": workdir,
        "prompt": prompt,
        "output": output,
        "jsonl": jsonl,
        "stderr": stderr
    }))
}

fn sibling_with_suffix(path: &Path, suffix: &str) -> PathBuf {
    PathBuf::from(format!("{}{}", path.display(), suffix))
}

fn codex_cmd(workdir: &Path) -> ProcessCommand {
    let mut cmd = ProcessCommand::new("env");
    cmd.current_dir(workdir).args([
        "-u",
        "OPENAI_API_KEY",
        "-u",
        "OPENAI_BASE_URL",
        "-u",
        "CODEX_API_KEY",
        "-u",
        "ANTHROPIC_API_KEY",
        "codex",
    ]);
    cmd
}

fn codex_common_args(last: &Path, codex_model: &str, effort: &str, writable: bool) -> Vec<String> {
    codex_common_args_with_isolation(
        last,
        codex_model,
        effort,
        writable,
        std::env::var("MILHOUSE_ISOLATED_BUILD").ok().as_deref() == Some("1"),
    )
}

fn codex_common_args_with_isolation(
    last: &Path,
    codex_model: &str,
    effort: &str,
    writable: bool,
    isolated: bool,
) -> Vec<String> {
    let mut args = vec![
        "-m".into(),
        codex_model.into(),
        "-c".into(),
        format!("model_reasoning_effort=\"{effort}\""),
        "--skip-git-repo-check".into(),
    ];
    if writable && isolated {
        args.extend([
            "--sandbox".into(),
            "workspace-write".into(),
            "-c".into(),
            "sandbox_workspace_write.network_access=true".into(),
        ]);
        if let Some(run_dir) = last.parent() {
            args.extend(["--add-dir".into(), run_dir.display().to_string()]);
        }
    } else if writable {
        args.push("--dangerously-bypass-approvals-and-sandbox".into());
    } else {
        args.extend(["--sandbox".into(), "read-only".into()]);
    }
    args.extend(["--json".into(), "-o".into(), last.display().to_string()]);
    args
}

fn run_codex_child(
    mut cmd: ProcessCommand,
    prompt: &Path,
    jsonl: &Path,
    stderr: &Path,
    active_run: Option<&Path>,
) -> Result<std::process::ExitStatus, AppError> {
    let prompt = fs::read(prompt)?;
    let stdout = File::create(jsonl)?;
    let stderr = File::create(stderr)?;
    let active_child = active_run
        .map(|run_dir| prepare_active_child(&mut cmd, run_dir))
        .transpose()?;
    if active_child.is_none() {
        cmd.process_group(0);
    }
    let mut child = match cmd
        .stdin(Stdio::piped())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            cleanup_active_child(active_run);
            return Err(error.into());
        }
    };
    drop(active_child);
    let child_pid = child.id() as i32;
    let prompt_write = child
        .stdin
        .take()
        .ok_or_else(|| AppError::External("missing codex stdin".into()))
        .and_then(|mut stdin| stdin.write_all(&prompt).map_err(AppError::from));
    if let Err(error) = prompt_write {
        stop_child(&mut child);
        let stopped = stop_tracked_child(active_run, child_pid);
        if stopped.is_ok() {
            cleanup_active_child(active_run);
        }
        stopped?;
        return Err(error);
    }
    let result = wait_status_with_activity(
        child,
        child_timeout("MILHOUSE_CODEX_TIMEOUT", 86400),
        child_timeout("MILHOUSE_CODEX_IDLE_TIMEOUT", 3600),
        jsonl.parent().unwrap_or(Path::new(".")),
        "codex",
    );
    if let Err(error) = stop_tracked_child(active_run, child_pid) {
        if let Some(run_dir) = active_run {
            let _ = write_log(
                run_dir,
                &format!(
                    "codex child {child_pid} cleanup failed; {} retained: {error}",
                    ACTIVE_CHILD_PID_FILE
                ),
            );
        }
        return Err(error);
    }
    cleanup_active_child(active_run);
    result
}

struct ActiveChildFiles {
    _pid: File,
}

fn prepare_active_child(
    cmd: &mut ProcessCommand,
    run_dir: &Path,
) -> Result<ActiveChildFiles, AppError> {
    let mut pid = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(run_dir.join(ACTIVE_CHILD_PID_FILE))?;
    pid.try_lock_exclusive()
        .map_err(|error| AppError::External(format!("active child lock held: {error}")))?;
    pid.set_len(0)?;
    pid.seek(SeekFrom::Start(0))?;
    let pid_fd = pid.as_raw_fd();
    let flags = unsafe { libc::fcntl(pid_fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(pid_fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } < 0 {
        return Err(io::Error::last_os_error().into());
    }
    unsafe {
        cmd.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(io::Error::last_os_error());
            }
            write_current_pid(pid_fd)
        });
    }
    Ok(ActiveChildFiles { _pid: pid })
}

fn write_current_pid(fd: i32) -> io::Result<()> {
    let mut bytes = [0_u8; 32];
    let mut cursor = bytes.len() - 1;
    bytes[cursor] = b'\n';
    let mut pid = unsafe { libc::getpid() } as u32;
    loop {
        cursor -= 1;
        bytes[cursor] = b'0' + (pid % 10) as u8;
        pid /= 10;
        if pid == 0 {
            break;
        }
    }
    let mut written = cursor;
    while written < bytes.len() {
        let count =
            unsafe { libc::write(fd, bytes[written..].as_ptr().cast(), bytes.len() - written) };
        if count > 0 {
            written += count as usize;
        } else if count < 0 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        } else {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn cleanup_active_child(run_dir: Option<&Path>) {
    if let Some(run_dir) = run_dir {
        let _ = fs::remove_file(run_dir.join(ACTIVE_CHILD_PID_FILE));
    }
}

fn stop_tracked_child(run_dir: Option<&Path>, expected_pid: i32) -> Result<(), AppError> {
    let Some(run_dir) = run_dir else {
        return Ok(());
    };
    let stop = || {
        let pid = tracked_pid(run_dir, ACTIVE_CHILD_PID_FILE, Duration::from_secs(2))?;
        if pid > 1 && pid != expected_pid {
            return Err(AppError::External(format!(
                "active child pid changed from {expected_pid} to {pid}"
            )));
        }
        stop_process_group(pid, Duration::from_secs(2))
    };
    stop().map_err(|error| match error {
        AppError::ProcessesAlive(_) => error,
        other => AppError::ProcessIdentityUncertain(format!(
            "could not verify tracked child process group: {other}"
        )),
    })
}

fn transient_codex_failure(jsonl: &Path, stderr: &Path, last: &Path) -> bool {
    [jsonl, stderr, last]
        .iter()
        .filter_map(|p| fs::read_to_string(p).ok())
        .any(|s| {
            let s = s.to_ascii_lowercase();
            s.contains("model is at capacity")
                || s.contains("selected model is at capacity")
                || s.contains("temporarily unavailable")
        })
}

fn build_codex_loop_prompt(root: &Path, run_dir: &Path, attempt: u32) -> Result<String, AppError> {
    let run_json = fs::read_to_string(run_dir.join("run.json"))?;
    let contract = fs::read_to_string(run_dir.join("contract.md"))?;
    let history = fs::read_to_string(run_dir.join("run-history.md"))?;
    Ok(fs::read_to_string(root.join("prompts/codex-loop.md"))?
        .replace("@RUN_DIR@", &run_dir.to_string_lossy())
        .replace("@ATTEMPT@", &attempt.to_string())
        .replace("@RUN_JSON@", &run_json)
        .replace("@CONTRACT_MD@", &contract)
        .replace("@RUN_HISTORY@", &history))
}

fn run_verifier(
    root: &Path,
    workdir: &Path,
    config: &Config,
    run_dir: &Path,
    attempt: u32,
) -> Result<VerifierVerdict, AppError> {
    let verifier_profile = config.verify_profile.details()?;
    let prompt = build_verify_prompt(root, workdir, run_dir)?;
    let prompt_path = run_dir.join(format!("verifier-{attempt}.md"));
    let jsonl_path = run_dir.join(format!("verifier-{attempt}.jsonl"));
    let stderr_path = run_dir.join(format!("verifier-{attempt}.stderr.log"));
    let last_path = run_dir.join(format!("verifier-{attempt}.txt"));
    fs::write(&prompt_path, prompt)?;
    let mut cmd = codex_cmd(workdir);
    cmd.args(["exec", "--cd"])
        .arg(workdir)
        .args(codex_common_args(
            &last_path,
            &verifier_profile.model,
            verifier_profile.effort.as_str(),
            false,
        ))
        .arg("-");
    let status = run_codex_child(cmd, &prompt_path, &jsonl_path, &stderr_path, Some(run_dir))?;
    if !status.success() {
        return Err(AppError::External(format!(
            "Sol high verifier exited {status}; stderr: {}",
            stderr_path.display()
        )));
    }
    let raw = fs::read_to_string(&last_path)?;
    let verifier = extract_verifier_json(&raw)
        .ok_or_else(|| AppError::External("Sol high verifier produced no valid JSON".into()))?;
    let out = run_dir.join(format!("verifier-{attempt}.json"));
    fs::write(&out, serde_json::to_string_pretty(&verifier)? + "\n")?;
    validate_verifier_file(&out)?;
    append_run_history(
        run_dir,
        &format!(
            "## Verifier {attempt}\npassed: {}\nnotes: {}\n\n",
            verifier.passed, verifier.notes
        ),
    )?;
    Ok(verifier)
}

fn build_verify_prompt(root: &Path, workdir: &Path, run_dir: &Path) -> Result<String, AppError> {
    let run_json = fs::read_to_string(run_dir.join("run.json"))?;
    let contract = fs::read_to_string(run_dir.join("contract.md"))?;
    let history = fs::read_to_string(run_dir.join("run-history.md"))?;
    Ok(fs::read_to_string(root.join("prompts/verify.md"))?
        .replace("@RUN_JSON@", &run_json)
        .replace("@CONTRACT_MD@", &contract)
        .replace("@RUN_HISTORY@", &history)
        .replace("@DIFF@", &workdir_diff(workdir))
        .replace("@EVIDENCE@", &evidence_summary(run_dir)?))
}

fn init_run_history(run_dir: &Path) -> Result<(), AppError> {
    let path = run_dir.join("run-history.md");
    if !path.exists() {
        fs::write(path, "# Run History\n\n")?;
    }
    Ok(())
}

fn write_green_verdict(run_dir: &Path) -> Result<(), AppError> {
    let run = read_run_spec(run_dir)?;
    let verdict = Verdict {
        run_id: run.id,
        status: VerdictStatus::Green,
        summary: "Verifier passed contract.md 100%.".into(),
        criteria_met: true,
        contract: PathBuf::from("contract.md"),
        evidence: evidence_paths(run_dir)?,
        note: fs::read_to_string(run_dir.join("run-history.md")).unwrap_or_default(),
    };
    write_verdict(run_dir, &verdict)
}

fn write_stopped_verdict(run_dir: &Path, summary: String) -> Result<(), AppError> {
    let run = read_run_spec(run_dir)?;
    let verdict = Verdict {
        run_id: run.id,
        status: VerdictStatus::HitRunLimit,
        summary,
        criteria_met: false,
        contract: PathBuf::from("contract.md"),
        evidence: evidence_paths(run_dir).unwrap_or_default(),
        note: fs::read_to_string(run_dir.join("run-history.md")).unwrap_or_default(),
    };
    write_verdict(run_dir, &verdict)
}

fn write_needs_user_verdict(run_dir: &Path, blocker: String) -> Result<(), AppError> {
    let run = read_run_spec(run_dir)?;
    let verdict = Verdict {
        run_id: run.id,
        status: VerdictStatus::NeedsUser,
        summary: format!(
            "Verifier failed on the same blocker two attempts running; stopping for a human decision instead of burning attempts: {blocker}"
        ),
        criteria_met: false,
        contract: PathBuf::from("contract.md"),
        evidence: evidence_paths(run_dir).unwrap_or_default(),
        note: fs::read_to_string(run_dir.join("run-history.md")).unwrap_or_default(),
    };
    write_verdict(run_dir, &verdict)
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BlockerFingerprint {
    normalized: String,
    criteria: Vec<String>,
    causes: Vec<&'static str>,
    identifiers: Vec<String>,
}

/// Fingerprint verifier prose using stable acceptance-criterion and cause anchors.
/// Verifier notes are generated text, so equivalent external blockers are often
/// paraphrased even though their criterion IDs and root causes are unchanged.
fn blocker_fingerprint(notes: &str) -> BlockerFingerprint {
    let normalized = notes
        .split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| !c.is_ascii_digit())
                .collect::<String>()
        })
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();

    let lower = notes.to_lowercase();
    let mut criteria = notes
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| {
            word.strip_prefix('a').is_some_and(|suffix| {
                !suffix.is_empty() && suffix.chars().all(|c| c.is_ascii_digit())
            })
        })
        .collect::<Vec<_>>();
    criteria.sort();
    criteria.dedup();

    let mut causes = Vec::new();
    let cause_groups = [
        (
            "credentials",
            ["credential", "auto-login", "password", "api key", "api_key"].as_slice(),
        ),
        (
            "native_runtime",
            [
                "coresimulator",
                "simulator",
                "sandbox",
                "swiftpm",
                "operation not permitted",
                "native push-gate",
                "native non-nested",
                "xcode",
            ]
            .as_slice(),
        ),
        (
            "deployment",
            [
                "deploy",
                "deployment",
                "production secret",
                "secret manager",
            ]
            .as_slice(),
        ),
        ("schema", ["schema", "openapi"].as_slice()),
        (
            "signing",
            ["signing", "provisioning profile", "certificate"].as_slice(),
        ),
        (
            "external_wait",
            ["external wait", "pending approval", "human decision"].as_slice(),
        ),
        (
            "provider_capacity",
            ["rate limit", "provider capacity", "quota"].as_slice(),
        ),
        (
            "compile",
            ["compile error", "mismatched types", "type error"].as_slice(),
        ),
    ];
    for (cause, needles) in cause_groups {
        if needles
            .iter()
            .any(|needle| contains_blocker_term(&lower, needle))
        {
            causes.push(cause);
        }
    }

    let mut identifiers = notes
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| {
            word.contains('_')
                && word
                    .chars()
                    .all(|c| c == '_' || c.is_ascii_uppercase() || c.is_ascii_digit())
                && word.chars().any(|c| c.is_ascii_uppercase())
        })
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    identifiers.sort();
    identifiers.dedup();

    BlockerFingerprint {
        normalized,
        criteria,
        causes,
        identifiers,
    }
}

fn contains_blocker_term(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(start, _)| {
        let before = haystack[..start].chars().next_back();
        let end = start + needle.len();
        let after = haystack[end..].chars().next();
        let boundary = |c: Option<char>| c.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'));
        boundary(before) && boundary(after)
    })
}

fn blockers_equivalent(previous: &BlockerFingerprint, current: &BlockerFingerprint) -> bool {
    let identifiers_conflict = !previous.identifiers.is_empty()
        && !current.identifiers.is_empty()
        && !previous
            .identifiers
            .iter()
            .any(|identifier| current.identifiers.contains(identifier));
    if identifiers_conflict {
        return false;
    }
    if previous.criteria != current.criteria
        && (!previous.criteria.is_empty() || !current.criteria.is_empty())
    {
        return false;
    }

    if previous.normalized == current.normalized {
        return true;
    }

    if !previous.criteria.is_empty()
        && previous.criteria == current.criteria
        && !previous.causes.is_empty()
        && previous.causes == current.causes
    {
        return true;
    }

    !previous.identifiers.is_empty()
        && previous
            .identifiers
            .iter()
            .any(|identifier| current.identifiers.contains(identifier))
        && !previous.causes.is_empty()
        && previous.causes == current.causes
}

fn write_verdict(run_dir: &Path, verdict: &Verdict) -> Result<(), AppError> {
    let path = run_dir.join("verdict.json");
    fs::write(&path, serde_json::to_string_pretty(&verdict)? + "\n")?;
    validate_verdict_file(&path)?;
    Ok(())
}

fn write_progress(
    root: &Path,
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
    cost: &RunCost,
    phase: &str,
    attempt: u32,
) -> Result<(), AppError> {
    let builder = config.build_profile.details()?;
    let codex_cost = cost.codex.map(round4);
    let total = cost.total().map(round4).unwrap_or(0.0);
    let cost_by_tier = json!({
        "fable": 0.0,
        "claude": 0.0,
        "codex": codex_cost.unwrap_or(0.0),
    });
    let cost_by_model = json!({
        builder.model.clone(): codex_cost.unwrap_or(0.0),
    });
    atomic_write(
        &run_dir.join("progress.json"),
        &(serde_json::to_string_pretty(&json!({
            "status": "running",
            "issue": task.id,
            "phase": phase,
            "attempt": attempt,
            "claude_cost_usd": total,
            "milhouse_cost_usd": total,
            "codex_cost_usd": codex_cost,
            "cost_status": if cost.unknown { "unknown" } else { "known" },
            "cost_by_tier": cost_by_tier,
            "milhouse_cost_by_tier": cost_by_tier,
            "cost_by_model": cost_by_model,
            "milhouse_cost_by_model": cost_by_model,
            "run_dir": run_dir,
            "arch_sha": arch_sha(root)?,
            "variant": "milhouse-rust"
        }))? + "\n"),
    )
}

fn evidence_paths(run_dir: &Path) -> Result<Vec<PathBuf>, AppError> {
    let bytes = directory_bytes(run_dir)?;
    if bytes > RUN_STORAGE_BUDGET_BYTES {
        return Err(AppError::StorageBudget(format!(
            "run storage exceeded {} bytes (used {bytes})",
            RUN_STORAGE_BUDGET_BYTES
        )));
    }
    let dir = run_dir.join("evidence");
    if !dir.is_dir() {
        return Err(AppError::Input("missing evidence/ directory".into()));
    }
    let mut paths = walk_files(&dir)?
        .into_iter()
        .filter_map(|p| p.strip_prefix(run_dir).ok().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(AppError::Input("evidence/ is empty".into()));
    }
    Ok(paths)
}

fn append_run_history(run_dir: &Path, text: &str) -> Result<(), AppError> {
    let mut f = OpenOptions::new()
        .create(true)
        .append(true)
        .open(run_dir.join("run-history.md"))?;
    f.write_all(text.as_bytes())?;
    Ok(())
}

fn workdir_diff(workdir: &Path) -> String {
    let mut out = String::new();
    for repo in git_repos(workdir) {
        out.push_str(&format!("== {}: git status --short ==\n", repo.display()));
        out.push_str(&cmd_text(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(&repo)
                .arg("status")
                .arg("--short"),
            20_000,
        ));
        out.push_str(&format!("\n== {}: git diff --stat ==\n", repo.display()));
        out.push_str(&cmd_text(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(&repo)
                .arg("diff")
                .arg("--stat"),
            20_000,
        ));
        out.push_str(&format!(
            "\n== {}: git diff HEAD~1..HEAD --stat ==\n",
            repo.display()
        ));
        out.push_str(&cmd_text(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(&repo)
                .arg("diff")
                .arg("HEAD~1..HEAD")
                .arg("--stat"),
            20_000,
        ));
        out.push('\n');
    }
    out
}

fn evidence_summary(run_dir: &Path) -> Result<String, AppError> {
    let dir = run_dir.join("evidence");
    if !dir.is_dir() {
        return Ok("missing evidence/ directory".into());
    }
    let mut out = String::new();
    for path in walk_files(&dir)?.into_iter().take(40) {
        let rel = path.strip_prefix(run_dir).unwrap_or(&path);
        out.push_str(&format!("== {} ==\n", rel.display()));
        if path.metadata().map(|m| m.len()).unwrap_or(0) <= 40_000 {
            out.push_str(&fs::read_to_string(&path).unwrap_or_else(|_| "<binary>\n".into()));
        } else {
            out.push_str("<large file>\n");
        }
        out.push('\n');
    }
    if out.is_empty() {
        out.push_str("evidence/ is empty\n");
    }
    Ok(out)
}

fn extract_verifier_json(raw: &str) -> Option<VerifierVerdict> {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if let Some(result) = v.get("result").and_then(Value::as_str) {
            if let Some(verifier) = extract_verifier_json(result) {
                return Some(verifier);
            }
        }
        if let Ok(verifier) = serde_json::from_value::<VerifierVerdict>(v) {
            return valid_verifier(verifier);
        }
    }
    let mut depth = 0i32;
    let mut start = None;
    let mut in_string = false;
    let mut escaped = false;
    for (i, ch) in raw.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        if ch == '"' {
            in_string = true;
            continue;
        }
        match ch {
            '{' => {
                if depth == 0 {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let s = &raw[start?..=i];
                    if let Ok(verifier) = serde_json::from_str::<VerifierVerdict>(s) {
                        if let Some(verifier) = valid_verifier(verifier) {
                            return Some(verifier);
                        }
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    None
}

fn valid_verifier(verifier: VerifierVerdict) -> Option<VerifierVerdict> {
    (!verifier.notes.trim().is_empty()).then_some(verifier)
}

fn validate_frozen_contract(run_dir: &Path) -> Result<u64, AppError> {
    let path = run_dir.join("contract.md");
    let text = fs::read_to_string(&path)
        .map_err(|e| AppError::External(format!("missing contract.md: {e}")))?;
    validate_contract_text(&text)?;
    Ok(seahash::hash(text.as_bytes()))
}

fn run_owned(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
) -> Result<RunStatus, AppError> {
    let state = lock(&run_dir.join("run.json"))?;
    if run_dir.join("sentinel").is_file() || run_dir.join("result.json").is_file() {
        drop(state);
        return Err(AppError::Input(format!(
            "run already terminal: {}",
            run_dir.display()
        )));
    }
    let identity = start_run_process_group(run_dir)?;
    drop(state);
    let result = run_engine(root, workdir, config, task, run_dir);
    let _ = fs::remove_file(run_dir.join("run.pid"));
    drop(identity);
    result
}

fn start_run_process_group(run_dir: &Path) -> Result<File, AppError> {
    if unsafe { libc::setpgid(0, 0) } != 0 {
        let error = io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::EPERM)
            || unsafe { libc::getpgrp() } != std::process::id() as i32
        {
            return Err(error.into());
        }
    }
    let mut identity = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(run_dir.join("run.pid"))?;
    identity
        .try_lock_exclusive()
        .map_err(|error| AppError::External(format!("run identity lock held: {error}")))?;
    identity.set_len(0)?;
    identity.seek(SeekFrom::Start(0))?;
    writeln!(identity, "{}", std::process::id())?;
    identity.sync_all()?;
    Ok(identity)
}

fn ensure_contract_unchanged(run_dir: &Path, before: u64) -> Result<(), AppError> {
    let after = validate_frozen_contract(run_dir)?;
    if after != before {
        return Err(AppError::External("codex phase mutated contract.md".into()));
    }
    Ok(())
}

pub(crate) fn anthropic_key_file() -> Result<PathBuf, AppError> {
    if let Ok(p) = env::var("ANTHROPIC_KEY_FILE") {
        return Ok(PathBuf::from(p));
    }
    let config_path = crate::Config::default().home.join("config.json");
    if let Ok(text) = fs::read_to_string(&config_path) {
        if let Ok(v) = serde_json::from_str::<Value>(&text) {
            if let Some(p) = v["anthropic_key_file"].as_str() {
                return Ok(PathBuf::from(p));
            }
        }
    }
    Err(AppError::Input(
        "no Anthropic key file configured: set ANTHROPIC_KEY_FILE or run `mil setup --anthropic-key-file <path>`".into(),
    ))
}

pub(crate) fn read_env_key(path: &Path, key: &str) -> Result<String, AppError> {
    for line in fs::read_to_string(path)?.lines() {
        let s = line.trim().strip_prefix("export ").unwrap_or(line.trim());
        if let Some(v) = s.strip_prefix(&format!("{key}=")) {
            let v = v.trim().trim_matches('"').trim_matches('\'').to_string();
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    Err(AppError::External(format!(
        "missing {key} in {}",
        path.display()
    )))
}

fn finalize_run(
    root: &Path,
    workdir: &Path,
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
    cost: RunCost,
    resumes: u32,
) -> Result<RunStatus, AppError> {
    let verdict = validate_verdict_file(&run_dir.join("verdict.json"))
        .ok()
        .and_then(|v| serde_json::from_value::<Verdict>(v["verdict"].clone()).ok());
    let status = verdict
        .as_ref()
        .map(|v| RunStatus::from(v.status))
        .unwrap_or(RunStatus::Failed);
    if verdict.is_none() {
        write_log(run_dir, "no valid verdict — finalizing failed")?;
    }
    let commit = git_short_head(workdir).unwrap_or_default();
    let total = cost.total();
    let codex_cost = cost.codex.map(round4);
    let cost_by_tier = json!({
        "fable": 0.0,
        "claude": 0.0,
        "codex": codex_cost.unwrap_or(0.0),
    });
    let builder = config.build_profile.details()?;
    let verifier_profile = config.verify_profile.details()?;
    let cost_by_model = json!({builder.model.clone(): codex_cost.unwrap_or(0.0)});
    let result = json!({
        "status": status.as_str(), "issue": task.id, "steps": 1, "resumes": resumes, "commit": commit,
        "pushed": null,
        "claude_cost_usd": total,
        "milhouse_cost_usd": total,
        "codex_cost_usd": codex_cost,
        "cost_status": if cost.unknown { "unknown" } else { "known" },
        "workflow_cost_usd": null,
        "cost_by_tier": cost_by_tier,
        "milhouse_cost_by_tier": cost_by_tier,
        "cost_by_model": cost_by_model,
        "milhouse_cost_by_model": cost_by_model,
        "run_dir": run_dir, "grade_score": null, "arch_sha": arch_sha(root)?,
        "verdict": verdict, "models": {"executor": builder.model, "executor_profile": config.build_profile.id(), "verifier": verifier_profile.model, "verifier_profile": config.verify_profile.id(), "scorer": Model::Opus.id()?},
        "variant": "milhouse-rust"
    });
    durable_atomic_write(
        &run_dir.join("result.json"),
        &(serde_json::to_string_pretty(&result)? + "\n"),
    )?;
    fs::write(run_dir.join("sentinel"), status.as_str().to_string() + "\n")?;
    let _ = fs::remove_file(run_dir.join("run.pid"));
    let mut row = result.clone();
    row["ts"] = json!(iso_now());
    append_jsonl(&config.home.join("runs.jsonl"), &row)?;
    write_log(
        run_dir,
        &format!(
            "result: {} cost={} commit={} run_dir={}",
            status.as_str(),
            total
                .map(|c| format!("${c:.4}"))
                .unwrap_or_else(|| "null".into()),
            commit,
            run_dir.display()
        ),
    )?;
    Ok(status)
}

fn finalize_stale_run(
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
    reason: &str,
) -> Result<RunStatus, AppError> {
    finalize_terminal_failure(
        config,
        task,
        run_dir,
        RunStatus::FailedStale,
        "failed_stale",
        reason,
    )
}

fn finalize_storage_budget_run(
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
    reason: &str,
) -> Result<RunStatus, AppError> {
    finalize_terminal_failure(
        config,
        task,
        run_dir,
        RunStatus::FailedStorageBudget,
        "failed_storage_budget",
        reason,
    )
}

fn finalize_terminal_failure(
    config: &Config,
    task: &TaskInput,
    run_dir: &Path,
    status: RunStatus,
    sentinel: &str,
    reason: &str,
) -> Result<RunStatus, AppError> {
    let result = terminal_result(status, task.id.clone(), reason, run_dir);
    durable_atomic_write(
        &run_dir.join("result.json"),
        &(serde_json::to_string_pretty(&result)? + "\n"),
    )?;
    fs::write(run_dir.join("sentinel"), format!("{sentinel}\n"))?;
    let _ = fs::remove_file(run_dir.join("run.pid"));
    let mut row = result;
    row["ts"] = json!(iso_now());
    append_jsonl(&config.home.join("runs.jsonl"), &row)?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn temp_run(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "milhouse-engine-test-{name}-{}-{}",
            std::process::id(),
            epoch()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn temp_root(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "milhouse-root-test-{name}-{}-{}",
            std::process::id(),
            epoch()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("prompts")).unwrap();
        fs::write(
            dir.join("prompts/codex-loop.md"),
            "create a persisted Codex goal\nsatisfy @RUN_DIR@\n@RUN_JSON@\n@CONTRACT_MD@\n@RUN_HISTORY@\n",
        )
        .unwrap();
        dir
    }

    #[test]
    fn claude_plan_prompt_is_not_consumed_by_add_dir() {
        let args = interactive_claude_args(
            Path::new("/tmp/contract-parent"),
            ModelProfile::named("opus-5-5-high-subscription")
                .details()
                .unwrap(),
            "plan this",
        );
        assert_eq!(args[4], OsStr::new("--add-dir"));
        assert_eq!(args[5], OsStr::new("/tmp/contract-parent"));
        assert_eq!(args[6], OsStr::new("--"));
        assert_eq!(args[7], OsStr::new("plan this"));
    }

    #[test]
    fn codex_plan_prompt_is_not_consumed_by_add_dir() {
        let args = interactive_codex_args(
            Path::new("/tmp/work"),
            Path::new("/tmp/contract-parent"),
            ModelProfile::named("sol-6.1-high-subscription")
                .details()
                .unwrap(),
            "plan this",
        );
        assert_eq!(
            &args[args.len() - 2..],
            [OsStr::new("--"), OsStr::new("plan this")]
        );
    }

    #[test]
    fn planning_can_pause_before_contract_exists() {
        let contract = temp_run("planning-pause").join("contract.md");
        let (status, output) = planning_result(&contract).unwrap();
        assert_eq!(status, "planning");
        assert!(output.is_null());

        fs::write(&contract, complete_contract_fixture()).unwrap();
        let (status, output) = planning_result(&contract).unwrap();
        assert_eq!(status, "planned");
        assert_eq!(output, json!(contract));
        assert!(contract.with_extension("html").is_file());
    }

    #[test]
    fn planning_prompt_is_a_thin_persistent_coordinator() {
        let prompt = planning_coordinator_prompt(
            Path::new("/tmp/mil-plan/SKILL.md"),
            "Plan · TASK-512",
            "TASK-512",
            "sol-6.1-high-subscription",
            "codex",
            Path::new("/path/to/workspace"),
            "No additional worker profiles were requested.",
            Path::new("/path/to/workspace/contract.md"),
        );
        assert!(prompt.starts_with("Plan · TASK-512\n\nRole: coordinator."));
        assert!(prompt.contains("Do not perform planning judgment yourself"));
        assert!(prompt.contains("Use `rolly agent` for every mapper"));
        assert!(prompt.contains("Default worker profile: sol-6.1-high-subscription"));
        assert!(prompt.contains("prototype routing may override it"));
        assert!(prompt.contains("Keep this one user conversation active across the entire map"));
        assert!(prompt.contains("documented cancel/defer recommendation"));
    }

    #[test]
    fn planning_session_title_is_brief() {
        let title = planning_session_title(
            "analytics data for important metrics in the rolly dashboard with a very long suffix",
        );
        assert!(title.starts_with("Plan · analytics data"));
        assert!(title.ends_with('…'));
        assert!(title.chars().count() <= 60);
    }

    #[test]
    fn autonomous_plan_decision_is_strict_and_typed() {
        assert!(matches!(
            parse_autonomous_plan_decision(r#"{"state":"planned","contract_markdown":"contract"}"#)
                .unwrap(),
            AutonomousPlanDecision::Planned { .. }
        ));
        assert!(matches!(
            parse_autonomous_plan_decision(
                r#"{"state":"needs_user","reason_markdown":"choose behavior"}"#
            )
            .unwrap(),
            AutonomousPlanDecision::NeedsUser { .. }
        ));
        assert!(parse_autonomous_plan_decision(
            r#"{"state":"planned","contract_markdown":"contract","extra":true}"#
        )
        .is_err());
        assert!(parse_autonomous_plan_decision("```json\n{}\n```").is_err());

        assert!(matches!(
            parse_autonomous_plan_review(r#"{"state":"approved"}"#).unwrap(),
            AutonomousPlanReview::Approved {}
        ));
        assert!(matches!(
            parse_autonomous_plan_review(
                r#"{"state":"needs_user","reason_markdown":"scope unclear"}"#
            )
            .unwrap(),
            AutonomousPlanReview::NeedsUser { .. }
        ));
        assert!(parse_autonomous_plan_review(
            r#"{"state":"approved","reason_markdown":"unexpected"}"#
        )
        .is_err());
    }

    #[test]
    fn legacy_named_surface_cannot_reach_contract_without_current_use_evidence() {
        let raw_planner_artifact = r#"{
            "state":"planned",
            "pursuit_evidence":{
                "goal":"Keep the current operator prompt workflow safe",
                "current_use_evidence":[],
                "named_concepts":[{
                    "name":"Story prompt callable",
                    "status":"legacy_or_internal",
                    "live_concept":"Radio prompt configuration",
                    "basis":"The old callable and generated schema still exist"
                }],
                "decision_test":{
                    "pursue_now":"Version the existing endpoint",
                    "defer":"Waiting leaves the mismatch",
                    "cancel":"The repository still contains the endpoint",
                    "no_build":"Documentation would not change the payload",
                    "delete_or_retire":""
                }
            },
            "contract_markdown":"A complete-looking contract"
        }"#;

        let AutonomousPlanDecision::Planned {
            pursuit_evidence, ..
        } = parse_autonomous_plan_decision(raw_planner_artifact).unwrap()
        else {
            panic!("expected planned artifact");
        };
        let error = validate_autonomous_pursuit_evidence(pursuit_evidence.as_ref())
            .err()
            .expect("legacy-only evidence should fail closed");

        assert!(error.contains("current_use_evidence"));
        assert!(error.contains("delete_or_retire"));
    }

    #[test]
    fn pursuit_gate_accepts_a_legacy_name_only_when_the_live_concept_is_evidenced() {
        let raw_evidence = json!({
            "goal": "Keep the current operator prompt workflow safe",
            "current_use_evidence": [{
                "source": "studio/current-radio-panel.mjs:loadPromptConfiguration",
                "observation": "The current operator entry point invokes this workflow"
            }],
            "named_concepts": [{
                "name": "Story prompt callable",
                "status": "legacy_or_internal",
                "live_concept": "Radio prompt configuration",
                "basis": "The live caller uses the old wire name for the current radio concept"
            }],
            "decision_test": {
                "pursue_now": "A narrow document-integrity fix protects a live workflow",
                "defer": "Waiting leaves current writes unsafe",
                "cancel": "The live operator entry point proves the goal remains current",
                "no_build": "Configuration cannot prevent the malformed write",
                "delete_or_retire": "The evidenced workflow is still required"
            }
        });

        let evidence = validate_autonomous_pursuit_evidence(Some(&raw_evidence)).unwrap();
        assert!(matches!(
            evidence.named_concepts[0].status,
            NamedConceptStatus::LegacyOrInternal
        ));
    }

    #[test]
    fn one_canonical_pursuit_gate_drives_guided_and_autonomous_planning() {
        let gate = include_str!("../skills/mil-plan/references/pursuit-evidence.md");
        let skill = include_str!("../skills/mil-plan/SKILL.md");
        let orchestration = include_str!("../skills/mil-plan/references/orchestration.md");

        for required in [
            "current`, `legacy_or_internal`, or `mixed`",
            "current-use evidence is absent",
            "The first planning question must establish whether the live concept is still used",
            "`pursue_now`",
            "`defer`",
            "`cancel`",
            "`no_build`",
            "`delete_or_retire`",
        ] {
            assert!(
                gate.contains(required),
                "missing pursuit gate rule: {required}"
            );
        }
        assert!(skill.contains("references/pursuit-evidence.md"));
        assert!(orchestration.contains("[pursuit-evidence.md](pursuit-evidence.md)"));
    }

    #[test]
    fn autonomous_prompt_fails_closed_before_contracting() {
        let prompt = autonomous_plan_prompt(
            Path::new("/tmp/milhouse"),
            "TASK-1",
            "fix it",
            Path::new("/tmp/contract.md"),
        );
        assert!(prompt.contains("genuinely small and self-contained"));
        assert!(prompt.contains("When in doubt, stop for guided planning"));
        assert!(prompt.contains("no unresolved product/UX/architecture/scope choice"));
        assert!(prompt.contains("skills/mil-plan/references/pursuit-evidence.md"));
        assert!(
            prompt.contains("exact `planned` JSON shape specified by the pursuit evidence gate")
        );
        assert!(prompt.contains("\"state\":\"needs_user\""));

        let review = autonomous_review_prompt(
            Path::new("/tmp/milhouse"),
            "TASK-1",
            "fix it",
            r#"{"current_use_evidence":["live caller"]}"#,
            "# Contract",
        );
        assert!(review.contains("independent autonomous-plan reviewer"));
        assert!(review.contains("Do not implement"));
        assert!(review.contains("repair the contract"));
        assert!(review.contains("verify the proposed evidence"));
        assert!(review.contains("skills/mil-plan/references/pursuit-evidence.md"));
        assert!(review.contains("\"state\":\"approved\""));
        assert!(review.contains("# Contract"));
    }

    #[test]
    fn autonomous_contract_canonicalizes_missing_acceptance_bullets() {
        let contract = complete_contract_fixture().replace(
            "- A1 (meet): **The behavior works with durable proof.**\n  - proof: run-check\n  - expect: exit 0",
            "A1 (meet): **The behavior works with durable proof.**\n  proof: run-check\n  expect: exit 0",
        );

        let canonical = canonicalize_autonomous_contract(&contract);

        assert!(canonical.contains(
            "- A1 (meet): **The behavior works with durable proof.**\n  - proof: run-check\n  - expect: exit 0"
        ));
        assert!(validate_build_contract_text(&canonical).is_ok());
    }

    #[test]
    fn autonomous_contract_canonicalization_does_not_repair_semantics() {
        let contract = complete_contract_fixture()
            .replace("A1 (meet)", "A1 (maybe)")
            .replace("  - expect: exit 0\n", "");

        let canonical = canonicalize_autonomous_contract(&contract);

        assert!(validate_build_contract_text(&canonical).is_err());
    }

    #[test]
    fn unattended_planning_is_opus_subscription_only() {
        assert!(validate_unattended_plan_profile(
            "planner",
            &ModelProfile::named("opus-5-5-high-subscription")
        )
        .is_ok());
        assert!(validate_unattended_plan_profile(
            "planner",
            &ModelProfile::named("fable-5-1-high-api")
        )
        .is_err());

        let guidance = include_str!("../skills/mil-plan/references/orchestration.md");
        assert!(guidance
            .contains("Unattended and default planning always use `opus-5-5-high-subscription`"));
        assert!(guidance.contains("never select, resume, or fall back to a Fable or API profile"));
        assert!(!guidance.contains("--agent fable-5-"));
    }

    #[test]
    fn latest_run_picks_newest_and_needs_run_json() {
        let home = temp_run("latest");
        assert!(latest_run(&home).is_err()); // no runs/ yet
        for base in ["20260101-000000-a", "20260709-120000-b"] {
            let d = home.join("runs").join(base);
            fs::create_dir_all(&d).unwrap();
            fs::write(d.join("run.json"), "{}").unwrap();
        }
        // a run dir without run.json is ignored
        fs::create_dir_all(home.join("runs/20260709-130000-noise")).unwrap();
        assert_eq!(
            latest_run(&home).unwrap().file_name().unwrap(),
            OsStr::new("20260709-120000-b")
        );
    }

    #[test]
    fn validates_frozen_contract() {
        let dir = temp_run("contract");
        assert!(validate_frozen_contract(&dir).is_err());
        fs::write(dir.join("contract.md"), "\n").unwrap();
        assert!(validate_frozen_contract(&dir).is_err());
        fs::write(
            dir.join("contract.md"),
            format!("# Acceptance criteria\n\n{}", "proof\n".repeat(40)),
        )
        .unwrap();
        assert!(validate_frozen_contract(&dir).is_ok());
    }

    #[test]
    fn detects_contract_mutation() {
        let dir = temp_run("contract-freeze");
        fs::write(
            dir.join("contract.md"),
            format!("# Acceptance criteria\n\n{}", "proof\n".repeat(40)),
        )
        .unwrap();
        let before = validate_frozen_contract(&dir).unwrap();
        ensure_contract_unchanged(&dir, before).unwrap();
        fs::write(dir.join("contract.md"), "# Changed\n").unwrap();
        assert!(ensure_contract_unchanged(&dir, before).is_err());
    }

    #[test]
    fn codex_loop_prompt_requires_persisted_goal() {
        let root = temp_root("codex-loop");
        let run = temp_run("codex-loop");
        fs::write(run.join("run.json"), r#"{"id":"x"}"#).unwrap();
        fs::write(run.join("contract.md"), "# Contract\n").unwrap();
        fs::write(run.join("run-history.md"), "# Run History\n").unwrap();
        let prompt = build_codex_loop_prompt(&root, &run, 1).unwrap();
        assert!(!prompt.starts_with("/goal"));
        assert!(prompt.contains("create a persisted Codex goal"));
        assert!(prompt.contains("# Contract"));
        assert!(prompt.contains(r#""id":"x""#));
        assert!(prompt.contains("# Run History"));
    }

    #[test]
    fn detects_transient_codex_capacity_failure() {
        let run = temp_run("codex-capacity");
        let jsonl = run.join("codex-session.jsonl");
        let stderr = run.join("codex-stderr.log");
        let last = run.join("codex-output.txt");
        fs::write(&jsonl, r#"{"type":"error","message":"Selected model is at capacity. Please try a different model."}"#).unwrap();
        fs::write(&stderr, "").unwrap();
        fs::write(&last, "").unwrap();
        assert!(transient_codex_failure(&jsonl, &stderr, &last));
    }

    #[test]
    fn codex_timeout_kills_child() {
        let mut cmd = ProcessCommand::new("sh");
        cmd.arg("-c").arg("sleep 10").process_group(0);
        let child = cmd.spawn().unwrap();
        let activity = temp_run("codex-timeout");
        let err = wait_status_with_activity(
            child,
            Duration::from_millis(1),
            Duration::from_secs(10),
            &activity,
            "codex",
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("codex timed out"));
    }

    #[test]
    fn codex_idle_timeout_marks_stale() {
        let activity = temp_run("codex-idle");
        let mut cmd = ProcessCommand::new("sh");
        cmd.arg("-c").arg("sleep 10").process_group(0);
        let child = cmd.spawn().unwrap();
        let err = wait_status_with_activity(
            child,
            Duration::from_secs(10),
            Duration::from_millis(1),
            &activity,
            "codex",
        )
        .unwrap_err();
        assert!(matches!(err, AppError::Stale(_)));
    }

    #[test]
    fn codex_child_pid_is_removed_after_exit() {
        let run = temp_run("codex-child-pid");
        let prompt = run.join("prompt.md");
        let jsonl = run.join("session.jsonl");
        let stderr = run.join("stderr.log");
        fs::write(&prompt, "done\n").unwrap();
        let mut command = ProcessCommand::new("sh");
        command.arg("-c").arg("cat >/dev/null");

        let status = run_codex_child(command, &prompt, &jsonl, &stderr, Some(&run)).unwrap();

        assert!(status.success());
        assert!(!run.join(ACTIVE_CHILD_PID_FILE).exists());
    }

    #[test]
    fn codex_child_stops_background_descendants_before_clearing_identity() {
        let run = temp_run("codex-child-descendant");
        let prompt = run.join("prompt.md");
        let jsonl = run.join("session.jsonl");
        let stderr = run.join("stderr.log");
        let group = run.join("child-group");
        fs::write(&prompt, "done\n").unwrap();
        let mut command = ProcessCommand::new("sh");
        command
            .arg("-c")
            .arg("cat >/dev/null; echo $$ > \"$1\"; sleep 30 &")
            .arg("sh")
            .arg(&group);

        let status = run_codex_child(command, &prompt, &jsonl, &stderr, Some(&run)).unwrap();
        let group_pid = fs::read_to_string(group)
            .unwrap()
            .trim()
            .parse::<i32>()
            .unwrap();

        assert!(status.success());
        assert!(!process_group_alive(group_pid).unwrap());
        assert!(!run.join(ACTIVE_CHILD_PID_FILE).exists());
    }

    #[test]
    fn codex_once_uses_hardened_exec_shape() {
        use std::os::unix::fs::PermissionsExt;

        let _guard = ENV_LOCK.lock().unwrap();
        let dir = temp_run("codex-once");
        let bin = dir.join("bin");
        let workdir = dir.join("work");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&workdir).unwrap();
        let fake = bin.join("codex");
        fs::write(
            &fake,
            r#"#!/bin/sh
out=""
args="$*"
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then shift; out="$1"; fi
  shift
done
[ -n "$out" ] || { echo "missing -o" >&2; exit 2; }
case "$args" in *--json*" -o "*"-"*) ;; *) echo "bad args: $args" >&2; exit 3;; esac
for k in OPENAI_API_KEY OPENAI_BASE_URL CODEX_API_KEY ANTHROPIC_API_KEY; do
  eval "v=\${$k+x}"
  [ -z "$v" ] || { echo "$k set" >&2; exit 4; }
done
cat > "$out.stdin"
printf 'codex stdout\n'
printf 'codex output\n' > "$out"
"#,
        )
        .unwrap();
        let mut perms = fs::metadata(&fake).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&fake, perms).unwrap();
        let old_path = env::var("PATH").ok();
        let old_openai = env::var("OPENAI_API_KEY").ok();
        let old_base = env::var("OPENAI_BASE_URL").ok();
        let old_codex = env::var("CODEX_API_KEY").ok();
        let old_ant = env::var("ANTHROPIC_API_KEY").ok();
        env::set_var(
            "PATH",
            format!("{}:{}", bin.display(), old_path.clone().unwrap_or_default()),
        );
        env::set_var("OPENAI_API_KEY", "metered");
        env::set_var("OPENAI_BASE_URL", "metered");
        env::set_var("CODEX_API_KEY", "metered");
        env::set_var("ANTHROPIC_API_KEY", "metered");

        let prompt = dir.join("prompt.md");
        let output = dir.join("nested/out.txt");
        fs::write(&prompt, "say ok\n").unwrap();
        let result = codex_once(&workdir, "gpt-6.1-sol", &prompt, &output).unwrap();

        match old_path {
            Some(v) => env::set_var("PATH", v),
            None => env::remove_var("PATH"),
        }
        for (k, old) in [
            ("OPENAI_API_KEY", old_openai),
            ("OPENAI_BASE_URL", old_base),
            ("CODEX_API_KEY", old_codex),
            ("ANTHROPIC_API_KEY", old_ant),
        ] {
            match old {
                Some(v) => env::set_var(k, v),
                None => env::remove_var(k),
            }
        }

        assert_eq!(result["ok"], true);
        assert_eq!(fs::read_to_string(&output).unwrap(), "codex output\n");
        assert_eq!(
            fs::read_to_string(dir.join("nested/out.txt.stdin")).unwrap(),
            "say ok\n"
        );
        assert_eq!(
            fs::read_to_string(dir.join("nested/out.txt.jsonl")).unwrap(),
            "codex stdout\n"
        );
        assert!(dir.join("nested/out.txt.stderr.log").is_file());
    }

    #[test]
    fn codex_once_rejects_empty_prompt() {
        let dir = temp_run("codex-empty");
        let prompt = dir.join("prompt.md");
        fs::write(&prompt, "\n").unwrap();
        let err = codex_once(&dir, "gpt-6.1-sol", &prompt, &dir.join("out.txt"))
            .unwrap_err()
            .to_string();
        assert!(err.contains("--prompt is empty"));
    }

    #[test]
    fn verifier_diff_includes_nested_workspace_repositories() {
        let workspace = temp_run("workspace-diff");
        let repo = workspace.join("org/product");
        fs::create_dir_all(&repo).unwrap();
        let status = ProcessCommand::new("git")
            .args(["init", "-b", "main"])
            .arg(&repo)
            .status()
            .unwrap();
        assert!(status.success());
        fs::write(repo.join("changed.txt"), "changed\n").unwrap();

        let diff = workdir_diff(&workspace);
        assert!(diff.contains(&repo.display().to_string()));
        assert!(diff.contains("?? changed.txt"));
    }

    #[test]
    fn codex_args_separate_builder_and_reviewer_access() {
        let last = PathBuf::from("out.txt");
        let builder = codex_common_args(&last, "gpt-6.1-sol", "low", true);
        assert!(builder.contains(&"model_reasoning_effort=\"low\"".into()));
        assert!(builder.contains(&"--dangerously-bypass-approvals-and-sandbox".into()));
        let reviewer = codex_common_args(&last, "gpt-6.1-sol", "high", false);
        assert!(reviewer.contains(&"model_reasoning_effort=\"high\"".into()));
        assert!(reviewer
            .windows(2)
            .any(|args| args == ["--sandbox", "read-only"]));
        assert!(!reviewer.contains(&"--dangerously-bypass-approvals-and-sandbox".into()));
    }

    #[test]
    fn isolated_builder_is_workspace_sandboxed() {
        let last = PathBuf::from("/tmp/milhouse-run/out.txt");
        let args = codex_common_args_with_isolation(&last, "gpt-6.1-sol", "low", true, true);
        assert!(args
            .windows(2)
            .any(|args| args == ["--sandbox", "workspace-write"]));
        assert!(args
            .windows(2)
            .any(|args| args == ["--add-dir", "/tmp/milhouse-run"]));
        assert!(!args.contains(&"--dangerously-bypass-approvals-and-sandbox".into()));
    }

    #[test]
    fn writes_progress_cost_snapshot() {
        let root = temp_root("progress-root");
        let run = temp_run("progress");
        let workdir = temp_run("progress-workdir");
        fs::write(
            run.join("run.json"),
            serde_json::to_string(&json!({
                "id": "progress",
                "run_dir": run,
                "workdir": workdir,
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        let config = Config::default();
        let task = TaskInput {
            id: "progress".into(),
            prompt: "do thing".into(),
            metadata: json!({}),
        };
        write_progress(
            &root,
            &config,
            &task,
            &run,
            &RunCost {
                codex: Some(0.0),
                unknown: false,
            },
            "codex_loop",
            1,
        )
        .unwrap();
        let progress = read_json_value(&run.join("progress.json")).unwrap();
        assert_eq!(progress["status"], "running");
        assert_eq!(progress["cost_by_model"]["gpt-6.1-sol"], 0.0);
    }

    #[test]
    fn resume_reuses_frozen_contract() {
        let old = temp_run("resume-old").canonicalize().unwrap();
        let new = temp_run("resume-new").canonicalize().unwrap();
        fs::write(
            old.join("contract.md"),
            "# Acceptance\n\nThe resumed run must reuse this complete acceptance contract exactly. This text is intentionally long enough for the contract validator and confirms that a continuation does not mutate or regenerate the settled build contract. Every required behavior and proof remains frozen across resumed execution sessions.\n",
        )
        .unwrap();
        fs::write(
            new.join("run.json"),
            serde_json::to_string(&json!({
                "id": "resume",
                "run_dir": new,
                "workdir": new,
                "prompt": "continue",
                "resume_from": old,
                "resume_mode": "reuse_contract"
            }))
            .unwrap(),
        )
        .unwrap();

        seed_reused_contract(&old, &new).unwrap();
        record_continuation(&old, &new).unwrap();

        assert_eq!(
            fs::read_to_string(new.join("contract.md")).unwrap(),
            fs::read_to_string(old.join("contract.md")).unwrap()
        );
        assert_eq!(
            read_json_value(&old.join("continuation.json")).unwrap()["run_dir"],
            json!(new)
        );
    }

    #[test]
    fn assigns_local_short_run_ids() {
        let home = temp_run("short-id-home");
        let workdir = temp_run("short-id-workdir");
        let config = Config {
            home: home.clone(),
            ..Config::default()
        };
        let first = TaskInput {
            id: "first".into(),
            prompt: "do first".into(),
            metadata: json!({}),
        };
        let second = TaskInput {
            id: "second".into(),
            prompt: "do second".into(),
            metadata: json!({}),
        };

        let first_dir = init_run_dir(&config, &workdir, &first, None, None).unwrap();
        let second_dir = init_run_dir(&config, &workdir, &second, None, None).unwrap();

        assert_eq!(read_run_spec(&first_dir).unwrap().short_id, Some(1));
        assert_eq!(read_run_spec(&second_dir).unwrap().short_id, Some(2));
        assert_eq!(
            read_json_value(&home.join("run-counter.json")).unwrap()["last"],
            2
        );
        assert_eq!(
            fs::read_to_string(home.join("run-index.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn finalize_without_verdict_marks_failed() {
        let root = temp_root("finalize-root");
        let run = temp_run("finalize-failed");
        let workdir = temp_run("finalize-workdir");
        let run = run.canonicalize().unwrap();
        fs::write(
            run.join("run.json"),
            serde_json::to_string(&json!({
                "id": "finalize-failed",
                "run_dir": run,
                "workdir": workdir,
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        let config = Config {
            home: temp_run("finalize-home"),
            ..Config::default()
        };
        let status = finalize_run(
            &root,
            &workdir,
            &config,
            &TaskInput {
                id: "finalize-failed".into(),
                prompt: "do thing".into(),
                metadata: json!({}),
            },
            &run,
            RunCost {
                unknown: true,
                ..Default::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(status, RunStatus::Failed);
        let result = read_json_value(&run.join("result.json")).unwrap();
        assert_eq!(result["status"], "failed");
        assert_eq!(result["cost_status"], "unknown");
        assert_eq!(
            fs::read_to_string(run.join("sentinel")).unwrap(),
            "failed\n"
        );
    }

    #[test]
    fn same_blocker_twice_stops_for_a_human() {
        // The same external blocker, differing only in volatile timestamp/attempt bits,
        // normalizes identically — the loop's guard fires on the second consecutive hit.
        let first = "Blocked on attempt 1 at 12:03:07 — missing GOOGLE_MAPS_API_KEY credential.";
        let second = "Blocked on attempt 2 at 15:41:52 — missing GOOGLE_MAPS_API_KEY credential.";
        assert!(blockers_equivalent(
            &blocker_fingerprint(first),
            &blocker_fingerprint(second)
        ));

        // Genuinely different failures do not match, so the loop keeps grinding as today.
        let other = "Compile error in src/foo.rs: mismatched types.";
        assert!(!blockers_equivalent(
            &blocker_fingerprint(first),
            &blocker_fingerprint(other)
        ));

        // On the repeated blocker the loop writes a needs_user verdict carrying the text verbatim.
        let run = temp_run("needs-user-blocker");
        let workdir = temp_run("needs-user-workdir");
        fs::write(run.join("contract.md"), complete_contract_fixture()).unwrap();
        fs::write(
            run.join("run.json"),
            serde_json::to_string(&json!({
                "id": "needs-user-blocker",
                "run_dir": run,
                "workdir": workdir,
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        write_needs_user_verdict(&run, second.to_string()).unwrap();
        let verdict = read_json_value(&run.join("verdict.json")).unwrap();
        assert_eq!(verdict["status"], "needs_user");
        assert!(verdict["summary"].as_str().unwrap().contains(second));
        assert_eq!(verdict["criteria_met"], false);
    }

    #[test]
    fn paraphrased_criterion_blockers_stop_without_grinding_attempts() {
        let first = "A14 failed before app launch: required auto-login credentials remain unavailable. A13 also lacks green durable evidence for exact native push-gate chain; release, UI, and CLI jobs exited 1.";
        let second = "A14 failed before app launch because required auto-login credentials remain absent. A13 is also unproven: every exact native push-gate attempt exited 1 due CoreSimulator/SwiftPM sandbox failures.";
        assert!(blockers_equivalent(
            &blocker_fingerprint(first),
            &blocker_fingerprint(second)
        ));
    }

    #[test]
    fn changed_root_causes_under_same_criteria_keep_working() {
        let deployment_and_schema = "A14 lacks credentials. A13 failed because production secret deployment and schema:openapi:diff failed.";
        let native_runtime = "A14 lacks credentials. A13 failed because the native CoreSimulator sandbox is unavailable.";
        assert!(!blockers_equivalent(
            &blocker_fingerprint(deployment_and_schema),
            &blocker_fingerprint(native_runtime)
        ));
    }

    #[test]
    fn different_named_credentials_do_not_collapse() {
        let first = "Missing GOOGLE_MAPS_API_KEY credential.";
        let second = "Missing STRIPE_API_KEY credential.";
        assert!(!blockers_equivalent(
            &blocker_fingerprint(first),
            &blocker_fingerprint(second)
        ));
    }

    #[test]
    fn different_criteria_do_not_collapse() {
        let first = "A13 blocked: external approval is pending.";
        let second = "A14 blocked: external approval is pending.";
        assert!(!blockers_equivalent(
            &blocker_fingerprint(first),
            &blocker_fingerprint(second)
        ));
    }
}
