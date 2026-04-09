use swink_agent_tui::{App, TuiConfig};

#[test]
fn tui_reexports_remain_consumable() {
    let _: fn(TuiConfig) -> App = App::new;
}
