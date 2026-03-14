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
                PresetResult::Fail { available_count: ids.len() }
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
            result: PresetResult::NetworkError { error: err.to_owned() },
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
