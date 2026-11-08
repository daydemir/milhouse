use super::*;

const CACHE_MAX_AGE_SECS: u64 = 2 * 60 * 60;
const COST_SOURCE: &str = "anthropic_admin_api";

pub(crate) fn refresh(config: &Config) -> Result<Value, AppError> {
    let admin_key = env_key("ANTHROPIC_ADMIN_API_KEY")?;
    let local_key = env_key("ANTHROPIC_API_KEY")?;
    let api_key = resolve_api_key(&admin_key, &local_key)?;
    let raw = usage_report(&admin_key)?;
    let cache = build_cache(&raw, &api_key)?;
    atomic_write(
        &cache_path(&config.home),
        &(serde_json::to_string_pretty(&cache)? + "\n"),
    )?;
    Ok(cache)
}

pub(crate) fn status(config: &Config) -> Result<Value, AppError> {
    read_cache(&config.home)
}

pub(crate) fn gate(config: &Config) -> Result<Value, AppError> {
    let blocked = over_cap(config)?;
    let spent = if config.daily_cost_cap_usd > 0.0 {
        read_cache(&config.home)?
            .get("spent_today")
            .and_then(|value| value.get("total"))
            .and_then(Value::as_f64)
            .unwrap_or(0.0)
    } else {
        0.0
    };
    Ok(json!({
        "blocked": blocked,
        "cap_usd": config.daily_cost_cap_usd,
        "spent_today_usd": spent,
    }))
}

pub(crate) fn over_cap(config: &Config) -> Result<bool, AppError> {
    if config.daily_cost_cap_usd <= 0.0 {
        return Ok(false);
    }
    let cache = read_cache(&config.home)?;
    let spent = cache
        .get("spent_today")
        .ok_or_else(|| AppError::External("missing spend cache fields".into()))?;
    let refreshed = spent
        .get("refreshed_at")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    if is_stale(refreshed) {
        return Err(AppError::External(
            "spend cache stale; refusing new work while cost cap is enabled".into(),
        ));
    }
    Ok(spent.get("total").and_then(Value::as_f64).unwrap_or(0.0) >= config.daily_cost_cap_usd)
}

pub(crate) fn dashboard_values(config: &Config) -> (Value, Value) {
    match read_cache(&config.home) {
        Ok(cache) => (
            cache
                .get("spent_today")
                .cloned()
                .unwrap_or_else(|| error_today("missing spend cache fields")),
            cache
                .get("spend_history")
                .cloned()
                .unwrap_or_else(|| error_history("missing spend cache fields")),
        ),
        Err(err) => {
            let msg = err.to_string();
            (error_today(&msg), error_history(&msg))
        }
    }
}

