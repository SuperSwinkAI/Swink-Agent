//! Tests for `runner`.
#![cfg(test)]

use super::*;

#[tokio::test]
async fn acquire_case_permit_cancels_while_waiting_for_capacity() {
    let semaphore = Arc::new(Semaphore::new(0));
    let token = CancellationToken::new();
    let waiter_token = token.clone();
    let waiter_semaphore = Arc::clone(&semaphore);
    let waiter = tokio::spawn(async move {
        acquire_case_permit(waiter_semaphore, Some(&waiter_token))
            .await
            .is_none()
    });

    tokio::task::yield_now().await;
    token.cancel();

    assert!(waiter.await.expect("permit waiter should not panic"));
}
