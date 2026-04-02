# Quickstart: TransferToAgent Tool & Handoff Safety

**Feature**: 040-agent-transfer-handoff  
**Date**: 2026-04-02

## Basic Transfer Setup

```rust
use swink_agent::{
    Agent, AgentOptions, AgentRegistry, TransferToAgentTool, StopReason,
};

// 1. Create a registry with agents
let registry = Arc::new(AgentRegistry::new());
registry.register("billing", Agent::new(AgentOptions::new(
    "You handle billing questions.", model.clone(), stream_fn.clone(), convert,
)));
registry.register("technical", Agent::new(AgentOptions::new(
    "You handle technical issues.", model.clone(), stream_fn.clone(), convert,
)));

// 2. Create a triage agent with the transfer tool
let transfer_tool = TransferToAgentTool::new(registry.clone());
let triage = Agent::new(AgentOptions::new(
    "You triage customer requests. Transfer to the appropriate agent.",
    model, stream_fn, convert,
).with_tool(Arc::new(transfer_tool)));

// 3. Run the triage agent
let result = triage.prompt("I have a billing question").await?;

// 4. Check for transfer
if result.stop_reason == StopReason::Transfer {
    let signal = result.transfer_signal.unwrap();
    println!("Transfer to: {}", signal.target_agent());
    println!("Reason: {}", signal.reason());
    // Dispatch target agent with signal.conversation_history()
}
```

## Restricted Transfers

```rust
// Only allow transfers to billing and technical
let transfer_tool = TransferToAgentTool::with_allowed_targets(
    registry.clone(),
    ["billing", "technical"],
);

// Attempting to transfer to "admin" will return an error result
// to the LLM, which can then self-correct or inform the user
```

## Circular Transfer Detection

```rust
use swink_agent::{TransferChain, TransferError};

// Create a chain for this user message
let mut chain = TransferChain::default(); // max_depth: 5

// Orchestrator pushes each agent before dispatching
chain.push("triage")?;      // OK — first agent
chain.push("billing")?;     // OK — new agent

// If billing tries to transfer back to triage:
match chain.push("triage") {
    Err(TransferError::CircularTransfer { agent_name, .. }) => {
        // Reject the transfer — triage is already in the chain
        println!("Circular transfer detected: {}", agent_name);
    }
    Ok(()) => { /* proceed with transfer */ }
    Err(TransferError::MaxDepthExceeded { .. }) => {
        println!("Too many transfers in one message");
    }
}
```

## Full Orchestration Loop

```rust
let mut chain = TransferChain::default();
let mut current_agent_name = "triage".to_string();
let mut messages = vec![user_message];

loop {
    chain.push(&current_agent_name)?;
    
    let agent_ref = registry.get(&current_agent_name)
        .ok_or("agent not found")?;
    let mut agent = agent_ref.lock().await;
    
    let result = agent.prompt_with_messages(messages).await?;
    
    match result.stop_reason {
        StopReason::Transfer => {
            let signal = result.transfer_signal.unwrap();
            current_agent_name = signal.target_agent().to_string();
            messages = signal.conversation_history().to_vec();
            // Loop continues with target agent
        }
        StopReason::Stop => {
            println!("Final response: {}", result.assistant_text());
            break;
        }
        _ => break,
    }
}
```
