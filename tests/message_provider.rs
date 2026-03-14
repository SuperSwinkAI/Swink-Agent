mod common;

use std::sync::{Arc, Mutex};

use swink_agent::{MessageProvider, from_fns};

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
