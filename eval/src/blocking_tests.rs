//! Tests for `blocking`.
#![cfg(test)]

use std::sync::mpsc;
use std::time::Duration;

use tokio::sync::oneshot;

use super::block_in_place_on;

/// Regression for the eval `run_set` hang: a wakeup addressed to the outer
/// `Runtime::block_on` must survive a nested blocking wait on the same thread.
///
/// The outer future polls `a` (parked on a oneshot) and then `b`, which
/// blocks in place. A spawned task waits until `b` is inside that blocking
/// wait, completes `a` — waking the outer `block_on` — and only then releases
/// `b`. With a nested `Handle::block_on`, the inner wait consumes the outer
/// wakeup and the outer `block_on` never re-polls `a`.
#[test]
fn outer_block_on_wakeup_survives_nested_blocking_wait() {
    let (done_tx, done_rx) = mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("build runtime");
        rt.block_on(async {
            let (a_tx, a_rx) = oneshot::channel::<()>();
            let (b_tx, b_rx) = oneshot::channel::<()>();
            let (entered_tx, entered_rx) = oneshot::channel::<()>();

            tokio::spawn(async move {
                entered_rx.await.expect("b entered its blocking wait");
                a_tx.send(()).expect("a is waiting");
                // Let the nested wait park and observe the wakeup meant for
                // the outer `block_on` before releasing it.
                tokio::time::sleep(Duration::from_millis(50)).await;
                b_tx.send(()).expect("b is waiting");
            });

            let a = async { a_rx.await.expect("a released") };
            let b = async {
                block_in_place_on(async {
                    entered_tx.send(()).expect("spawned task is waiting");
                    b_rx.await.expect("b released");
                });
            };
            tokio::join!(a, b);
        });
        done_tx.send(()).expect("test thread is waiting");
    });

    done_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("outer block_on lost its wakeup and hung");
}
