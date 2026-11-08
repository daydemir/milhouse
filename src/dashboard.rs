use super::*;
pub(crate) fn dashboard(root: &Path, config: &Config, port: u16) -> Result<(), AppError> {
    let serve = config.home.join("dash");
    build_dashboard_data(root, config, &serve)?;
    fs::copy(root.join("dash/index.html"), serve.join("index.html"))?;
    let root = root.to_path_buf();
    let server_root = root.clone();
    let fallback_config = config.clone();
    let home = config.home.clone();
    let config_path = home.join("config.json");
    let serve_for_thread = serve.clone();
    thread::spawn(move || loop {
        let live = load_config(Some(&config_path)).unwrap_or_else(|_| fallback_config.clone());
        let _ = build_dashboard_data(&root, &live, &serve_for_thread);
        thread::sleep(Duration::from_secs(10));
    });
    eprintln!(
        "mil dashboard → http://127.0.0.1:{port}/   (serving {}, data refreshed every 10s)",
        serve.display()
    );
    serve_http(server_root, serve, home, port)
}

fn build_dashboard_data(root: &Path, config: &Config, serve: &Path) -> Result<(), AppError> {
    fs::create_dir_all(serve)?;
    let runs_dir = config.home.join("runs");
    let mut runs_all = Vec::new();
    if runs_dir.is_dir() {
        for e in fs::read_dir(&runs_dir)?.filter_map(Result::ok) {
            if e.path().is_dir() {
                finalize_dead_run(&e.path())?;
                runs_all.push(build_run_light(&e.path())?);
            }
        }
    }
    runs_all.sort_by_key(|r| r.get("start").and_then(Value::as_u64).unwrap_or(0));
    let terminal: Vec<_> = runs_all
        .iter()
        .filter(|r| !r.get("running").and_then(Value::as_bool).unwrap_or(false))
        .collect();
    let green = terminal
        .iter()
        .filter(|r| r.get("status").and_then(Value::as_str) == Some("green"))
        .count();
    let detail = runs_all
        .iter()
        .rev()
        .take(15)
        .map(build_run_detail)
        .collect::<Result<Vec<_>, _>>()?;
    let cfg = config_json(config)?;
    let rows = read_jsonl(&root.join("parity/scores.jsonl"));
    let (spent_today, spend_history) = crate::spend::dashboard_values(config);
    let usage_limits = usage_limits_snapshot(&config.home);
    let data = json!({
        "generated": iso_now(),
        "summary": {"total": terminal.len(), "green": green, "failed": terminal.len().saturating_sub(green), "green_rate": if terminal.is_empty(){0}else{100*green/terminal.len()}, "pushed": terminal.iter().filter(|r| r.get("pushed").and_then(Value::as_bool)==Some(true)).count(), "cost": round2(terminal.iter().filter_map(|r| r.get("cost").and_then(Value::as_f64)).sum())},
        "parity": parity_rows(&rows),
        "bench": build_bench_data(&rows),
        "config": cfg,
        "active_run": active_run(&config.home),
        "spent_today": spent_today,
        "spend_history": spend_history,
        "usage_limits": usage_limits,
        "prompts": prompts_snapshot(root, &config.home),
        "checks": checks_snapshot(root, config),
        "runs": detail,
    });
    atomic_write(
        &serve.join("data.json"),
        &(serde_json::to_string(&data)? + "\n"),
    )
}

const USAGE_LIMITS_CACHE_SECS: u64 = 10 * 60;

fn usage_limits_snapshot(home: &Path) -> Value {
    let path = home.join("usage-limits-cache.json");
    if let Some(cache) = read_json_value(&path) {
        let ts = cache
            .get("generated_at")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        if epoch().saturating_sub(ts) < USAGE_LIMITS_CACHE_SECS {
            return cache;
        }
    }
    let snapshot = json!({
        "generated_at": epoch(),
        "claude": claude_usage().unwrap_or_else(|e| json!({"error": e.to_string()})),
        "codex": codex_usage().unwrap_or_else(|e| json!({"error": e.to_string()})),
    });
    let _ = atomic_write(
        &path,
        &(serde_json::to_string_pretty(&snapshot).unwrap_or_default() + "\n"),
    );
    snapshot
}

