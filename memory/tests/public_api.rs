use swink_agent_memory::{
    BlockingSessionStore, CompactionResult, InterruptState, JsonlSessionStore, LoadOptions,
    PendingToolCall, SessionEntry, SessionMeta, SessionMigrator, SessionStore,
    SessionStoreFuture, SummarizingCompactor, format_session_id, now_utc,
};

#[test]
fn memory_root_reexports_remain_consumable() {
    let _ = std::any::type_name::<JsonlSessionStore>();
    let _ = std::any::type_name::<dyn SessionStore>();
    let _ = std::any::type_name::<SessionMeta>();
    let _ = std::any::type_name::<SessionEntry>();
    let _ = std::any::type_name::<InterruptState>();
    let _ = std::any::type_name::<PendingToolCall>();
    let _ = std::any::type_name::<LoadOptions>();
    let _ = std::any::type_name::<CompactionResult>();
    let _ = std::any::type_name::<SummarizingCompactor>();
    let _ = std::any::type_name::<dyn SessionMigrator>();
    let _ = std::any::type_name::<BlockingSessionStore<JsonlSessionStore>>();
    let _ = std::any::type_name::<SessionStoreFuture<'static, ()>>();

    let id = format_session_id();
    assert_eq!(id.len(), 15);
    let now = now_utc();
    assert!(now.timestamp() > 1_700_000_000);
}
