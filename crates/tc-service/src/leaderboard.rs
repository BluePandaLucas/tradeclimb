use super::*;
use std::collections::BTreeMap;

/// Compare only identical campaign, period, and execution settings. Read all
/// completed jobs rather than the bounded recent-run feed used by Overview.
pub(crate) fn groups(store: &Store) -> Result<Vec<Value>> {
    let db = store.db()?;
    let mut query = db.prepare(
        "SELECT data FROM jobs WHERE json_extract(data,'$.state') = 'completed' ORDER BY rowid DESC",
    )?;
    let jobs = query
        .query_map([], |r| r.get::<_, String>(0))?
        .map(|row| Ok(serde_json::from_str::<Job>(&row?)?))
        .collect::<Result<Vec<_>>>()?;
    Ok(rank(jobs))
}

fn rank(jobs: Vec<Job>) -> Vec<Value> {
    let mut groups: BTreeMap<String, (Value, BTreeMap<String, Job>)> = BTreeMap::new();
    for job in jobs
        .into_iter()
        .filter(|j| j.state == "completed" && !overlaps_final(j.config.start, j.config.end))
    {
        let context = json!({"campaign":job.campaign,"period":job.period,"config":job.config,"objective":job.objective});
        let (_, latest) = groups
            .entry(context.to_string())
            .or_insert_with(|| (context, BTreeMap::new()));
        let entry = latest.entry(job.strategy.clone()).or_insert(job.clone());
        // Input is newest row first; keep that order when timestamps tie.
        if job.created_at > entry.created_at {
            *entry = job;
        }
    }
    groups.into_iter().filter_map(|(id, (mut context, latest))| {
        let mut entries = latest.into_values().filter_map(|j| {
            let result = j.result.as_ref()?;
            let metric = j.objective.metric();
            let score = if j.period == Period::Validation {
                result[metric].as_f64()?
            } else {
                result["metrics"][metric].as_f64()?
            };
            score.is_finite().then(|| json!({"strategy":j.strategy,"job":j.id,"created_at":j.created_at,"score":score,metric:score,
                "eligibility":if j.period == Period::Validation {if j.objective == Objective::NetCagr {"CAGR only"} else {"Sharpe only"}} else if result["metrics"]["eligible"] == true {"Eligible"} else {"Review"}}))
        }).collect::<Vec<_>>();
        entries.sort_by(|a,b| b["score"].as_f64().unwrap_or_default().total_cmp(&a["score"].as_f64().unwrap_or_default()).then_with(|| a["strategy"].as_str().cmp(&b["strategy"].as_str())));
        let mut position = 0;
        let mut previous = None;
        for (i, entry) in entries.iter_mut().enumerate() {
            let score = entry["score"].as_f64();
            if score != previous { position = i + 1; }
            previous = score;
            entry["rank"] = json!(position);
        }
        if entries.is_empty() { return None; }
        context["id"] = json!(id);
        context["entries"] = json!(entries);
        Some(context)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn job(id: &str, strategy: &str, score: Option<f64>) -> Job {
        Job {
            objective: Objective::NetCagr,
            id: id.into(),
            strategy: strategy.into(),
            campaign: "campaign".into(),
            period: Period::Development,
            state: "completed".into(),
            created_at: 1,
            updated_at: 1,
            progress: 1.0,
            error: None,
            result: Some(json!({"metrics":{"net_cagr":score,"eligible":true}})),
            config: RunConfig::new(0, 24 * 60 * MINUTE),
        }
    }
    #[test]
    fn reserved_results_cannot_enter_any_leaderboard() {
        let mut reserved = job("reserved", "reserved", Some(99.0));
        reserved.config = RunConfig::new(FINAL_TEST_START, FINAL_TEST_END);
        assert!(rank(vec![reserved]).is_empty());
    }
    #[test]
    fn sharpe_ranking_uses_sharpe_and_never_falls_back_to_cagr() -> Result<()> {
        let mut first = job("a", "a", Some(9.0));
        first.objective = Objective::SharpeDailyV1;
        first.result = Some(json!({"metrics":{"sharpe":0.5,"net_cagr":9.0,"eligible":true}}));
        let mut second = first.clone();
        second.id = "b".into();
        second.strategy = "b".into();
        second.result = Some(json!({"metrics":{"sharpe":1.5,"net_cagr":0.1,"eligible":true}}));
        let mut missing = first.clone();
        missing.id = "missing".into();
        missing.strategy = "missing".into();
        missing.result = Some(json!({"metrics":{"net_cagr":99.0,"eligible":true}}));
        let mut validation = first.clone();
        validation.period = Period::Validation;
        validation.result =
            Some(json!({"sharpe":0.8,"metrics":{"net_cagr":99.0},"equity":"SECRET"}));
        let groups = rank(vec![
            first,
            second,
            missing,
            validation,
            job("old", "old", Some(0.2)),
        ]);
        assert_eq!(groups.len(), 3);
        let dev = groups
            .iter()
            .find(|g| g["objective"] == "sharpe_daily_v1" && g["period"] == "development")
            .context("sharpe group")?;
        assert_eq!(dev["entries"].as_array().context("entries")?.len(), 2);
        assert_eq!(dev["entries"][0]["strategy"], "b");
        let val = groups
            .iter()
            .find(|g| g["period"] == "validation")
            .context("validation")?;
        assert_eq!(val["entries"][0]["sharpe"], 0.8);
        assert_eq!(val["entries"][0]["eligibility"], "Sharpe only");
        assert!(!serde_json::to_string(val)?.contains("SECRET"));
        Ok(())
    }
    #[test]
    fn ranks_latest_completed_versions_with_ties_and_missing_scores() -> Result<()> {
        let mut latest = job("latest", "a", Some(-0.1));
        latest.created_at = 2;
        let mut pending = job("pending", "b", Some(2.0));
        pending.state = "running".into();
        let mut missing = job("missing", "c", None);
        missing.created_at = 2;
        let groups = rank(vec![
            job("old", "a", Some(0.9)),
            latest,
            job("b", "b", Some(0.2)),
            job("tie", "d", Some(0.2)),
            pending,
            missing,
            job("old-c", "c", Some(0.8)),
        ]);
        let rows = groups[0]["entries"].as_array().context("entries")?;
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["strategy"], "b");
        assert_eq!(rows[0]["rank"], 1);
        assert_eq!(rows[1]["rank"], 1);
        assert_eq!(rows[2]["rank"], 3);
        assert_eq!(rows[2]["job"], "latest");
        Ok(())
    }
    #[test]
    fn separates_campaigns_costs_and_validation_without_exposing_detail() -> Result<()> {
        let first = job("a", "a", Some(0.1));
        let mut cost = first.clone();
        cost.config.slippage_bps += 1;
        let mut campaign = first.clone();
        campaign.campaign = "other".into();
        let mut validation = first.clone();
        validation.period = Period::Validation;
        validation.result = Some(json!({"net_cagr":0.3,"metrics":{"net_cagr":99.0}}));
        let groups = rank(vec![first, cost, campaign, validation]);
        assert_eq!(groups.len(), 4);
        let validation = groups
            .iter()
            .find(|g| g["period"] == "validation")
            .context("validation group")?;
        assert_eq!(validation["entries"][0]["net_cagr"], 0.3);
        assert_eq!(validation["entries"][0]["eligibility"], "CAGR only");
        assert!(validation["entries"][0].get("metrics").is_none());
        Ok(())
    }
    #[test]
    fn includes_completed_runs_beyond_overview_limit() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::open(temp.path())?;
        for i in 0..510 {
            let job = job(&format!("run-{i}"), &format!("strategy-{i}"), Some(0.1));
            store.db()?.execute(
                "INSERT INTO jobs(id,key,fingerprint,data) VALUES(?1,?1,?1,?2)",
                params![job.id, serde_json::to_string(&job)?],
            )?;
        }
        assert_eq!(
            groups(&store)?[0]["entries"]
                .as_array()
                .context("entries")?
                .len(),
            510
        );
        Ok(())
    }
}
