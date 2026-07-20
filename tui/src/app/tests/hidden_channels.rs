use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::TuiConfig;

use super::super::*;

#[tokio::test]
async fn f5_toggles_show_hidden_channels() {
    let mut app = App::new(TuiConfig::default());
    assert!(
        !app.view.conversation.show_hidden_channels,
        "off by default"
    );

    let key = KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE);
    app.handle_key_event(key);
    assert!(app.view.conversation.show_hidden_channels);

    app.handle_key_event(key);
    assert!(!app.view.conversation.show_hidden_channels);
}
