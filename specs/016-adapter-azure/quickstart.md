# Quickstart: 016-adapter-azure

**Date**: 2026-04-02

## Usage — API Key Auth

```rust
use swink_agent_adapters::AzureStreamFn;
use swink_agent_adapters::azure::AzureAuth;

let stream_fn = AzureStreamFn::new(
    "https://my-resource.openai.azure.com/openai/deployments/gpt-4o",
    AzureAuth::ApiKey("my-api-key".into()),
);

let agent = Agent::builder()
    .stream_fn(stream_fn)
    .model("gpt-4o")  // deployment's model
    .build();
```

## Usage — Azure AD / Entra ID Auth

```rust
use swink_agent_adapters::AzureStreamFn;
use swink_agent_adapters::azure::AzureAuth;

let stream_fn = AzureStreamFn::new(
    "https://my-resource.openai.azure.com/openai/deployments/gpt-4o",
    AzureAuth::EntraId {
        tenant_id: "my-tenant-id".into(),
        client_id: "my-client-id".into(),
        client_secret: "my-client-secret".into(),
    },
);
```

## Error Handling

```rust
use swink_agent::error::AgentError;

match agent.prompt("hello").await {
    Ok(msg) => println!("{msg:?}"),
    Err(AgentError::ContentFiltered) => {
        eprintln!("Content was blocked by Azure's safety filter");
    }
    Err(AgentError::ModelThrottled) => {
        eprintln!("Rate limited — retry with backoff");
    }
    Err(e) => eprintln!("Error: {e}"),
}
```

## Feature Gate

```toml
# Cargo.toml
[dependencies]
swink-agent-adapters = { version = "...", features = ["azure"] }
```

## URL Format

The base URL should include the deployment path:
```
https://{resource}.openai.azure.com/openai/deployments/{deployment}
```

The adapter appends `/chat/completions` automatically. Trailing slashes are stripped.