fn build_cache(raw: &Value, api_key: &ApiKey) -> Result<Value, AppError> {
    let start = utc_midnight_epoch();
    let end = start + 86_400;
    let refreshed = epoch();
    let mut totals = Totals::default();
    let mut buckets = Vec::new();
    let mut latest = 0;

    for bucket in raw
        .get("data")
        .and_then(Value::as_array)
        .ok_or_else(|| AppError::External("usage report missing data".into()))?
    {
        let ts = iso_epoch(
            bucket
                .get("starting_at")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let bucket_end = iso_epoch(
            bucket
                .get("ending_at")
                .and_then(Value::as_str)
                .unwrap_or(""),
        );
        let mut bt = Totals::default();
        for row in bucket
            .get("results")
            .and_then(Value::as_array)
            .unwrap_or(&Vec::new())
        {
            if row.get("api_key_id").and_then(Value::as_str) != Some(api_key.id.as_str()) {
                continue;
            }
            let model = row.get("model").and_then(Value::as_str).unwrap_or("");
            let cost = usage_cost(row)?;
            bt.add(model, cost);
        }
        if bt.total > 0.0 || bucket_end > 0 {
            latest = latest.max(bucket_end);
            totals.merge(&bt);
            buckets.push(json!({
                "ts": ts,
                "end": bucket_end,
                "total": round4(bt.total),
                "fable": round4(bt.fable),
                "claude": round4(bt.claude),
                "codex": 0.0,
                "models": bt.models,
            }));
        }
    }

    let meta = json!({
        "source": COST_SOURCE,
        "estimated": true,
        "tz": "UTC",
        "api_key_id": api_key.id,
        "api_key_name": api_key.name,
        "window_start": start,
        "window_end": end,
        "latest_through": if latest == 0 { Value::Null } else { json!(latest) },
        "refreshed_at": refreshed,
        "stale_after": refreshed + CACHE_MAX_AGE_SECS,
        "stale": false,
        "error": Value::Null,
    });
    let mut spent = meta.clone();
    spent["total"] = json!(round4(totals.total));
    spent["fable"] = json!(round4(totals.fable));
    spent["claude"] = json!(round4(totals.claude));
    spent["codex"] = json!(0.0);
    spent["models"] = json!(totals.models);

    let mut history = meta;
    history["start"] = json!(start);
    history["end"] = json!(end);
    history["total"] = json!(round4(totals.total));
    history["fable"] = json!(round4(totals.fable));
    history["claude"] = json!(round4(totals.claude));
    history["codex"] = json!(0.0);
    history["buckets"] = json!(buckets);

    Ok(json!({"spent_today": spent, "spend_history": history}))
}

fn read_cache(home: &Path) -> Result<Value, AppError> {
    let mut cache = read_json_value(&cache_path(home))
        .ok_or_else(|| AppError::External("spend cache missing; run `mil spend refresh`".into()))?;
    let stale = cache
        .get("spent_today")
        .and_then(|v| v.get("refreshed_at"))
        .and_then(Value::as_u64)
        .map(is_stale)
        .unwrap_or(true);
    for key in ["spent_today", "spend_history"] {
        if let Some(obj) = cache.get_mut(key).and_then(Value::as_object_mut) {
            obj.insert("stale".into(), json!(stale));
        }
    }
    Ok(cache)
}

fn usage_report(admin_key: &str) -> Result<Value, AppError> {
    let start = utc_midnight_epoch();
    curl_json(
        "https://api.anthropic.com/v1/organizations/usage_report/messages",
        &[
            ("starting_at", iso_from_epoch(start)),
            ("ending_at", iso_from_epoch(start + 86_400)),
            ("bucket_width", "1h".into()),
            ("group_by[]", "api_key_id".into()),
            ("group_by[]", "model".into()),
            ("group_by[]", "context_window".into()),
        ],
        admin_key,
    )
}

fn resolve_api_key(admin_key: &str, local_key: &str) -> Result<ApiKey, AppError> {
    let keys = curl_json(
        "https://api.anthropic.com/v1/organizations/api_keys",
        &[],
        admin_key,
    )?;
    let mut matches = keys
        .get("data")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .iter()
        .filter(|k| k.get("status").and_then(Value::as_str) == Some("active"))
        .filter(|k| {
            k.get("partial_key_hint")
                .and_then(Value::as_str)
                .is_some_and(|hint| hint_matches(local_key, hint))
        })
        .map(|k| ApiKey {
            id: k
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            name: k
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        })
        .collect::<Vec<_>>();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(AppError::External(
            "no active Anthropic API key matches local ANTHROPIC_API_KEY".into(),
        )),
        _ => Err(AppError::External(
            "multiple active Anthropic API keys match local ANTHROPIC_API_KEY".into(),
        )),
    }
}

fn curl_json(base: &str, params: &[(&str, String)], admin_key: &str) -> Result<Value, AppError> {
    let mut last = None;
    for _ in 0..3 {
        match curl_json_once(base, params, admin_key) {
            Ok(v) => return Ok(v),
            Err(err) => {
                last = Some(err);
                thread::sleep(Duration::from_secs(2));
            }
        }
    }
    Err(last.unwrap_or_else(|| AppError::External("Anthropic API request failed".into())))
}

