//! Regression test for the `ToolDescription` mutation pathway.
//!
//! Prior to this fix, `MutatingAgentFactory` had no mechanism at all for
//! `ToolDescription` candidates — `prompt_override` returned `None` for that
//! target, so every `ToolDescription` candidate evaluated to an agent
//! identical to the baseline and could never be accepted (FR-009/010/011/014).
//!
//! `ToolDescriptionOverride` threads the mutated description through
//! `case.metadata`, and a cooperating factory applies it via
//! `apply_tool_description_override` before constructing the `Agent`. This
//! test proves the override actually reaches the constructed agent's tool.

use std::sync::Arc;

use swink_agent::testing::{MockTool, SimpleMockStreamFn};
use swink_agent::{Agent, AgentOptions, IntoTool, ModelSpec};
use swink_agent_eval::{AgentFactory, EvalCase, EvalError, ResponseCriteria, Score};
use tokio_util::sync::CancellationToken;

use swink_agent_evolve::{MutatingAgentFactory, ToolDescriptionOverride};

/// A factory that builds its tool list fresh per case and cooperates with
/// `ToolDescriptionOverride` — the contract any real factory must implement
/// for `ToolDescription` candidates to take effect.
struct ToolAwareFactory;

impl AgentFactory for ToolAwareFactory {
    fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
        let tools: Vec<_> = vec![MockTool::new("my_tool").into_tool()];
        let tools = match ToolDescriptionOverride::from_case(case) {
            Some(override_) => {
                swink_agent_evolve::apply_tool_description_override(tools, &override_)
            }
            None => tools,
        };

        let stream_fn = Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()]));
        let model = ModelSpec::new("test", "test-model");
        let options =
            AgentOptions::new_simple(&case.system_prompt, model, stream_fn).with_tools(tools);
        Ok((Agent::new(options), CancellationToken::new()))
    }
}

fn case() -> EvalCase {
    EvalCase::new(
        "c1",
        "c1",
        "You are helpful.",
        vec!["hello".to_string()],
    )
    .with_expected_response(ResponseCriteria::Custom(Arc::new(|_: &str| {
        Score::new(1.0, 0.5)
    })))
}

#[test]
fn tool_description_candidate_changes_the_built_agent_tool() {
    let inner = Arc::new(ToolAwareFactory);
    let case = case();

    // Baseline: no override.
    let (baseline_agent, _) =
        MutatingAgentFactory::new(Arc::clone(&inner) as Arc<dyn AgentFactory>, None)
            .create_agent(&case)
            .unwrap();
    let baseline_description = baseline_agent.state().tools[0].description().to_string();

    // Candidate: ToolDescription override for "my_tool".
    let override_ = ToolDescriptionOverride {
        tool_name: "my_tool".to_string(),
        description: "Completely rewritten description for my_tool.".to_string(),
    };
    let (candidate_agent, _) = MutatingAgentFactory::new(inner as Arc<dyn AgentFactory>, None)
        .with_tool_override(Some(override_.clone()))
        .create_agent(&case)
        .unwrap();
    let candidate_description = candidate_agent.state().tools[0].description().to_string();

    assert_ne!(
        baseline_description, candidate_description,
        "ToolDescription candidate must change the constructed agent's tool description"
    );
    assert_eq!(candidate_description, override_.description);
}

#[test]
fn factory_that_ignores_the_override_key_is_unaffected() {
    // A factory that doesn't read `ToolDescriptionOverride::from_case` (the
    // pre-existing behavior of any factory unaware of the contract) must
    // still function — the override is inert for it, not a silent corruption.
    struct IgnorantFactory;
    impl AgentFactory for IgnorantFactory {
        fn create_agent(&self, case: &EvalCase) -> Result<(Agent, CancellationToken), EvalError> {
            let stream_fn = Arc::new(SimpleMockStreamFn::new(vec!["ok".to_string()]));
            let model = ModelSpec::new("test", "test-model");
            let options = AgentOptions::new_simple(&case.system_prompt, model, stream_fn)
                .with_tools(vec![MockTool::new("my_tool").into_tool()]);
            Ok((Agent::new(options), CancellationToken::new()))
        }
    }

    let inner = Arc::new(IgnorantFactory) as Arc<dyn AgentFactory>;
    let override_ = ToolDescriptionOverride {
        tool_name: "my_tool".to_string(),
        description: "new description".to_string(),
    };
    let (agent, _) = MutatingAgentFactory::new(inner, None)
        .with_tool_override(Some(override_))
        .create_agent(&case())
        .unwrap();
    assert_eq!(
        agent.state().tools[0].description(),
        "A mock tool for testing"
    );
}
