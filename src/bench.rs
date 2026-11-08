use super::*;
use crate::{engine, grade};
pub(crate) fn bench(root: &Path, config: &Config, args: BenchArgs) -> Result<(), AppError> {
    let cleanup = args.cleanup;
    match args.command {
        Some(BenchCommand::Calibrate) => calibrate(root),
        Some(BenchCommand::Ref { task, bank }) => bench_ref(root, &bank, &task),
        Some(BenchCommand::Gold { task, bank, runs }) => {
            for _ in 0..runs {
                bench_gold(root, &bank, &task, cleanup)?;
            }
            Ok(())
        }
        None => run_benchmark(root, config, args.bank, &args.task, &args.variant, cleanup),
    }
}

pub(crate) fn run_benchmark(
    root: &Path,
    config: &Config,
    bank: String,
    task: &str,
    variant: &str,
    cleanup: bool,
) -> Result<(), AppError> {
    let dirs = bench_tasks(root, &bank, task)?;
    for dir in dirs {
        bench_one(root, config, &bank, &dir, variant, cleanup)?;
    }
    Ok(())
}

fn bench_one(
    root: &Path,
    config: &Config,
    bank: &str,
    dir: &Path,
    variant: &str,
    cleanup: bool,
) -> Result<(), AppError> {
    let card = read_json_value(&dir.join("card.json"))
        .ok_or_else(|| AppError::Input(format!("missing card.json: {}", dir.display())))?;
    let task = dir
        .file_name()
        .and_then(OsStr::to_str)
        .unwrap_or("task")
        .to_string();
    let work = PathBuf::from(env::var("BENCH_WORK").unwrap_or_else(|_| "/tmp/mil-bench".into()))
        .join(format!("{}-{}", task, compact_timestamp_local()));
    fs::create_dir_all(&work)?;
    let _cleanup = CleanupDir::new(work.clone(), cleanup);
    run_setup(&dir.join("setup.sh"), &work)?;
    let mut bench_config = config.clone();
    bench_config.home = work.join(".oshome");
    bench_config.lock_path = work.join(".lock");
    let goal = card
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("");
    let prompt = format!(
        "# Goal\n\n{goal}\n\n## Acceptance criteria\n\n- Fulfill the goal completely without unrelated changes.\n- Preserve existing behavior unless the goal explicitly changes it.\n- Record durable evidence for every completion claim.\n- Run the repository's relevant tests and checks successfully."
    );
    let task_input = TaskInput {
        id: task.clone(),
        prompt,
        metadata: json!({"bench": bank}),
    };
    let run = engine::run_task(root, &work, &bench_config, task_input)?;
    if run.status != RunStatus::Green.as_str() {
        return Err(AppError::External(format!(
            "bench task {} ended {}: {}",
            task,
            run.status,
            run.result_path.display()
        )));
    }
    let out = work.join(".bench-score.json");
    let grade = grade::grade_artifact(&work, &dir.join("rubric.md"), Some(&out))?;
    let run_cost = run.cost.unwrap_or(0.0);
    let grade_cost = grade.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
    let run_tiers = read_json_value(&run.result_path)
        .and_then(|v| v.get("cost_by_tier").cloned())
        .unwrap_or_else(|| json!({"fable":0.0,"claude":0.0,"codex":0.0}));
    let run_models = read_json_value(&run.result_path)
        .and_then(|v| v.get("cost_by_model").cloned())
        .unwrap_or_else(|| json!({}));
    let builder = config.build_profile.details()?;
    let row = json!({"ts": iso_now(), "task": task, "variant": variant, "score": grade["score"], "cost": round4(run_cost + grade_cost), "milhouse_cost": round4(run_cost), "grade_cost": round4(grade_cost), "cost_by_tier": merge_cost_by_tier(&run_tiers, grade.get("cost_by_tier").unwrap_or(&Value::Null)), "cost_by_model": merge_cost_by_model(&run_models, grade.get("cost_by_model").unwrap_or(&Value::Null)), "models": {"evaluation_rubric": Model::Opus.id()?, "executor": builder.model, "scorer": Model::Opus.id()?, "worker": builder.model}, "bench": bank, "grader": grade["grader"], "holdout": card.get("holdout").and_then(Value::as_bool).unwrap_or(false), "work": work});
    append_jsonl(&root.join("parity/scores.jsonl"), &row)?;
    Ok(())
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

fn merge_cost_by_tier(a: &Value, b: &Value) -> Value {
    let mut out = json!({});
    for tier in ["fable", "claude", "codex"] {
        out[tier] = json!(round4(
            a.get(tier).and_then(Value::as_f64).unwrap_or(0.0)
                + b.get(tier).and_then(Value::as_f64).unwrap_or(0.0)
        ));
    }
    out
}

fn merge_cost_by_model(a: &Value, b: &Value) -> Value {
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

fn run_setup(setup: &Path, work: &Path) -> Result<(), AppError> {
    if !setup.exists() {
        return Err(AppError::Input(format!(
            "missing setup: {}",
            setup.display()
        )));
    }
    let status = ProcessCommand::new("bash").arg(setup).arg(work).status()?;
    if status.success() {
        Ok(())
    } else {
        Err(AppError::External(format!(
            "setup failed: {}",
            setup.display()
        )))
    }
}

fn bench_ref(root: &Path, bank: &str, task: &str) -> Result<(), AppError> {
    for dir in bench_tasks(root, bank, task)? {
        let ref_dir = dir.join("reference");
        let grade = grade::grade_artifact(&ref_dir, &dir.join("rubric.md"), None)?;
        append_jsonl(
            &root.join("parity/scores.jsonl"),
            &json!({"ts": iso_now(), "task": dir.file_name().and_then(OsStr::to_str).unwrap_or(""), "variant": "fable-reference", "score": grade["score"], "models": {"author": Model::Fable.id()?, "scorer": Model::Fable.id()?, "worker": Model::Fable.id()?}, "bench": bank, "grader": grade["grader"], "holdout": read_json_value(&dir.join("card.json")).and_then(|c| c.get("holdout").cloned()).and_then(|v| v.as_bool()).unwrap_or(false), "work": ref_dir}),
        )?;
    }
    Ok(())
}

fn bench_gold(root: &Path, bank: &str, task: &str, cleanup: bool) -> Result<(), AppError> {
    for dir in bench_tasks(root, bank, task)? {
        let card = read_json_value(&dir.join("card.json")).unwrap_or_else(|| json!({}));
        let work = PathBuf::from(
            env::var("BENCH_WORK").unwrap_or_else(|_| "/tmp/mil-bench".into()),
        )
        .join(format!(
            "gold-{}-{}",
            dir.file_name().and_then(OsStr::to_str).unwrap_or("task"),
            compact_timestamp_local()
        ));
        fs::create_dir_all(&work)?;
        let _cleanup = CleanupDir::new(work.clone(), cleanup);
        run_setup(&dir.join("setup.sh"), &work)?;
        let prompt = format!(
            "Work end to end in the git sandbox at {}. The task:\n\n{}\n\nNever push.",
            work.display(),
            card.get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
        );
        let raw = grade::claude_print(&work, Model::Fable, &prompt, true).unwrap_or_default();
        fs::write(work.join(".gold-out.json"), &raw)?;
        let cost = serde_json::from_str::<Value>(&raw)
            .ok()
            .and_then(|v| v.get("total_cost_usd").and_then(Value::as_f64));
        let grade = grade::grade_artifact(
            &work,
            &dir.join("rubric.md"),
            Some(&work.join(".bench-score.json")),
        )?;
        append_jsonl(
            &root.join("parity/scores.jsonl"),
            &json!({"ts": iso_now(), "task": dir.file_name().and_then(OsStr::to_str).unwrap_or(""), "variant": "fable-gold", "score": grade["score"], "cost": cost, "cost_by_tier": {"fable": cost, "claude": 0, "codex": 0}, "models": {"author": Model::Fable.id()?, "scorer": Model::Fable.id()?, "worker": Model::Fable.id()?}, "bench": bank, "grader": grade["grader"], "holdout": card.get("holdout").and_then(Value::as_bool).unwrap_or(false), "work": work}),
        )?;
    }
    Ok(())
}

fn calibrate(root: &Path) -> Result<(), AppError> {
    for (name, want) in [("known-67", 67.0), ("known-86", 86.0)] {
        let dir = root.join("bench/calibration").join(name);
        let grade = grade::grade_artifact(&dir, &root.join("bench/calibration/rubric.md"), None)?;
        let score = grade.get("score").and_then(Value::as_f64).unwrap_or(0.0);
        if (score - want).abs() > 5.0 {
            return Err(AppError::External(format!(
                "CALIBRATE {name}: scored {score} want {want} ±5"
            )));
        }
        println!("CALIBRATE {name}: scored {score} (want {want} ±5) OK");
    }
    Ok(())
}

fn bench_tasks(root: &Path, bank: &str, task: &str) -> Result<Vec<PathBuf>, AppError> {
    let dir = root.join("bench").join(bank);
    if !dir.is_dir() {
        return Err(AppError::Input(format!("bench bank missing: {bank}")));
    }
    let specs = task
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    if specs.is_empty() {
        return Err(AppError::Input("empty bench task".into()));
    }
    let mut out = Vec::new();
    for e in fs::read_dir(dir)?.filter_map(Result::ok) {
        let p = e.path();
        if !p.join("card.json").exists() {
            continue;
        }
        let base = p.file_name().and_then(OsStr::to_str).unwrap_or("");
        let ident = read_json_value(&p.join("card.json"))
            .and_then(|v| {
                v.get("identifier")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .unwrap_or_default();
        if specs
            .iter()
            .any(|spec| bench_task_matches(spec, base, &ident))
        {
            out.push(p);
        }
    }
    if out.is_empty() {
        return Err(AppError::Input(format!("unknown task: {task}")));
    }
    out.sort();
    Ok(out)
}

fn bench_task_matches(spec: &str, base: &str, ident: &str) -> bool {
    spec == "all"
        || spec == base
        || base
            .strip_suffix(base.split_once('-').map(|x| x.1).unwrap_or(""))
            .is_some_and(|prefix| prefix.trim_end_matches('-') == spec)
        || ident == spec
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_short_full_and_identifier() {
        assert!(bench_task_matches("r4", "r4-vague-ops", "TASK-R4"));
        assert!(bench_task_matches(
            "r4-vague-ops",
            "r4-vague-ops",
            "TASK-R4"
        ));
        assert!(bench_task_matches("TASK-R4", "r4-vague-ops", "TASK-R4"));
        assert!(!bench_task_matches("r6", "r4-vague-ops", "TASK-R4"));
    }
}
