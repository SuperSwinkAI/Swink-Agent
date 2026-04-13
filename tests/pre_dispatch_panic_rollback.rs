use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use swink_agent::{
    PreDispatchPolicy, PreDispatchVerdict, SessionState, ToolDispatchContext,
    run_pre_dispatch_policies,
};

struct PanickingMutatingPolicy;

impl PreDispatchPolicy for PanickingMutatingPolicy {
    fn name(&self) -> &str {
        "panicking_mutator"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        ctx.arguments
            .as_object_mut()
            .expect("test arguments should be an object")
            .insert("panic_injected".to_string(), serde_json::json!("leaked"));
        panic!("panic after mutating arguments");
    }
}

struct RejectIfKeyPresentPolicy {
    called: Arc<AtomicBool>,
}

impl PreDispatchPolicy for RejectIfKeyPresentPolicy {
    fn name(&self) -> &str {
        "reject_if_key_present"
    }

    fn evaluate(&self, ctx: &mut ToolDispatchContext<'_>) -> PreDispatchVerdict {
        self.called.store(true, Ordering::SeqCst);
        if ctx.arguments.get("panic_injected").is_some() {
            PreDispatchVerdict::Skip("panic mutation leaked".to_string())
        } else {
            PreDispatchVerdict::Continue
        }
    }
}

#[test]
fn panicking_pre_dispatch_policy_restores_arguments_before_continuing() {
    let called = Arc::new(AtomicBool::new(false));
    let policies: Vec<Arc<dyn PreDispatchPolicy>> = vec![
        Arc::new(PanickingMutatingPolicy),
        Arc::new(RejectIfKeyPresentPolicy {
            called: Arc::clone(&called),
        }),
    ];
    let state = SessionState::new();
    let mut arguments = serde_json::json!({
        "path": "safe.txt",
        "mode": "overwrite"
    });
    let mut ctx = ToolDispatchContext {
        tool_name: "write_file",
        tool_call_id: "call-1",
        arguments: &mut arguments,
        execution_root: None,
        state: &state,
    };

    let verdict = run_pre_dispatch_policies(&policies, &mut ctx);

    assert!(matches!(verdict, PreDispatchVerdict::Continue));
    assert!(called.load(Ordering::SeqCst));
    assert_eq!(
        arguments,
        serde_json::json!({
            "path": "safe.txt",
            "mode": "overwrite"
        })
    );
}