fn curl_json_once(
    base: &str,
    params: &[(&str, String)],
    admin_key: &str,
) -> Result<Value, AppError> {
    let mut child = ProcessCommand::new("curl")
        .arg("-sS")
        .arg("-G")
        .arg("--config")
        .arg("-")
        .arg(base)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| AppError::External("missing curl stdin".into()))?;
        writeln!(
            stdin,
            "header = {}",
            curl_quote("anthropic-version: 2023-06-01")
        )?;
        writeln!(
            stdin,
            "header = {}",
            curl_quote(&format!("x-api-key: {admin_key}"))
        )?;
        for (k, v) in params {
            writeln!(
                stdin,
                "data-urlencode = {}",
                curl_quote(&format!("{k}={v}"))
            )?;
        }
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        return Err(AppError::External(format!(
            "curl failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    let v: Value = serde_json::from_slice(&out.stdout)?;
    if v.get("type").and_then(Value::as_str) == Some("error") {
        return Err(AppError::External(
            v.get("error")
                .and_then(|e| e.get("message"))
                .and_then(Value::as_str)
                .unwrap_or("Anthropic API error")
                .to_string(),
        ));
    }
    Ok(v)
}

fn usage_cost(row: &Value) -> Result<f64, AppError> {
    let model = row.get("model").and_then(Value::as_str).unwrap_or("");
    let (input, output) = model_rates(model)?;
    let context_multiplier = match row.get("context_window").and_then(Value::as_str) {
        Some("200k-1M") => 1.5,
        _ => 1.0,
    };
    let uncached = row
        .get("uncached_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as f64;
    let cache_5m = row
        .pointer("/cache_creation/ephemeral_5m_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as f64;
    let cache_1h = row
        .pointer("/cache_creation/ephemeral_1h_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as f64;
    let cache_read = row
        .get("cache_read_input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as f64;
    let output_tokens = row
        .get("output_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0) as f64;
    Ok(context_multiplier
        * (uncached * input
            + cache_5m * input * 1.25
            + cache_1h * input * 2.0
            + cache_read * input * 0.1
            + output_tokens * output)
        / 1_000_000.0)
}

fn model_rates(model: &str) -> Result<(f64, f64), AppError> {
    if model == "claude-fable-5" {
        Ok((10.0, 50.0))
    } else if model == "claude-opus-5" || model.starts_with("claude-opus-4-8") {
        Ok((5.0, 25.0))
    } else if model.starts_with("claude-sonnet-5") {
        Ok((2.0, 10.0))
    } else if model.starts_with("claude-sonnet-4-6") {
        Ok((3.0, 15.0))
    } else if model.starts_with("claude-haiku-4-5") {
        Ok((1.0, 5.0))
    } else {
        Err(AppError::External(format!(
            "unknown Anthropic model rate: {model}"
        )))
    }
}

fn tier(model: &str) -> &'static str {
    if model == "claude-fable-5" {
        "fable"
    } else {
        "claude"
    }
}

fn env_key(name: &str) -> Result<String, AppError> {
    env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(Ok)
        .unwrap_or_else(|| engine::read_env_key(&engine::anthropic_key_file()?, name))
}

fn cache_path(home: &Path) -> PathBuf {
    home.join("api-spend-cache.json")
}

fn hint_matches(key: &str, hint: &str) -> bool {
    if let Some((left, right)) = hint.split_once("...") {
        key.starts_with(left) && key.ends_with(right)
    } else {
        key == hint
    }
}

fn utc_midnight_epoch() -> u64 {
    chrono::Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_utc()
        .timestamp() as u64
}

fn iso_from_epoch(ts: u64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(ts as i64, 0)
        .unwrap()
        .format("%Y-%m-%dT%H:%M:%SZ")
        .to_string()
}

fn is_stale(refreshed_at: u64) -> bool {
    refreshed_at == 0 || epoch().saturating_sub(refreshed_at) > CACHE_MAX_AGE_SECS
}

fn curl_quote(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn error_today(msg: &str) -> Value {
    let start = utc_midnight_epoch();
    json!({"source":COST_SOURCE,"estimated":true,"tz":"UTC","total":0.0,"fable":0.0,"claude":0.0,"codex":0.0,"window_start":start,"window_end":start+86400,"latest_through":Value::Null,"refreshed_at":Value::Null,"stale":true,"error":msg})
}

fn error_history(msg: &str) -> Value {
    let start = utc_midnight_epoch();
    json!({"source":COST_SOURCE,"estimated":true,"tz":"UTC","start":start,"end":start+86400,"total":0.0,"fable":0.0,"claude":0.0,"codex":0.0,"buckets":[],"latest_through":Value::Null,"refreshed_at":Value::Null,"stale":true,"error":msg})
}

#[derive(Clone)]
struct ApiKey {
    id: String,
    name: String,
}

#[derive(Default)]
struct Totals {
    total: f64,
    fable: f64,
    claude: f64,
    models: BTreeMap<String, f64>,
}

impl Totals {
    fn add(&mut self, model: &str, cost: f64) {
        self.total += cost;
        match tier(model) {
            "fable" => self.fable += cost,
            _ => self.claude += cost,
        }
        *self.models.entry(model.to_string()).or_insert(0.0) += round4(cost);
    }

    fn merge(&mut self, other: &Totals) {
        self.total += other.total;
        self.fable += other.fable;
        self.claude += other.claude;
        for (model, cost) in &other.models {
            *self.models.entry(model.clone()).or_insert(0.0) += cost;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_hint_matches_local_key() {
        assert!(hint_matches(
            "sk-ant-api03-z2NFNpy2-OAAA",
            "sk-ant-api03-z2N...OAAA"
        ));
        assert!(!hint_matches(
            "sk-ant-api03-z2NFNpy2-OAAB",
            "sk-ant-api03-z2N...OAAA"
        ));
    }

    #[test]
    fn costs_usage_rows() {
        let row = json!({"model":"claude-fable-5","context_window":"0-200k","uncached_input_tokens":1000000,"cache_creation":{"ephemeral_5m_input_tokens":1000000,"ephemeral_1h_input_tokens":1000000},"cache_read_input_tokens":1000000,"output_tokens":1000000});
        assert_eq!(round4(usage_cost(&row).unwrap()), 93.5);
    }

    #[test]
    fn prices_opus_5_usage() {
        let row = json!({"model":"claude-opus-5","context_window":"0-200k","uncached_input_tokens":1000000,"cache_creation":{},"cache_read_input_tokens":0,"output_tokens":1000000});
        assert_eq!(round4(usage_cost(&row).unwrap()), 30.0);
    }

    #[test]
    fn long_context_costs_more() {
        let row = json!({"model":"claude-sonnet-5","context_window":"200k-1M","uncached_input_tokens":1000000,"cache_creation":{},"cache_read_input_tokens":0,"output_tokens":0});
        assert_eq!(round4(usage_cost(&row).unwrap()), 3.0);
    }

    #[test]
    fn cap_disabled_ignores_missing_cache() {
        let config = Config {
            home: env::temp_dir().join(format!("milhouse-spend-test-{}", epoch())),
            daily_cost_cap_usd: 0.0,
            ..Config::default()
        };
        assert!(!over_cap(&config).unwrap());
    }

    #[test]
    fn cap_enabled_fails_closed_without_cache() {
        let config = Config {
            home: env::temp_dir().join(format!("milhouse-spend-test-{}", epoch())),
            daily_cost_cap_usd: 1.0,
            ..Config::default()
        };
        assert!(over_cap(&config).is_err());
    }

    #[test]
    fn cap_uses_cached_api_total() {
        let config = Config {
            home: env::temp_dir().join(format!("milhouse-spend-test-{}", epoch())),
            daily_cost_cap_usd: 5.0,
            ..Config::default()
        };
        fs::create_dir_all(&config.home).unwrap();
        fs::write(
            cache_path(&config.home),
            serde_json::to_string(&json!({
                "spent_today": {"total": 5.25, "refreshed_at": epoch()},
                "spend_history": {"buckets": []}
            }))
            .unwrap(),
        )
        .unwrap();
        assert!(over_cap(&config).unwrap());
    }
}