fn claude_usage() -> Result<Value, AppError> {
    let child = ProcessCommand::new("claude")
        .args(["-p", "/usage", "--output-format", "json"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let out = wait_output(child, Duration::from_secs(20), "claude usage")?;
    if !out.status.success() {
        return Err(AppError::External(
            String::from_utf8_lossy(&out.stderr).trim().to_string(),
        ));
    }
    let raw: Value = serde_json::from_slice(&out.stdout)?;
    parse_claude_usage(raw.get("result").and_then(Value::as_str).unwrap_or(""))
}

fn parse_claude_usage(text: &str) -> Result<Value, AppError> {
    let session = claude_percent_reset(text, "Current session:")?;
    let week = claude_percent_reset(text, "Current week (all models):")?;
    let fable = text
        .lines()
        .find_map(|l| l.trim().strip_prefix("Current week (Fable):"))
        .and_then(percent_from)
        .ok_or_else(|| AppError::External("claude usage missing Fable week".into()))?;
    Ok(json!({
        "session_percent": session.0,
        "session_reset": session.1,
        "week_percent": week.0,
        "week_reset": week.1,
        "fable_week_percent": fable,
        "error": Value::Null,
    }))
}

fn claude_percent_reset(text: &str, prefix: &str) -> Result<(u64, String), AppError> {
    let line = text
        .lines()
        .find_map(|l| l.trim().strip_prefix(prefix))
        .ok_or_else(|| AppError::External(format!("claude usage missing {prefix}")))?;
    let pct = percent_from(line)
        .ok_or_else(|| AppError::External(format!("claude usage missing percent for {prefix}")))?;
    let reset = line
        .split("resets ")
        .nth(1)
        .map(str::trim)
        .unwrap_or("")
        .to_string();
    Ok((pct, reset))
}

fn percent_from(s: &str) -> Option<u64> {
    let before = s.split('%').next()?.trim();
    before
        .rsplit(|c: char| !c.is_ascii_digit())
        .next()?
        .parse()
        .ok()
}

fn codex_usage() -> Result<Value, AppError> {
    codex_usage_from_rate_limits(&latest_codex_rate_limits()?)
}

fn codex_usage_from_rate_limits(rl: &Value) -> Result<Value, AppError> {
    let p = rl.get("primary").unwrap_or(&Value::Null);
    let s = rl.get("secondary").unwrap_or(&Value::Null);
    Ok(json!({
        "primary_percent": window_percent(p),
        "primary_window_minutes": window_minutes(p),
        "primary_resets_at": window_reset(p),
        "secondary_percent": window_percent(s),
        "secondary_window_minutes": window_minutes(s),
        "secondary_resets_at": window_reset(s),
        "plan_type": rl.get("plan_type").cloned().unwrap_or(Value::Null),
        "error": Value::Null,
    }))
}

fn window_percent(v: &Value) -> Value {
    v.get("used_percent")
        .or_else(|| v.get("usedPercent"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn window_minutes(v: &Value) -> Value {
    v.get("window_minutes")
        .or_else(|| v.get("windowDurationMins"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn window_reset(v: &Value) -> Value {
    v.get("resets_at")
        .or_else(|| v.get("resetsAt"))
        .cloned()
        .unwrap_or(Value::Null)
}

fn latest_codex_rate_limits() -> Result<Value, AppError> {
    let home = env::var("HOME").map_err(|_| AppError::External("HOME unset".into()))?;
    let codex = PathBuf::from(home).join(".codex");
    let mut files = Vec::new();
    collect_jsonl(&codex.join("sessions"), &mut files);
    collect_jsonl(&codex.join("archived_sessions"), &mut files);
    let mut best: Option<(String, Value)> = None;
    for file in files {
        let Ok(lines) = fs::read_to_string(file) else {
            continue;
        };
        for line in lines.lines().filter(|l| l.contains("\"rate_limits\"")) {
            let Ok(v) = serde_json::from_str::<Value>(line) else {
                continue;
            };
            let Some(rl) = v
                .get("rate_limits")
                .or_else(|| v.get("payload").and_then(|p| p.get("rate_limits")))
            else {
                continue;
            };
            let ts = v
                .get("timestamp")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            if best.as_ref().is_none_or(|b| ts > b.0) {
                best = Some((ts, rl.clone()));
            }
        }
    }
    best.map(|(_, rl)| rl)
        .ok_or_else(|| AppError::External("no codex rate limit events found".into()))
}

fn collect_jsonl(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for e in entries.filter_map(Result::ok) {
        let p = e.path();
        if p.is_dir() {
            collect_jsonl(&p, out);
        } else if p.extension().and_then(OsStr::to_str) == Some("jsonl") {
            out.push(p);
        }
    }
}

fn finalize_dead_run(run: &Path) -> Result<(), AppError> {
    if !run.join("run.json").is_file() {
        return Ok(());
    }
    let _state = match lock_with_grace(&run.join("run.json"), Duration::from_secs(2)) {
        Ok(state) => state,
        Err(_) => return Ok(()),
    };
    let recorded_pid = read_text(&run.join("run.pid"))
        .ok()
        .and_then(|value| value.trim().parse::<i32>().ok())
        .or_else(|| {
            read_text(&run.join(ACTIVE_CHILD_PID_FILE))
                .ok()
                .and_then(|value| value.trim().parse::<i32>().ok())
        });
    let runner = match tracked_pid(run, "run.pid", Duration::ZERO) {
        Ok(pid) => pid,
        Err(_) => return Ok(()),
    };
    let child = match tracked_pid(run, ACTIVE_CHILD_PID_FILE, Duration::ZERO) {
        Ok(pid) => pid,
        Err(_) => return Ok(()),
    };
    let active_pid = [runner, child].into_iter().find(|pid| *pid > 1);
    if run.join("result.json").is_file() || run.join("sentinel").is_file() {
        if recorded_pid.is_some() && active_pid.is_none() {
            let _ = fs::remove_file(run.join("run.pid"));
            let _ = fs::remove_file(run.join(ACTIVE_CHILD_PID_FILE));
        }
        return Ok(());
    }
    let Some(pid) = recorded_pid else {
        return Ok(());
    };
    if active_pid.is_some() {
        return Ok(());
    }
    let spec = read_run_spec(run)?;
    let reason = format!("run pid {pid} is no longer alive");
    let result = terminal_result(RunStatus::FailedStale, spec.id, &reason, run);
    atomic_write(
        &run.join("result.json"),
        &(serde_json::to_string_pretty(&result)? + "\n"),
    )?;
    atomic_write(&run.join("sentinel"), "failed_stale\n")?;
    let _ = fs::remove_file(run.join("run.pid"));
    let _ = fs::remove_file(run.join(ACTIVE_CHILD_PID_FILE));
    Ok(())
}

pub(crate) fn build_run_light(run: &Path) -> Result<Value, AppError> {
    let base = run
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("")
        .to_string();
    let result = read_json_value(&run.join("result.json")).unwrap_or_else(|| json!({}));
    let cost_source = if result.get("status").is_some() {
        result.clone()
    } else {
        read_json_value(&run.join("progress.json")).unwrap_or_else(|| json!({}))
    };
    let spec = read_json_value(&run.join("run.json")).unwrap_or_else(|| json!({}));
    let sentinel = read_text(&run.join("sentinel"))
        .unwrap_or_default()
        .trim()
        .to_string();
    let pid_alive = run_pid(run).is_some_and(process_alive);
    let running = sentinel.is_empty() && result.get("status").is_none() && pid_alive;
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            if running {
                "running".into()
            } else if sentinel.is_empty()
                && result.get("status").is_none()
                && run.join("run.pid").is_file()
            {
                "stale".into()
            } else if sentinel.is_empty() {
                "unknown".into()
            } else {
                sentinel
            }
        });
    let start = run_dir_epoch(&base);
    let cost = cost_source
        .get("claude_cost_usd")
        .and_then(Value::as_f64)
        .unwrap_or(0.0)
        + cost_source
            .get("grade_cost_usd")
            .and_then(Value::as_f64)
            .unwrap_or(0.0);
    let cost_by_tier = merged_cost_by_tier(
        cost_source.get("cost_by_tier").unwrap_or(&Value::Null),
        cost_source
            .get("grade_cost_by_tier")
            .unwrap_or(&Value::Null),
    );
    let cost_by_model = merged_cost_by_model(
        cost_source.get("cost_by_model").unwrap_or(&Value::Null),
        cost_source
            .get("grade_cost_by_model")
            .unwrap_or(&Value::Null),
    );
    let cancelable = cancelable_status(&status, running);
    let last_activity = latest_modified(run)
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let idle_seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .saturating_sub(last_activity);
    let current_activity = current_codex_activity(run).unwrap_or_default();
    Ok(
        json!({"base": base, "short_id": spec.get("short_id").cloned().unwrap_or(Value::Null), "run": run, "issue": result.get("issue").or_else(|| cost_source.get("issue")).or_else(|| spec.get("id")).cloned().unwrap_or(Value::String(base)), "status": status, "running": running, "cancelable": cancelable, "stage": read_text(&run.join("checkpoint")).unwrap_or_default(), "reason": result.get("reason").cloned().unwrap_or(Value::String(String::new())), "commit": result.get("commit").cloned().unwrap_or(Value::String(String::new())), "pushed": result.get("pushed").and_then(Value::as_bool).unwrap_or(false), "cost": cost, "milhouse_cost": cost_source.get("milhouse_cost_usd").or_else(|| cost_source.get("oneshot_cost_usd")).or_else(|| cost_source.get("claude_cost_usd")).cloned().unwrap_or_else(|| json!(0.0)), "grade_cost": cost_source.get("grade_cost_usd").cloned().unwrap_or_else(|| json!(0.0)), "cost_by_model": cost_by_model, "cost_by_tier": cost_by_tier, "tokens": cost_source.get("claude_tokens").and_then(Value::as_u64).unwrap_or(0), "nsteps": result.get("steps").and_then(Value::as_u64).unwrap_or(0), "retries": result.get("retries").and_then(Value::as_u64).unwrap_or(0), "start": start, "end": start, "duration": duration_secs(run), "last_activity": last_activity, "idle_seconds": idle_seconds, "current_activity": current_activity}),
    )
}

fn cancelable_status(status: &str, running: bool) -> bool {
    running || status == "stale"
}

fn merged_cost_by_tier(a: &Value, b: &Value) -> Value {
    let mut out = json!({});
    for tier in ["fable", "claude", "codex"] {
        out[tier] = json!(round4(
            a.get(tier).and_then(Value::as_f64).unwrap_or(0.0)
                + b.get(tier).and_then(Value::as_f64).unwrap_or(0.0)
        ));
    }
    out
}

fn merged_cost_by_model(a: &Value, b: &Value) -> Value {
    let mut out = json!({});
    for source in [a, b] {
        if let Some(obj) = source.as_object() {
            for (model, cost) in obj {
                out[model] = json!(round4(
                    out.get(model).and_then(Value::as_f64).unwrap_or(0.0)
                        + cost.as_f64().unwrap_or(0.0)
                ));
            }
        }
    }
    out
}

fn build_run_detail(light: &Value) -> Result<Value, AppError> {
    let run = PathBuf::from(light.get("run").and_then(Value::as_str).unwrap_or(""));
    let spec = read_json_value(&run.join("run.json")).unwrap_or_else(|| json!({}));
    let card = read_json_value(&run.join("card.json")).unwrap_or_else(|| json!({}));
    let grade = read_json_value(&run.join("grade-out.json")).unwrap_or_else(|| json!({}));
    let mut v = light.clone();
    v.as_object_mut().unwrap().remove("run");
    v["title"] = card
        .get("title")
        .cloned()
        .unwrap_or(Value::String(String::new()));
    v["desc"] = Value::String(
        spec.get("prompt")
            .or_else(|| card.get("prompt"))
            .or_else(|| card.get("description"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .chars()
            .take(700)
            .collect(),
    );
    v["grade"] = json!({"score": grade.get("score"), "judges": grade.get("judges").cloned().unwrap_or_else(|| json!([]))});
    v["evidence"] = json!({"shots": [], "log_tail": tail_lines(&run_log_path(&run), 40), "gates_tail": tail_lines(&run.join("gates.log"), 30)});
    v["activity"] = build_run_activity(&run);
    let status = light.get("status").and_then(Value::as_str).unwrap_or("");
    let running = light
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    v["stages"] = run_stages(&run, status, running, &spec);
    v["events"] = run_events(&run);
    Ok(v)
}

// Build <-> Verify stage tracker derived from the frozen contract, progress, and verifier files.
fn run_stages(run: &Path, status: &str, running: bool, spec: &Value) -> Value {
    let build_model = spec
        .get("build_profile")
        .and_then(Value::as_str)
        .unwrap_or("sol-6.1-low-subscription")
        .to_string();
    let verify_model = spec
        .get("verify_profile")
        .and_then(Value::as_str)
        .unwrap_or("sol-6.1-high-subscription")
        .to_string();
    let progress = read_json_value(&run.join("progress.json")).unwrap_or_else(|| json!({}));
    let phase = progress.get("phase").and_then(Value::as_str).unwrap_or("");
    let attempt = progress.get("attempt").and_then(Value::as_u64).unwrap_or(0);
    let contract_ready = run.join("contract.md").is_file();
    let verifier_attempts = numbered_files(run, "verifier-", ".json");
    let last_verifier = verifier_attempts
        .last()
        .and_then(|(_, p)| read_json_value(p));
    let verified = last_verifier
        .as_ref()
        .and_then(|v| v.get("passed").and_then(Value::as_bool))
        == Some(true)
        || run.join("verdict.json").is_file() && status == "green";
    let has_new_artifacts = contract_ready || !progress.as_object().unwrap().is_empty();

    // Legacy runs without contract/progress/verifier files infer stages from the outcome.
    let (build, verify) = if !has_new_artifacts {
        match status {
            "green" => ("passed", "passed"),
            "running" => ("running", "pending"),
            "canceled" => ("canceled", "—"),
            "" | "unknown" | "stale" => ("pending", "pending"),
            _ => ("failed", "failed"),
        }
    } else {
        let build = if verified {
            "passed"
        } else if status == "hit_run_limit" || status.starts_with("failed") {
            "exhausted"
        } else if status == "canceled" {
            "canceled"
        } else if running && phase == "codex_loop" {
            "running"
        } else if contract_ready {
            "pending"
        } else {
            "—"
        };
        let verify = if verified {
            "passed"
        } else if last_verifier.is_some() {
            "failed"
        } else if running && phase == "verifier" {
            "running"
        } else if status == "canceled" {
            "—"
        } else {
            "pending"
        };
        (build, verify)
    };
    let attempt_note = if attempt > 0 {
        format!(" · attempt {attempt}")
    } else {
        String::new()
    };
    json!([
        {"name": "Build", "state": build, "model": format!("{build_model}{attempt_note}")},
        {"name": "Verify", "state": verify, "model": verify_model},
    ])
}

// A readable, honest event stream reconstructed from run-dir files, ordered by mtime. Each event
// is a real artifact the engine produced — no fabricated timeline.
fn run_events(run: &Path) -> Value {
    let mut events: Vec<(u64, &str, String)> = Vec::new();
    let at = |p: &Path| file_mtime(p);
    if run.join("contract.md").is_file() {
        events.push((
            at(&run.join("contract.md")),
            "contract",
            "contract.md frozen for execution.".into(),
        ));
    }
    for (n, p) in numbered_files(run, "codex-session-", ".jsonl") {
        events.push((at(&p), "build", format!("Build attempt {n} ran (codex).")));
    }
    for (n, p) in numbered_files(run, "verifier-", ".json") {
        let v = read_json_value(&p).unwrap_or_else(|| json!({}));
        let passed = v.get("passed").and_then(Value::as_bool).unwrap_or(false);
        let notes = v.get("notes").and_then(Value::as_str).unwrap_or("");
        if passed {
            events.push((
                at(&p),
                "pass",
                format!("Verify {n} passed — contract satisfied."),
            ));
        } else {
            events.push((
                at(&p),
                "fail",
                format!("Verify {n} failed — {}", snippet(notes, 180)),
            ));
        }
    }
    if let Some(v) = read_json_value(&run.join("verdict.json")) {
        let s = v.get("status").and_then(Value::as_str).unwrap_or("");
        events.push((
            at(&run.join("verdict.json")),
            if s == "green" { "pass" } else { "fail" },
            format!("Verdict: {}", verdict_snippet(&v)),
        ));
    }
    if let Some(v) = read_json_value(&run.join("result.json")) {
        let s = v.get("status").and_then(Value::as_str).unwrap_or("");
        let kind = if s == "green" { "pass" } else { "fail" };
        events.push((
            at(&run.join("result.json")),
            kind,
            format!("Run {}", result_snippet(&v).replace('\n', " · ")),
        ));
    }
    events.sort_by_key(|e| e.0);
    Value::Array(
        events
            .into_iter()
            .map(|(ts, kind, text)| json!({"ts": ts, "kind": kind, "text": text}))
            .collect(),
    )
}

fn file_mtime(p: &Path) -> u64 {
    fs::metadata(p)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn current_codex_activity(run: &Path) -> Option<String> {
    let (_, path) = numbered_files(run, "codex-session-", ".jsonl").pop()?;
    let event: Value = serde_json::from_str(fs::read_to_string(path).ok()?.lines().last()?).ok()?;
    if event.get("type").and_then(Value::as_str) != Some("item.started") {
        return None;
    }
    let item = event.get("item")?;
    let detail = item
        .get("command")
        .or_else(|| item.get("text"))
        .and_then(Value::as_str)?;
    Some(snippet(detail.lines().next().unwrap_or(detail), 120))
}

// Files like `verifier-3.json` -> (3, path), sorted ascending by number.
fn numbered_files(run: &Path, prefix: &str, suffix: &str) -> Vec<(u32, PathBuf)> {
    let mut out: Vec<(u32, PathBuf)> = fs::read_dir(run)
        .into_iter()
        .flatten()
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let n = name
                .strip_prefix(prefix)?
                .strip_suffix(suffix)?
                .parse()
                .ok()?;
            Some((n, e.path()))
        })
        .collect();
    out.sort_by_key(|(n, _)| *n);
    out
}

fn build_run_activity(run: &Path) -> Value {
    let mut items = Vec::new();
    if let Ok(text) = read_text(&run.join("checkpoint")) {
        push_activity(&mut items, "checkpoint", "checkpoint", "checkpoint", text);
    }
    let log = run_log_path(run);
    push_activity(
        &mut items,
        "log",
        "recent log",
        log.file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("milhouse.log"),
        tail_lines(&log, 14).join("\n"),
    );
    for path in session_files(run).into_iter().rev().take(2) {
        let title = path
            .file_name()
            .and_then(OsStr::to_str)
            .unwrap_or("session");
        push_activity(
            &mut items,
            "session",
            title,
            title,
            session_snippet(&path).unwrap_or_default(),
        );
    }
    if let Some(verdict) = read_json_value(&run.join("verdict.json")) {
        push_activity(
            &mut items,
            "verdict",
            "verdict",
            "verdict.json",
            verdict_snippet(&verdict),
        );
    }
    for name in ["worker-result.json", "result.json"] {
        if let Some(result) = read_json_value(&run.join(name)) {
            push_activity(&mut items, "result", name, name, result_snippet(&result));
        }
    }
    Value::Array(items)
}

fn run_log_path(run: &Path) -> PathBuf {
    let current = run.join("milhouse.log");
    if current.is_file() {
        current
    } else {
        run.join("oneshot.log")
    }
}

fn push_activity(items: &mut Vec<Value>, kind: &str, title: &str, file: &str, text: String) {
    let text = snippet(&text, 1200);
    if !text.is_empty() {
        items.push(json!({"kind": kind, "title": title, "file": file, "text": text}));
    }
}

fn session_files(run: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(run)
        .ok()
        .into_iter()
        .flat_map(|rd| rd.filter_map(Result::ok))
        .map(|e| e.path())
        .filter(|p| {
            p.file_name().and_then(OsStr::to_str).is_some_and(|n| {
                n.ends_with("session.json")
                    || n.ends_with("session.jsonl")
                    || n.starts_with("codex-session-")
            })
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn session_snippet(path: &Path) -> Option<String> {
    if path.extension().and_then(OsStr::to_str) == Some("jsonl") {
        let lines = read_jsonl(path)
            .into_iter()
            .rev()
            .filter_map(|v| {
                v.get("result")
                    .or_else(|| v.get("text"))
                    .or_else(|| v.get("content"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .take(4)
            .collect::<Vec<_>>();
        return Some(lines.into_iter().rev().collect::<Vec<_>>().join("\n\n"));
    }
    let v = read_json_value(path)?;
    Some(
        v.get("result")
            .or_else(|| v.get("summary"))
            .or_else(|| v.get("text"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| serde_json::to_string_pretty(&v).unwrap_or_default()),
    )
}

fn verdict_snippet(v: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(status) = v.get("status").and_then(Value::as_str) {
        parts.push(format!("status: {status}"));
    }
    if let Some(criteria) = v.get("criteria_met").and_then(Value::as_bool) {
        parts.push(format!("criteria met: {criteria}"));
    }
    if let Some(summary) = v.get("summary").and_then(Value::as_str) {
        parts.push(summary.to_string());
    }
    parts.join("\n")
}

fn result_snippet(v: &Value) -> String {
    let mut parts = Vec::new();
    for key in ["status", "issue", "reason", "commit"] {
        if let Some(s) = v.get(key).and_then(Value::as_str) {
            if !s.is_empty() {
                parts.push(format!("{key}: {s}"));
            }
        }
    }
    if let Some(pushed) = v.get("pushed").and_then(Value::as_bool) {
        parts.push(format!("pushed: {pushed}"));
    }
    if let Some(summary) = v.get("summary").and_then(Value::as_str) {
        parts.push(summary.to_string());
    }
    if parts.is_empty() {
        serde_json::to_string_pretty(v).unwrap_or_default()
    } else {
        parts.join("\n")
    }
}

fn snippet(text: &str, cap: usize) -> String {
    let s = text.trim();
    if s.chars().count() <= cap {
        return s.to_string();
    }
    s.chars().take(cap).collect::<String>() + "..."
}

fn serve_http(root: PathBuf, serve: PathBuf, home: PathBuf, port: u16) -> Result<(), AppError> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    for stream in listener.incoming() {
        let stream = stream?;
        let _ = handle_http(stream, &root, &serve, &home);
    }
    Ok(())
}

fn handle_http(
    mut stream: TcpStream,
    root: &Path,
    serve: &Path,
    home: &Path,
) -> Result<(), AppError> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    let parts: Vec<_> = first.split_whitespace().collect();
    if parts.len() < 2 {
        return Ok(());
    }
    let method = parts[0];
    let target = parts[1];
    let path = target
        .split('?')
        .next()
        .unwrap_or("/")
        .trim_end_matches('/');
    let mut len = 0usize;
    loop {
        let mut line = String::new();
        reader.read_line(&mut line)?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
            len = v.trim().parse().unwrap_or(0);
        }
    }
    if method == "POST" {
        if len > BODY_CAP {
            return http_json(&mut stream, 413, &json!({"ok": false, "msg": "too large"}));
        }
        let mut body = vec![0; len];
        reader.read_exact(&mut body)?;
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        return match path {
            "/config" | "/mil/config" => post_config(home, body, &mut stream),
            "/prompt" | "/mil/prompt" => post_prompt(root, home, body, &mut stream),
            "/cancel" | "/mil/cancel" => post_cancel(home, body, &mut stream),
            _ => http_json(
                &mut stream,
                404,
                &json!({"ok": false, "msg": "unknown route"}),
            ),
        };
    }
    if path == "/version" || path == "/mil/version" {
        return get_version(home, target, &mut stream);
    }
    let rel = if path.is_empty() || path == "/" || path == "/mil" {
        "index.html"
    } else {
        path.trim_start_matches("/mil/").trim_start_matches('/')
    };
    let Some(p) = safe_static_path(serve, rel) else {
        return http_json(&mut stream, 404, &json!({"ok": false}));
    };
    if p.is_file() {
        http_bytes(&mut stream, 200, content_type(&p), &fs::read(p)?)
    } else {
        http_json(&mut stream, 404, &json!({"ok": false}))
    }
}

fn safe_static_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = Path::new(rel);
    rel.components()
        .all(|part| matches!(part, Component::Normal(_)))
        .then(|| root.join(rel))
}

fn post_config(home: &Path, body: Value, stream: &mut TcpStream) -> Result<(), AppError> {
    let Some(obj) = body.as_object() else {
        return http_json(stream, 400, &json!({"ok": false, "msg": "bad json"}));
    };
    if obj.is_empty() || obj.iter().any(|(k, v)| !valid_config(k, v)) {
        return http_json(
            stream,
            400,
            &json!({"ok": false, "msg": "invalid keys/values"}),
        );
    }
    let path = home.join("config.json");
    let mut cur = read_json_value(&path).unwrap_or_else(|| json!({}));
    for (k, v) in obj {
        cur[k] = v.clone();
    }
    atomic_write(&path, &(serde_json::to_string(&cur)? + "\n"))?;
    http_json(stream, 200, &json!({"ok": true, "msg": "config saved"}))
}

pub(crate) fn valid_config(k: &str, v: &Value) -> bool {
    match k {
        "daily_cost_cap_usd" => v
            .as_i64()
            .is_some_and(|n| [0, 2, 4, 8, 16, 32, 64, 128, 256].contains(&n)),
        "grade_floor" => v
            .as_i64()
            .is_some_and(|n| [0, 50, 60, 70, 80, 90].contains(&n)),
        "plan_profile" | "autoplan_review_profile" => v.as_str().and_then(model_profile).is_some(),
        "build_profile" | "verify_profile" => v
            .as_str()
            .and_then(model_profile)
            .is_some_and(|profile| profile.is_codex().unwrap_or(false)),
        _ => false,
    }
}

fn prompt_path(root: &Path, id: &str) -> Option<PathBuf> {
    match id {
        "codex-loop" => Some(root.join("prompts/codex-loop.md")),
        "verify" => Some(root.join("prompts/verify.md")),
        _ => None,
    }
}

const PROMPT_IDS: [&str; 2] = ["codex-loop", "verify"];

fn versions_path(home: &Path, id: &str) -> PathBuf {
    home.join("versions").join(format!("{id}.jsonl"))
}

// Content-addressed prompt version id. seahash matches the rest of the codebase.
fn content_hash(text: &str) -> String {
    format!("{:016x}", seahash::hash(text.as_bytes()))[..12].to_string()
}

fn version_rows(home: &Path, id: &str) -> Vec<Value> {
    read_jsonl(&versions_path(home, id))
}

// Append a version row iff the content changed since the last row. Returns (hash, appended).
fn record_version(home: &Path, id: &str, text: &str) -> Result<(String, bool), AppError> {
    let hash = content_hash(text);
    let rows = version_rows(home, id);
    if rows
        .last()
        .and_then(|r| r.get("hash").and_then(Value::as_str))
        == Some(hash.as_str())
    {
        return Ok((hash, false));
    }
    let row = json!({"hash": hash, "ts": epoch(), "file": id, "content": text});
    append_line(&versions_path(home, id), &serde_json::to_string(&row)?)?;
    Ok((hash, true))
}

fn append_line(path: &Path, line: &str) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut f = OpenOptions::new().create(true).append(true).open(path)?;
    writeln!(f, "{line}")?;
    Ok(())
}

// Seed the version log from the live prompt file so history is never empty on first view.
fn prompts_snapshot(root: &Path, home: &Path) -> Value {
    let mut out = json!({});
    for id in PROMPT_IDS {
        let Some(path) = prompt_path(root, id) else {
            continue;
        };
        let text = read_text(&path).unwrap_or_default();
        let _ = record_version(home, id, &text);
        let versions: Vec<Value> = version_rows(home, id)
            .into_iter()
            .rev()
            .map(|r| json!({"hash": r.get("hash").cloned().unwrap_or(Value::Null), "ts": r.get("ts").cloned().unwrap_or(Value::Null)}))
            .collect();
        out[id] = json!({
            "id": id,
            "file": path.strip_prefix(root).unwrap_or(&path),
            "text": text,
            "current_hash": content_hash(&text),
            "versions": versions,
        });
    }
    out
}

const CHECKS_CACHE_SECS: u64 = 10 * 60;

// doctor + setup checks, cached — the login checks shell out, so don't re-run every 10s tick.
fn checks_snapshot(root: &Path, config: &Config) -> Value {
    let path = config.home.join("checks-cache.json");
    if let Some(cache) = read_json_value(&path) {
        if epoch().saturating_sub(
            cache
                .get("generated_at")
                .and_then(Value::as_u64)
                .unwrap_or(0),
        ) < CHECKS_CACHE_SECS
        {
            return cache;
        }
    }
    let snapshot = json!({
        "generated_at": epoch(),
        "doctor": crate::doctor_checks(root, config),
        "setup": crate::setup_checks(),
    });
    let _ = atomic_write(
        &path,
        &(serde_json::to_string(&snapshot).unwrap_or_default() + "\n"),
    );
    snapshot
}

fn post_prompt(
    root: &Path,
    home: &Path,
    body: Value,
    stream: &mut TcpStream,
) -> Result<(), AppError> {
    let Some(id) = body.get("file").and_then(Value::as_str) else {
        return http_json(stream, 400, &json!({"ok": false, "msg": "missing file"}));
    };
    let Some(path) = prompt_path(root, id) else {
        return http_json(stream, 400, &json!({"ok": false, "msg": "unknown prompt"}));
    };
    // Restore: body carries a version hash to roll the live file back to.
    if let Some(hash) = body.get("restore").and_then(Value::as_str) {
        let Some(content) = version_rows(home, id).into_iter().rev().find_map(|r| {
            (r.get("hash").and_then(Value::as_str) == Some(hash))
                .then(|| r.get("content").and_then(Value::as_str).map(str::to_string))
                .flatten()
        }) else {
            return http_json(
                stream,
                404,
                &json!({"ok": false, "msg": "version not found"}),
            );
        };
        atomic_write(&path, &content)?;
        let (hash, appended) = record_version(home, id, &content)?;
        return http_json(
            stream,
            200,
            &json!({"ok": true, "hash": hash, "msg": if appended { "restored — new version recorded" } else { "already the live version" }}),
        );
    }
    let Some(text) = body.get("text").and_then(Value::as_str) else {
        return http_json(stream, 400, &json!({"ok": false, "msg": "missing text"}));
    };
    atomic_write(&path, text)?;
    let (hash, appended) = record_version(home, id, text)?;
    http_json(
        stream,
        200,
        &json!({"ok": true, "hash": hash, "appended": appended, "msg": if appended { "saved — new version recorded" } else { "no change — already current" }}),
    )
}

// GET /version?file=<id>&hash=<h> — full text of a stored version, for the read-only viewer.
fn get_version(home: &Path, target: &str, stream: &mut TcpStream) -> Result<(), AppError> {
    let query = target.split('?').nth(1).unwrap_or("");
    let mut id = "";
    let mut hash = "";
    for pair in query.split('&') {
        match pair.split_once('=') {
            Some(("file", v)) => id = v,
            Some(("hash", v)) => hash = v,
            _ => {}
        }
    }
    if !PROMPT_IDS.contains(&id) {
        return http_json(stream, 400, &json!({"ok": false, "msg": "unknown prompt"}));
    }
    let content = version_rows(home, id).into_iter().rev().find_map(|r| {
        (r.get("hash").and_then(Value::as_str) == Some(hash))
            .then(|| r.get("content").and_then(Value::as_str).map(str::to_string))
            .flatten()
    });
    match content {
        Some(content) => http_json(stream, 200, &json!({"ok": true, "content": content})),
        None => http_json(
            stream,
            404,
            &json!({"ok": false, "msg": "version not found"}),
        ),
    }
}

fn post_cancel(home: &Path, body: Value, stream: &mut TcpStream) -> Result<(), AppError> {
    let Some(run) = body
        .get("run")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| active_run(home))
    else {
        return http_json(stream, 200, &json!({"ok": true, "cancelled": null}));
    };
    let run_dir = match find_run(home, &run) {
        Ok(run_dir) => run_dir,
        Err(err) => return http_json(stream, 404, &json!({"ok": false, "msg": err.to_string()})),
    };
    let _state = match lock_with_grace(&run_dir.join("run.json"), Duration::from_secs(2)) {
        Ok(state) => state,
        Err(error) => {
            return http_json(
                stream,
                409,
                &json!({"ok": false, "msg": error.to_string(), "cancelled": null}),
            )
        }
    };
    let light = build_run_light(&run_dir)?;
    if !light
        .get("cancelable")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return http_json(
            stream,
            409,
            &json!({"ok": false, "msg": "run already terminal", "cancelled": null}),
        );
    }
    let spec = read_run_spec(&run_dir)?;
    let (pid, child_pid) = match stop_run_processes(&run_dir, Duration::from_secs(2)) {
        Ok(pids) => pids,
        Err(error) => {
            return http_json(
                stream,
                409,
                &json!({"ok": false, "msg": error.to_string(), "cancelled": null}),
            )
        }
    };
    if run_dir.join("sentinel").is_file() || run_dir.join("result.json").is_file() {
        return http_json(
            stream,
            409,
            &json!({"ok": false, "msg": "run already terminal", "cancelled": null}),
        );
    }
    let result = terminal_result(
        RunStatus::Canceled,
        spec.id,
        "canceled by dashboard",
        &run_dir,
    );
    atomic_write(
        &run_dir.join("result.json"),
        &(serde_json::to_string_pretty(&result)? + "\n"),
    )?;
    atomic_write(&run_dir.join("sentinel"), "canceled\n")?;
    let _ = fs::remove_file(run_dir.join("run.pid"));
    let _ = fs::remove_file(run_dir.join(ACTIVE_CHILD_PID_FILE));
    http_json(
        stream,
        200,
        &json!({
            "ok": true,
            "cancelled": run,
            "pid_signaled": pid > 1,
            "child_pid_signaled": child_pid > 1
        }),
    )
}

fn http_json(stream: &mut TcpStream, code: u16, obj: &Value) -> Result<(), AppError> {
    http_bytes(
        stream,
        code,
        "application/json",
        serde_json::to_string(obj)?.as_bytes(),
    )
}
fn http_bytes(
    stream: &mut TcpStream,
    code: u16,
    content_type: &str,
    body: &[u8],
) -> Result<(), AppError> {
    write!(stream, "HTTP/1.1 {code} OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len())?;
    stream.write_all(body)?;
    Ok(())
}
fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(OsStr::to_str).unwrap_or("") {
        "html" => "text/html",
        "json" => "application/json",
        "css" => "text/css",
        "js" => "application/javascript",
        "png" => "image/png",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_run(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!(
            "milhouse-dashboard-test-{name}-{}-{}",
            std::process::id(),
            epoch()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prompt_path_is_whitelisted() {
        let root = Path::new("/repo");
        assert_eq!(
            prompt_path(root, "verify").unwrap(),
            root.join("prompts/verify.md")
        );
        assert!(prompt_path(root, "../Cargo.toml").is_none());
    }

    #[test]
    fn static_paths_cannot_escape_dashboard_root() {
        let root = Path::new("/tmp/dashboard");
        assert_eq!(
            safe_static_path(root, "assets/app.js"),
            Some(root.join("assets/app.js"))
        );
        assert!(safe_static_path(root, "../config.json").is_none());
        assert!(safe_static_path(root, "/etc/passwd").is_none());
    }

    #[test]
    fn agent_profiles_are_typed_by_phase() {
        assert!(valid_config(
            "plan_profile",
            &json!("opus-5-5-high-subscription")
        ));
        assert!(valid_config(
            "autoplan_review_profile",
            &json!("opus-5-5-high-subscription")
        ));
        assert!(valid_config(
            "build_profile",
            &json!("luna-6-high-subscription")
        ));
        assert!(valid_config(
            "verify_profile",
            &json!("sol-6.1-high-subscription")
        ));
        assert!(!valid_config("build_profile", &json!("fable-5-1-high-api")));
        assert!(!valid_config("verify_profile", &json!("unknown")));
    }

    #[test]
    fn dead_pid_run_finalizes_failed_stale() {
        let run = temp_run("dead-pid");
        let workdir = temp_run("workdir");
        let run = run.canonicalize().unwrap();
        fs::write(
            run.join("run.json"),
            serde_json::to_string(&json!({
                "id": "dead-pid",
                "run_dir": run,
                "workdir": workdir,
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(run.join("run.pid"), "999999\n").unwrap();
        finalize_dead_run(&run).unwrap();
        let result = read_json_value(&run.join("result.json")).unwrap();
        assert_eq!(result["status"], "failed_stale");
        assert_eq!(result["cost_status"], "unknown");
        assert_eq!(
            fs::read_to_string(run.join("sentinel")).unwrap(),
            "failed_stale\n"
        );
        assert!(!run.join("run.pid").exists());
    }

    #[test]
    fn dead_child_only_run_finalizes_failed_stale() {
        let run = temp_run("dead-child-pid").canonicalize().unwrap();
        fs::write(
            run.join("run.json"),
            serde_json::to_string(&json!({
                "id": "dead-child-pid",
                "run_dir": run,
                "workdir": temp_run("dead-child-workdir"),
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(run.join(ACTIVE_CHILD_PID_FILE), "999999\n").unwrap();

        finalize_dead_run(&run).unwrap();

        assert_eq!(
            read_json_value(&run.join("result.json")).unwrap()["status"],
            "failed_stale"
        );
        assert!(!run.join(ACTIVE_CHILD_PID_FILE).exists());
    }

    #[test]
    fn terminal_dead_pid_is_removed() {
        let run = temp_run("terminal-dead-pid");
        fs::write(run.join("run.json"), "{}\n").unwrap();
        fs::write(run.join("result.json"), r#"{"status":"canceled"}"#).unwrap();
        fs::write(run.join("run.pid"), "999999\n").unwrap();
        finalize_dead_run(&run).unwrap();
        assert!(!run.join("run.pid").exists());
    }

    #[test]
    fn dead_run_waits_while_terminal_state_is_locked() {
        let run = temp_run("locked-dead-pid");
        fs::write(run.join("run.pid"), "999999\n").unwrap();
        let state = lock(&run.join("run.json")).unwrap();

        finalize_dead_run(&run).unwrap();

        assert!(!run.join("result.json").exists());
        assert!(!run.join("sentinel").exists());
        drop(state);
    }

    #[test]
    fn running_run_uses_progress_cost() {
        use std::os::unix::process::CommandExt;

        let run = temp_run("progress-cost");
        let workdir = temp_run("progress-workdir");
        fs::write(
            run.join("run.json"),
            serde_json::to_string(&json!({
                "id": "progress-cost",
                "run_dir": run,
                "workdir": workdir,
                "prompt": "do thing"
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            run.join("progress.json"),
            r#"{"status":"running","issue":"progress-cost","claude_cost_usd":1.25,"oneshot_cost_usd":1.25,"cost_by_tier":{"fable":1.25,"claude":0,"codex":0},"cost_by_model":{"claude-fable-5":1.25},"claude_tokens":123}"#,
        )
        .unwrap();
        let mut runner = ProcessCommand::new("sleep");
        runner.arg("30").process_group(0);
        let mut runner = runner.spawn().unwrap();
        fs::write(run.join("run.pid"), format!("{}\n", runner.id())).unwrap();
        let run_lock = OpenOptions::new()
            .read(true)
            .write(true)
            .truncate(false)
            .open(run.join("run.pid"))
            .unwrap();
        run_lock.try_lock_exclusive().unwrap();
        fs::write(
            run.join("codex-session-1.jsonl"),
            r#"{"type":"item.started","item":{"command":"run long verification\nnext"}}"#,
        )
        .unwrap();
        let light = build_run_light(&run).unwrap();
        assert_eq!(light["status"], "running");
        assert_eq!(light["cost"], 1.25);
        assert_eq!(light["milhouse_cost"], 1.25);
        assert_eq!(light["cost_by_model"]["claude-fable-5"], 1.25);
        assert!(light["last_activity"].as_u64().unwrap() > 0);
        assert!(light["idle_seconds"].is_u64());
        assert_eq!(light["current_activity"], "run long verification");
        kill_process_group_force(runner.id() as i32);
        runner.wait().unwrap();
        drop(run_lock);
    }

    #[test]
    fn parses_claude_usage_text() {
        let parsed = parse_claude_usage(
            "Current session: 12% used · resets Jul 8 at 4:59pm (America/New_York)\n\
             Current week (all models): 34% used · resets Jul 15 at 11:59am (America/New_York)\n\
             Current week (Fable): 5% used\n",
        )
        .unwrap();
        assert_eq!(parsed["session_percent"], 12);
        assert_eq!(parsed["week_percent"], 34);
        assert_eq!(parsed["fable_week_percent"], 5);
        assert_eq!(
            parsed["session_reset"],
            "Jul 8 at 4:59pm (America/New_York)"
        );
    }

    #[test]
    fn maps_codex_rate_limits() {
        let parsed = codex_usage_from_rate_limits(&json!({
            "primary": {"used_percent": 20.0, "window_minutes": 300, "resets_at": 1783544104},
            "secondary": {"used_percent": 3.0, "window_minutes": 10080, "resets_at": 1784130904},
            "plan_type": "pro"
        }))
        .unwrap();
        assert_eq!(parsed["primary_percent"], 20.0);
        assert_eq!(parsed["primary_window_minutes"], 300);
        assert_eq!(parsed["secondary_percent"], 3.0);
        assert_eq!(parsed["plan_type"], "pro");
    }
}
