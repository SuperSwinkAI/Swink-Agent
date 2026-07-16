use std::pin::Pin;
use std::sync::Arc;

use futures::Stream;
use tokio_util::sync::CancellationToken;

use swink_agent::{
    AgentContext, AssistantMessageEvent, ModelConnection, ModelConnections, ModelSpec, StreamFn,
    StreamOptions,
};

struct MockDummyStreamFn;

impl StreamFn for MockDummyStreamFn {
    fn stream<'a>(
        &'a self,
        _model: &'a ModelSpec,
        _context: &'a AgentContext,
        _options: &'a StreamOptions,
        _cancellation_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = AssistantMessageEvent> + Send + 'a>> {
        Box::pin(futures::stream::empty())
    }
}

fn dummy_stream() -> Arc<dyn StreamFn> {
    Arc::new(MockDummyStreamFn)
}

#[test]
fn custom_agent_example_connections_use_expected_order() {
    let connections = ModelConnections::new(
        ModelConnection::new(
            ModelSpec::new("anthropic", "claude-sonnet-4-6"),
            dummy_stream(),
        ),
        vec![
            ModelConnection::new(ModelSpec::new("openai", "gpt-5.2"), dummy_stream()),
            ModelConnection::new(ModelSpec::new("local", "SmolLM3-3B-Q4_K_M"), dummy_stream()),
        ],
    );

    assert_eq!(
        connections.primary_model(),
        &ModelSpec::new("anthropic", "claude-sonnet-4-6")
    );
    assert_eq!(connections.extra_models().len(), 2);
    assert_eq!(
        connections.extra_models()[0].0,
        ModelSpec::new("openai", "gpt-5.2")
    );
    assert_eq!(
        connections.extra_models()[1].0,
        ModelSpec::new("local", "SmolLM3-3B-Q4_K_M")
    );
}
