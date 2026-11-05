use super::*;
fn qualification(job: &Job, campaign: &Campaign, manifest: &tc_data::Manifest) -> Result<Value> {
    ensure!(
        job.state == "completed"
            && job.period == Period::Development
            && job.objective == Objective::SharpeDailyV1,
        "completed Sharpe development evaluation required"
    );
    ensure_research_window(job.config.start, job.config.end)?;
    ensure!(
        !manifest.synthetic && manifest.accepted && manifest.venue == "bitvavo",
        "paper promotion requires accepted, non-synthetic Bitvavo research data"
    );
    let metrics = &job.result.as_ref().context("run has no results")?["metrics"];
    let sharpe = metrics["sharpe"]
        .as_f64()
        .filter(|v| v.is_finite())
        .context("defined Sharpe required")?;
    let drawdown = metrics["max_drawdown"]
        .as_f64()
        .context("drawdown required")?;
    let cagr = metrics["net_cagr"]
        .as_f64()
        .filter(|v| v.is_finite())
        .context("CAGR required")?;
    ensure!(
        metrics["eligible"] == true && drawdown.is_finite() && (0.0..=0.15).contains(&drawdown),
        "development risk eligibility and maximum 15% drawdown required"
    );
    Ok(
        json!({"strategy":job.strategy,"job":job.id,"campaign":job.campaign,"sharpe":sharpe,"max_drawdown":drawdown,"net_cagr":cagr,"target_cagr":campaign.target_cagr,"growth_target_met":cagr>=campaign.target_cagr,"basis":"development"}),
    )
}
impl Store {
    pub(crate) fn paper_candidates(&self) -> Result<Vec<Value>> {
        let db = self.db()?;
        let mut query=db.prepare("SELECT data FROM jobs WHERE json_extract(data,'$.state')='completed' AND json_extract(data,'$.period')='development' ORDER BY rowid DESC")?;
        let campaigns: std::collections::BTreeMap<_, _> = self
            .objects::<Campaign>("campaign")?
            .into_iter()
            .map(|c| (c.id.clone(), c))
            .collect();
        let manifests: std::collections::BTreeMap<_, _> = tc_data::list(&self.root)?
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let mut seen = std::collections::BTreeSet::new();
        let mut candidates = vec![];
        for row in query.query_map([], |r| r.get::<_, String>(0))? {
            let job: Job = serde_json::from_str(&row?)?;
            if !seen.insert(job.strategy.clone()) {
                continue;
            }
            let Some(campaign) = campaigns.get(&job.campaign) else {
                continue;
            };
            let Some(manifest) = manifests.get(&campaign.dataset) else {
                continue;
            };
            if let Ok(candidate) = qualification(&job, campaign, manifest) {
                candidates.push(candidate);
            }
        }
        candidates.sort_by(|a, b| {
            b["sharpe"]
                .as_f64()
                .unwrap_or_default()
                .total_cmp(&a["sharpe"].as_f64().unwrap_or_default())
        });
        Ok(candidates)
    }
    /// Only the authenticated owner HTTP action calls this; agents cannot reach it.
    pub fn approve_paper(&self, strategy: &str, id: &str, approved: bool) -> Result<PaperSession> {
        ensure!(approved, "explicit owner approval required");
        let job = self.job(id)?;
        ensure!(
            job.strategy == strategy,
            "evaluation belongs to a different strategy version"
        );
        let campaign: Campaign = self.get("campaign", &job.campaign)?;
        let manifest = tc_data::list(&self.root)?
            .into_iter()
            .find(|m| m.id == campaign.dataset)
            .context("unknown dataset")?;
        let record = qualification(&job, &campaign, &manifest)?;
        ensure!(
            self.paper_candidates()?.iter().any(|c| c["job"] == id),
            "choose the latest qualifying development evaluation"
        );
        self.start_paper_recorded(strategy, Some(record))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn paper_needs_owner_approval_and_real_eligible_sharpe_but_growth_is_aspirational() {
        let temp = tempfile::tempdir().unwrap();
        let store = Store::open(temp.path()).unwrap();
        // Explicitly mocked native provenance/results; never market evidence.
        let manifest = tc_data::snapshot(
            &store.root,
            vec![],
            timestamp("2024-01-01").unwrap(),
            timestamp("2024-04-01").unwrap(),
            false,
            true,
        )
        .unwrap();
        let campaign = store.campaign(&manifest.id).unwrap();
        let strategy = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "candidate".into(),
                    description: "fixture".into(),
                    kind: StrategyKind::Cash,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        let mut job = store
            .submit(Submission {
                timing: None,
                trace: None,
                strategy: strategy.id.clone(),
                campaign: campaign.id.clone(),
                period: Period::Development,
                idempotency_key: "candidate".into(),
                slippage_bps: None,
            })
            .unwrap();
        job.state = "completed".into();
        job.result = Some(
            json!({"metrics":{"eligible":true,"sharpe":1.2,"net_cagr":0.10,"max_drawdown":0.10}}),
        );
        store.update_job(&job).unwrap();
        assert!(store.approve_paper(&strategy.id, &job.id, false).is_err());
        let session = store.approve_paper(&strategy.id, &job.id, true).unwrap();
        let record: Value = store.get("paper_approval", &session.id).unwrap();
        assert_eq!(record["growth_target_met"], false);
        assert_eq!(record["job"], job.id);
        store.stop_paper(&session.id).unwrap();
        job.result.as_mut().unwrap()["metrics"]["max_drawdown"] = json!(0.16);
        store.update_job(&job).unwrap();
        assert!(store.approve_paper(&strategy.id, &job.id, true).is_err());
        job.result.as_mut().unwrap()["metrics"]["max_drawdown"] = json!(0.10);
        job.result.as_mut().unwrap()["metrics"]["sharpe"] = Value::Null;
        store.update_job(&job).unwrap();
        assert!(store.approve_paper(&strategy.id, &job.id, true).is_err());
    }
}
