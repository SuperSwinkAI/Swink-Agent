use std::collections::{HashMap, HashSet};

use futures::future::join_all;
use reqwest::Client;
use serde_json::Value;

use crate::catalog::{ProviderEndpoint, VerifyTask};

pub enum PresetResult {
    Pass,
    Fail { available_count: usize },
    Skipped { reason: &'static str },
    NetworkError { error: String },
}

pub struct VerifyRow {
    pub task: VerifyTask,
    pub result: PresetResult,
}

pub async fn verify_all(tasks: Vec<VerifyTask>) -> Vec<VerifyRow> {
    let mut groups: HashMap<String, (ProviderEndpoint, Vec<VerifyTask>)> = HashMap::new();
    for task in tasks {
        let key = task.provider_key.clone();
        let endpoint = task.endpoint.clone();
        let entry = groups.entry(key).or_insert_with(|| (endpoint, Vec::new()));
        entry.1.push(task);
    }
    let client = Client::new();
    let futs = groups.into_values().map(|(endpoint, group_tasks)| {
        let client = client.clone();
        async move { verify_provider(client, endpoint, group_tasks).await }
    });
    let mut rows: Vec<VerifyRow> = join_all(futs).await.into_iter().flatten().collect();
    rows.sort_by(|a, b| {
        a.task
            .provider_key
            .cmp(&b.task.provider_key)
            .then(a.task.preset_id.cmp(&b.task.preset_id))
    });
    rows
}

async fn verify_provider(
    client: Client,
    endpoint: ProviderEndpoint,
    tasks: Vec<VerifyTask>,
) -> Vec<VerifyRow> {
    match endpoint {
        ProviderEndpoint::Skipped { reason } => tasks
            .into_iter()
            .map(|task| VerifyRow {
                task,
                result: PresetResult::Skipped { reason },
            })
            .collect(),
        ProviderEndpoint::Anthropic { base_url, api_key } => {
            let url = format!("{base_url}/v1/models");
            match fetch_anthropic_models(&client, &url, &api_key).await {
                Ok(ids) => check_membership(tasks, &ids),
                Err(err) => error_rows(tasks, &err),
            }
        }
        ProviderEndpoint::OpenAiCompat { base_url, api_key } => {
            let url = format!("{base_url}/v1/models");
            match fetch_openai_models(&client, &url, &api_key).await {
                Ok(ids) => check_membership(tasks, &ids),
                Err(err) => error_rows(tasks, &err),
            }
        }
        ProviderEndpoint::Google { base_url, api_key } => {
            let url = format!("{base_url}/v1beta/models");
            match fetch_google_models(&client, &url, &api_key).await {
                Ok(ids) => check_membership(tasks, &ids),
                Err(err) => error_rows(tasks, &err),
            }
        }
    }
}

fn check_membership(tasks: Vec<VerifyTask>, ids: &HashSet<String>) -> Vec<VerifyRow> {
    tasks
        .into_iter()
        .map(|task| {
            let result = if ids.contains(&task.model_id) {
                PresetResult::Pass
            } else {
                PresetResult::Fail {
                    available_count: ids.len(),
                }
            };
            VerifyRow { task, result }
        })
        .collect()
}

fn error_rows(tasks: Vec<VerifyTask>, err: &str) -> Vec<VerifyRow> {
    tasks
        .into_iter()
        .map(|task| VerifyRow {
            task,
            result: PresetResult::NetworkError {
                error: err.to_owned(),
            },
        })
        .collect()
}

async fn fetch_anthropic_models(
    client: &Client,
    url: &str,
    api_key: &str,
) -> Result<HashSet<String>, String> {
    let resp = client
        .get(url)
        .header("x-api-key", api_key)
        .header("anthropic-version", "2023-06-01")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(extract_array_ids(&json, "data", "id"))
}

async fn fetch_openai_models(
    client: &Client,
    url: &str,
    api_key: &str,
) -> Result<HashSet<String>, String> {
    let resp = client
        .get(url)
        .header("Authorization", format!("Bearer {api_key}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let json: Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(extract_array_ids(&json, "data", "id"))
}

async fn fetch_google_models(
    client: &Client,
    url: &str,
    api_key: &str,
) -> Result<HashSet<String>, String> {
    let mut ids = HashSet::new();
    let mut page_token: Option<String> = None;
    for _ in 0..20_u8 {
        let mut req = client.get(url).header("x-goog-api-key", api_key);
        if let Some(token) = &page_token {
            req = req.query(&[("pageToken", token.as_str())]);
        }
        let resp = req.send().await.map_err(|e| e.to_string())?;
        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }
        let json: Value = resp.json().await.map_err(|e| e.to_string())?;
        if let Some(models) = json["models"].as_array() {
            for m in models {
                if let Some(name) = m["name"].as_str() {
                    ids.insert(name.strip_prefix("models/").unwrap_or(name).to_owned());
                }
            }
        }
        page_token = json["nextPageToken"].as_str().map(str::to_owned);
        if page_token.is_none() {
            break;
        }
    }
    Ok(ids)
}

fn extract_array_ids(json: &Value, array_key: &str, id_key: &str) -> HashSet<String> {
    json[array_key]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|item| item[id_key].as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use serde_json::json;

    use crate::catalog::{ProviderEndpoint, VerifyTask};

    fn task(model_id: &str) -> VerifyTask {
        VerifyTask {
            provider_key: "provider".to_owned(),
            preset_id: model_id.to_owned(),
            preset_display: model_id.to_owned(),
            model_id: model_id.to_owned(),
            endpoint: ProviderEndpoint::Skipped { reason: "test" },
        }
    }

    #[tokio::test]
    async fn verify_all_groups_skipped_tasks_and_sorts_rows() {
        let tasks = vec![
            VerifyTask {
                provider_key: "zeta".to_owned(),
                preset_id: "second".to_owned(),
                preset_display: "Second".to_owned(),
                model_id: "zeta-second".to_owned(),
                endpoint: ProviderEndpoint::Skipped {
                    reason: "missing credential",
                },
            },
            VerifyTask {
                provider_key: "alpha".to_owned(),
                preset_id: "first".to_owned(),
                preset_display: "First".to_owned(),
                model_id: "alpha-first".to_owned(),
                endpoint: ProviderEndpoint::Skipped {
                    reason: "local provider",
                },
            },
            VerifyTask {
                provider_key: "zeta".to_owned(),
                preset_id: "first".to_owned(),
                preset_display: "First".to_owned(),
                model_id: "zeta-first".to_owned(),
                endpoint: ProviderEndpoint::Skipped {
                    reason: "missing credential",
                },
            },
        ];

        let rows = super::verify_all(tasks).await;

        let ordered_keys: Vec<_> = rows
            .iter()
            .map(|row| (row.task.provider_key.as_str(), row.task.preset_id.as_str()))
            .collect();
        assert_eq!(
            ordered_keys,
            vec![("alpha", "first"), ("zeta", "first"), ("zeta", "second")]
        );
        assert!(matches!(
            rows[0].result,
            super::PresetResult::Skipped {
                reason: "local provider"
            }
        ));
        assert!(matches!(
            rows[1].result,
            super::PresetResult::Skipped {
                reason: "missing credential"
            }
        ));
        assert!(matches!(
            rows[2].result,
            super::PresetResult::Skipped {
                reason: "missing credential"
            }
        ));
    }

    #[test]
    fn check_membership_marks_known_models_as_passes() {
        let ids = HashSet::from(["known-model".to_owned()]);
        let rows = super::check_membership(vec![task("known-model"), task("missing-model")], &ids);

        assert!(matches!(rows[0].result, super::PresetResult::Pass));
        assert!(matches!(
            rows[1].result,
            super::PresetResult::Fail { available_count: 1 }
        ));
    }

    #[test]
    fn error_rows_preserve_task_context() {
        let rows = super::error_rows(vec![task("model-a"), task("model-b")], "HTTP 500");

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].task.model_id, "model-a");
        assert!(matches!(
            &rows[0].result,
            super::PresetResult::NetworkError { error } if error == "HTTP 500"
        ));
        assert_eq!(rows[1].task.model_id, "model-b");
    }

    #[test]
    fn extract_array_ids_ignores_missing_or_non_string_ids() {
        let json = json!({
            "data": [
                { "id": "model-a" },
                { "id": 42 },
                { "name": "model-b" }
            ]
        });

        let ids = super::extract_array_ids(&json, "data", "id");

        assert_eq!(ids, HashSet::from(["model-a".to_owned()]));
    }
}
