mod common;

use std::sync::{Arc, Mutex};

use swink_agent::{
    ComposedMessageProvider, MessageProvider, from_fns, message_channel,
};

use common::user_msg;

#[test]
fn from_fns_creates_working_provider() {
    let provider = from_fns(
        || vec![user_msg("steer")],
        || vec![user_msg("follow")],
    );

    let steering = provider.poll_steering();
    assert_eq!(steering.len(), 1);

    let follow_up = provider.poll_follow_up();
    assert_eq!(follow_up.len(), 1);
}

#[test]
fn stateful_steering_closure() {
    let call_count = Arc::new(Mutex::new(0u32));
    let cc = Arc::clone(&call_count);

    let provider = from_fns(
        move || {
            *cc.lock().unwrap() += 1;
            vec![user_msg("steer")]
        },
        Vec::new,
    );

    provider.poll_steering();
    provider.poll_steering();
    provider.poll_steering();

    assert_eq!(*call_count.lock().unwrap(), 3);
}

#[test]
fn stateful_follow_up_closure() {
    let call_log = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = Arc::clone(&call_log);

    let provider = from_fns(
        Vec::new,
        move || {
            log.lock().unwrap().push("called".to_owned());
            vec![user_msg("follow")]
        },
    );

    provider.poll_follow_up();
    provider.poll_follow_up();

    let log = call_log.lock().unwrap();
    assert_eq!(log.len(), 2);
    assert!(log.iter().all(|e| e == "called"));
    drop(log);
}

#[test]
fn empty_returns() {
    let provider = from_fns(Vec::new, Vec::new);

    assert!(provider.poll_steering().is_empty());
    assert!(provider.poll_follow_up().is_empty());
}

// ─── Channel-based MessageProvider ──────────────────────────────────────────

#[test]
fn channel_follow_up_delivers_messages() {
    let (provider, sender) = message_channel();

    assert!(sender.send(user_msg("hello")));
    assert!(sender.send(user_msg("world")));

    let msgs = provider.poll_follow_up();
    assert_eq!(msgs.len(), 2);
    // Steering should be empty
    assert!(provider.poll_steering().is_empty());
}

#[test]
fn channel_steering_delivers_messages() {
    let (provider, sender) = message_channel();

    assert!(sender.send_steering(user_msg("interrupt")));

    let msgs = provider.poll_steering();
    assert_eq!(msgs.len(), 1);
    // Follow-up should be empty
    assert!(provider.poll_follow_up().is_empty());
}

#[test]
fn channel_empty_when_nothing_sent() {
    let (provider, _sender) = message_channel();

    assert!(provider.poll_steering().is_empty());
    assert!(provider.poll_follow_up().is_empty());
}

#[test]
fn channel_drains_all_buffered_messages() {
    let (provider, sender) = message_channel();

    for i in 0..5 {
        sender.send_follow_up(user_msg(&format!("msg-{i}")));
    }

    let msgs = provider.poll_follow_up();
    assert_eq!(msgs.len(), 5);

    // Second poll returns empty
    assert!(provider.poll_follow_up().is_empty());
}

#[test]
fn channel_sender_returns_false_after_provider_dropped() {
    let (provider, sender) = message_channel();
    drop(provider);

    assert!(!sender.send(user_msg("orphaned")));
    assert!(!sender.send_steering(user_msg("orphaned")));
}

#[test]
fn channel_sender_is_clone() {
    let (provider, sender) = message_channel();
    let sender2 = sender.clone();

    sender.send(user_msg("from-1"));
    sender2.send(user_msg("from-2"));

    let msgs = provider.poll_follow_up();
    assert_eq!(msgs.len(), 2);
}

#[test]
fn channel_interleaved_steering_and_follow_up() {
    let (provider, sender) = message_channel();

    sender.send_steering(user_msg("steer-1"));
    sender.send_follow_up(user_msg("follow-1"));
    sender.send_steering(user_msg("steer-2"));

    assert_eq!(provider.poll_steering().len(), 2);
    assert_eq!(provider.poll_follow_up().len(), 1);
}

// ─── ComposedMessageProvider ────────────────────────────────────────────────

#[test]
fn composed_provider_merges_both() {
    let primary = Arc::new(from_fns(
        || vec![user_msg("primary-steer")],
        || vec![user_msg("primary-follow")],
    ));
    let secondary = Arc::new(from_fns(
        || vec![user_msg("secondary-steer")],
        || vec![user_msg("secondary-follow")],
    ));

    let composed = ComposedMessageProvider::new(primary, secondary);

    let steering = composed.poll_steering();
    assert_eq!(steering.len(), 2);

    let follow_up = composed.poll_follow_up();
    assert_eq!(follow_up.len(), 2);
}
