use super::*;
use tc_sdk::{ModelRequest, ModelResponse};
#[derive(Serialize, Deserialize)]
pub struct InferenceRequest {
    pub scope: String,
    pub request: ModelRequest,
}
/// Server configured provider; callers cannot select arbitrary URLs or credentials.
pub async fn infer(store: &Store, input: InferenceRequest) -> Result<ModelResponse> {
    ensure!(
        input.request.max_tokens > 0
            && input.request.max_tokens <= 2048
            && input.request.prompt.len() <= 32_768,
        "inference request exceeds limits"
    );
    ensure!(
        store.job(&input.scope).is_ok() || store.get::<PaperSession>("paper", &input.scope).is_ok(),
        "unknown inference scope"
    );
    let key = digest(&serde_json::to_vec(&input.request)?);
    {
        let mut db = store.db()?;
        let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let cached: Option<String> = tx
            .query_row(
                "SELECT response FROM model_cache WHERE scope=?1 AND key=?2",
                params![input.scope, key],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(cached) = cached {
            ensure!(
                cached != "pending",
                "inference pending or previously failed; inspect locally"
            );
            return Ok(serde_json::from_str(&cached)?);
        }
        let calls: i64 = tx.query_row(
            "SELECT COUNT(*) FROM model_cache WHERE scope=?1",
            [&input.scope],
            |r| r.get(0),
        )?;
        ensure!(calls < 64, "model call budget exhausted");
        tx.execute(
            "INSERT INTO model_cache VALUES(?1,?2,'pending')",
            params![input.scope, key],
        )?;
        tx.commit()?;
    }
    let mapping: std::collections::BTreeMap<String, String> =
        serde_json::from_str(&std::env::var("TC_MODEL_MAP").unwrap_or_else(|_| "{}".into()))?;
    let response = if let Some(model) = mapping.get(&input.request.capability) {
        let base =
            std::env::var("TC_OLLAMA_URL").unwrap_or_else(|_| "http://127.0.0.1:11434".into());
        let result:Value=reqwest::Client::builder().timeout(Duration::from_secs(30)).build()?.post(format!("{base}/api/generate")).json(&json!({"model":model,"prompt":input.request.prompt,"stream":false,"options":{"num_predict":input.request.max_tokens}})).send().await?.error_for_status()?.json().await?;
        ModelResponse {
            text: result["response"]
                .as_str()
                .context("invalid model response")?
                .into(),
            provider: "ollama".into(),
            model: model.clone(),
            training_cutoff: None,
        }
    } else {
        ensure!(
            input.request.capability == "fake",
            "model capability not configured"
        );
        ModelResponse {
            text: input.request.prompt,
            provider: "fake".into(),
            model: "echo".into(),
            training_cutoff: None,
        }
    };
    store.db()?.execute(
        "UPDATE model_cache SET response=?1 WHERE scope=?2 AND key=?3",
        params![serde_json::to_string(&response)?, input.scope, key],
    )?;
    Ok(response)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn responses_replay_within_scope_and_never_cross_sessions() {
        let root = tempfile::tempdir().unwrap();
        let store = Store::open(root.path()).unwrap();
        let s = store
            .register(
                StrategySpec {
                    parameters: Default::default(),
                    name: "Cash".into(),
                    description: "Model fixture".into(),
                    kind: StrategyKind::Cash,
                    markets: vec!["BTC-EUR".into()],
                },
                None,
                None,
            )
            .unwrap();
        let a = store.start_paper(&s.id).unwrap();
        store.stop_paper(&a.id).unwrap();
        let b = store.start_paper(&s.id).unwrap();
        for scope in [&a.id, &a.id, &b.id] {
            let response = infer(
                &store,
                InferenceRequest {
                    scope: scope.clone(),
                    request: tc_sdk::ModelRequest {
                        capability: "fake".into(),
                        prompt: "same prompt".into(),
                        max_tokens: 10,
                    },
                },
            )
            .await
            .unwrap();
            assert_eq!(response.text, "same prompt");
        }
        let count: i64 = store
            .db()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM model_cache", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }
}
