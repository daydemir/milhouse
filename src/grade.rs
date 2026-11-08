use super::*;
use crate::engine;
use std::os::unix::process::CommandExt;
pub(crate) fn grade_run(config: &Config, run_dir: PathBuf) -> Result<Value, AppError> {
    let contract = run_dir.join("contract.md");
    if !contract.exists() {
        return Err(AppError::Input(format!(
            "missing contract: {}",
            contract.display()
        )));
    }
    let out = run_dir.join("grade-out.json");
    let grade = grade_artifact(&run_dir, &contract, Some(&out))?;
    let result_path = run_dir.join("result.json");
    if result_path.exists() {
        let mut result: Value = serde_json::from_str(&fs::read_to_string(&result_path)?)?;
        result["grade_score"] = grade.get("score").cloned().unwrap_or(Value::Null);
        result["grade_cost_usd"] = grade.get("cost").cloned().unwrap_or_else(|| json!(0.0));
        result["grade_cost_by_tier"] = grade
            .get("cost_by_tier")
            .cloned()
            .unwrap_or_else(|| json!({"fable":0.0,"claude":0.0,"codex":0.0}));
        result["grade_cost_by_model"] = grade
            .get("cost_by_model")
            .cloned()
            .unwrap_or_else(|| json!({}));
        fs::write(&result_path, serde_json::to_string_pretty(&result)? + "\n")?;
    }
    append_grade_cost(config, &run_dir, &grade)?;
    Ok(json!({"ok": true, "run_dir": run_dir, "grade": grade}))
}

fn append_grade_cost(config: &Config, run_dir: &Path, grade: &Value) -> Result<(), AppError> {
    let cost = grade.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    if cost == 0.0 {
        return Ok(());
    }
    append_jsonl(
        &config.home.join("costs.jsonl"),
        &json!({"ts": iso_now(), "kind": "grade", "run_dir": run_dir, "cost": cost, "cost_by_tier": grade.get("cost_by_tier").cloned().unwrap_or_else(|| json!({"fable":0.0,"claude":0.0,"codex":0.0})), "cost_by_model": grade.get("cost_by_model").cloned().unwrap_or_else(|| json!({}))}),
    )
}

pub(crate) fn grade_artifact(
    artifact: &Path,
    criteria: &Path,
    out: Option<&Path>,
) -> Result<Value, AppError> {
    if !artifact.is_dir() {
        return Err(AppError::Input(format!(
            "artifact-dir not a directory: {}",
            artifact.display()
        )));
    }
    if !criteria.is_file() {
        return Err(AppError::Input(format!(
            "criteria file not found: {}",
            criteria.display()
        )));
    }
    if let Ok(stub) = env::var("GRADE_STUB").or_else(|_| env::var("BENCH_SCORER_STUB")) {
        let mut v: Value = serde_json::from_str(&fs::read_to_string(stub)?)?;
        v["cost"] = v.get("cost").cloned().unwrap_or_else(|| json!(0.0));
        v["cost_by_tier"] = v
            .get("cost_by_tier")
            .cloned()
            .unwrap_or_else(|| json!({"fable":0.0,"claude":0.0,"codex":0.0}));
        v["cost_by_model"] = v.get("cost_by_model").cloned().unwrap_or_else(|| json!({}));
        if let Some(out) = out {
            fs::write(out, serde_json::to_string_pretty(&v)? + "\n")?;
        }
        return Ok(v);
    }
    let ctx = format!(
        "===== ACCEPTANCE CRITERIA =====\n{}\n\n===== ARTIFACTS (dir: {}) =====\n{}",
        fs::read_to_string(criteria)?,
        artifact.display(),
        flatten_artifact(artifact)?
    );
    let judges = vec![
        judge(artifact, "contract-correctness", Model::Opus, "Run the acceptance commands in the current directory if runnable, and base correctness on real PASS/FAIL plus the supplied criteria.", &ctx)?,
        judge(artifact, "principles-minimality", Model::Opus, &format!("{PRINCIPLES} Judge only adherence and minimality."), &ctx)?,
        judge(artifact, "evidence-honesty", Model::Opus, "Judge only whether claims rest on evidence present in the artifacts.", &ctx)?,
    ];
    let mut scores: Vec<f64> = judges
        .iter()
        .filter_map(|j| j.get("score").and_then(Value::as_f64))
        .collect();
    scores.sort_by(|a, b| a.partial_cmp(b).unwrap_or(Ordering::Equal));
    let score = scores.get(1).copied().unwrap_or(0.0);
    let cost = round4(
        judges
            .iter()
            .filter_map(|j| j.get("cost").and_then(Value::as_f64))
            .sum(),
    );
    let cost_by_tier = cost_by_tier(&judges);
    let cost_by_model = cost_by_model(&judges);
    let result = json!({"score": score, "judges": judges, "cost": cost, "cost_by_tier": cost_by_tier, "cost_by_model": cost_by_model, "grader": "rust-v1", "models": {"contract_correctness": Model::Opus.id()?, "principles_minimality": Model::Opus.id()?, "evidence_honesty": Model::Opus.id()?, "arbiter": Model::Opus.id()?}});
    if let Some(out) = out {
        fs::write(out, serde_json::to_string_pretty(&result)? + "\n")?;
    }
    Ok(result)
}

