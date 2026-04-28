#![cfg(feature = "openai")]

use swink_agent_eval_judges::{
    BlockingOpenAIJudgeClient, BlockingOpenAiJudgeClient, OpenAIJudgeClient, OpenAiJudgeClient,
};

#[test]
fn crate_root_reexports_spec_cased_openai_aliases() {
    let client: OpenAiJudgeClient =
        OpenAIJudgeClient::new("https://example.com", "test-key", "gpt-test");
    let blocking_client: BlockingOpenAiJudgeClient = BlockingOpenAIJudgeClient::new(client);

    let _ = blocking_client;
}