fn judge(
    artifact: &Path,
    lens: &str,
    model: Model,
    instr: &str,
    ctx: &str,
) -> Result<Value, AppError> {
    let prompt = format!("You are a strict, BLIND grader applying exactly ONE lens: {lens}.\n{instr}\nScore this ONE attempt 0-100 THROUGH YOUR LENS ONLY. Cite concrete evidence. Output ONLY raw JSON conforming to this schema:\n{GRADE_SCHEMA}\n\n{ctx}");
    for _ in 0..2 {
        let raw = claude_print(artifact, model, &prompt, true)?;
        if let Some(v) = extract_score_json(&raw) {
            let cost = metered_claude_cost(model, &raw);
            return Ok(
                json!({"lens": lens, "model": model.id()?, "tier": model.tier(), "score": v["score"].clone(), "citations": v["citations"].clone(), "cost": cost}),
            );
        }
    }
    Err(AppError::External(format!(
        "judge {lens} produced no valid score"
    )))
}

pub(crate) fn claude_print(
    cwd: &Path,
    model: Model,
    prompt: &str,
    json_out: bool,
) -> Result<String, AppError> {
    let agent = model.details()?;
    let bin = env::var("CLAUDE_BIN").unwrap_or_else(|_| "claude".into());
    let mut cmd = ProcessCommand::new(bin);
    cmd.current_dir(cwd)
        .arg("-p")
        .arg("--model")
        .arg(&agent.model)
        .arg("--effort")
        .arg(agent.effort.as_str())
        .arg("--dangerously-skip-permissions");
    if json_out {
        cmd.arg("--output-format").arg("json");
    }
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    cmd.process_group(0);
    scrub_env(&mut cmd);
    if agent.auth == AgentAuth::Api {
        cmd.env(
            "ANTHROPIC_API_KEY",
            engine::read_env_key(&engine::anthropic_key_file()?, "ANTHROPIC_API_KEY")?,
        );
    }
    let mut child = cmd.spawn()?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| AppError::External("missing claude stdin".into()))?;
    stdin.write_all(prompt.as_bytes())?;
    drop(stdin);
    let output = wait_output(
        child,
        child_timeout("MILHOUSE_VERIFIER_TIMEOUT", 900),
        "claude verifier",
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

fn cost_by_tier(judges: &[Value]) -> Value {
    let mut out = json!({"fable":0.0,"claude":0.0,"codex":0.0});
    for j in judges {
        let tier = j.get("tier").and_then(Value::as_str).unwrap_or("claude");
        let cost = j.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
        out[tier] = json!(round4(out[tier].as_f64().unwrap_or(0.0) + cost));
    }
    out
}

fn cost_by_model(judges: &[Value]) -> Value {
    let mut out = json!({});
    for j in judges {
        let model = j.get("model").and_then(Value::as_str).unwrap_or("");
        let cost = j.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
        if !model.is_empty() {
            out[model] = json!(round4(
                out.get(model).and_then(Value::as_f64).unwrap_or(0.0) + cost
            ));
        }
    }
    out
}

pub(crate) fn claude_cost(raw: &str) -> f64 {
    serde_json::from_str::<Value>(raw)
        .ok()
        .and_then(|v| v.get("total_cost_usd").and_then(Value::as_f64))
        .unwrap_or(0.0)
}

fn metered_claude_cost(model: Model, raw: &str) -> f64 {
    match model {
        Model::Fable => claude_cost(raw),
        Model::Opus | Model::Codex => 0.0,
    }
}

fn scrub_env(cmd: &mut ProcessCommand) {
    for k in [
        "ANTHROPIC_API_KEY",
        "ANTHROPIC_AUTH_TOKEN",
        "CLAUDE_CODE_OAUTH_TOKEN",
        "CLAUDECODE",
        "OPENAI_API_KEY",
        "OPENAI_BASE_URL",
        "CODEX_API_KEY",
        "CLAUDE_CODE_DISABLE_WORKFLOWS",
    ] {
        cmd.env_remove(k);
    }
}

pub(crate) fn extract_score_json(raw: &str) -> Option<Value> {
    if let Ok(v) = serde_json::from_str::<Value>(raw) {
        if let Some(result) = v.get("result").and_then(Value::as_str) {
            if let Some(score) = extract_score_json_from_text(result) {
                return Some(score);
            }
        }
        if valid_score_json(&v) {
            return Some(v);
        }
    }
    extract_score_json_from_text(raw)
}

fn extract_score_json_from_text(raw: &str) -> Option<Value> {
    let mut best = None;
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
                    if let Ok(v) = serde_json::from_str::<Value>(s) {
                        if valid_score_json(&v) {
                            best = Some(v);
                        }
                    }
                    start = None;
                }
            }
            _ => {}
        }
    }
    best
}

fn valid_score_json(v: &Value) -> bool {
    let Some(obj) = v.as_object() else {
        return false;
    };
    if obj.len() != 2 {
        return false;
    }
    let Some(score) = obj.get("score").and_then(Value::as_f64) else {
        return false;
    };
    (0.0..=100.0).contains(&score)
        && obj
            .get("citations")
            .and_then(Value::as_array)
            .is_some_and(|a| {
                !a.is_empty() && a.iter().all(|c| c.as_str().is_some_and(|s| !s.is_empty()))
            })
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;

    #[test]
    fn extracts_score_from_claude_result_string() {
        let raw = json!({
            "type": "result",
            "result": "notes {\"score\":77,\"citations\":[\"pass\"]}",
            "total_cost_usd": 0.1
        })
        .to_string();

        let v = extract_score_json(&raw).unwrap();
        assert_eq!(v["score"], 77);
        assert_eq!(v["citations"][0], "pass");
    }

    #[test]
    fn rejects_non_strict_score_payloads() {
        for raw in [
            r#"{"score":101,"citations":["a"]}"#,
            r#"{"score":90,"citations":[]}"#,
            r#"{"score":90,"citations":["a"],"extra":true}"#,
        ] {
            assert!(extract_score_json(raw).is_none());
        }
    }

    #[test]
    fn charges_only_api_metered_grader_models() {
        let raw = r#"{"total_cost_usd":1.25}"#;
        assert_eq!(metered_claude_cost(Model::Fable, raw), 1.25);
        assert_eq!(metered_claude_cost(Model::Opus, raw), 0.0);
    }
}

fn flatten_artifact(dir: &Path) -> Result<String, AppError> {
    let mut out = String::new();
    if git_toplevel(dir).as_deref() == Some(dir)
        && ProcessCommand::new("git")
            .arg("-C")
            .arg(dir)
            .arg("rev-parse")
            .arg("origin/main")
            .output()
            .is_ok_and(|o| o.status.success())
    {
        out.push_str("== git diff origin/main...HEAD (capped) ==\n");
        out.push_str(&cmd_text(
            ProcessCommand::new("git")
                .arg("-C")
                .arg(dir)
                .arg("diff")
                .arg("origin/main...HEAD"),
            60_000,
        ));
        out.push_str("\n== deliverables ==\n");
        for f in ["BRIEF.md", "TRIAGE.md", "ANSWERS.md", "decisions.md"] {
            push_file_cap(&mut out, &dir.join(f), 60_000)?;
        }
        return Ok(out);
    }
    let mut total = 0usize;
    for p in walk_files(dir)? {
        if p.components()
            .any(|c| c.as_os_str() == ".git" || c.as_os_str() == "__pycache__")
        {
            continue;
        }
        let ext = p
            .extension()
            .and_then(OsStr::to_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        if matches!(
            ext.as_str(),
            "png"
                | "jpg"
                | "jpeg"
                | "gif"
                | "pdf"
                | "zip"
                | "mp3"
                | "mp4"
                | "bin"
                | "pyc"
                | "pyo"
                | "so"
        ) {
            continue;
        }
        let meta = fs::metadata(&p)?;
        if meta.len() > 60_000 {
            continue;
        }
        total += meta.len() as usize;
        if total > 120_000 {
            break;
        }
        out.push_str(&format!(
            "== {} ==\n",
            p.strip_prefix(dir).unwrap_or(&p).display()
        ));
        out.push_str(&fs::read_to_string(&p).unwrap_or_default());
        out.push('\n');
    }
    Ok(out)
}

fn push_file_cap(out: &mut String, path: &Path, cap: usize) -> Result<(), AppError> {
    if path.exists() {
        out.push_str(&format!(
            "----- {} -----\n",
            path.file_name().and_then(OsStr::to_str).unwrap_or("file")
        ));
        out.push_str(&read_cap(path, cap)?);
        out.push('\n');
    }
    Ok(())
}
