mod extract;
mod fetch;
mod screenshot;
mod search;

use std::future::Future;
use std::time::Duration;

use tokio_util::sync::CancellationToken;

pub use extract::ExtractTool;
pub use fetch::FetchTool;
pub use screenshot::ScreenshotTool;
pub use search::SearchTool;

enum OperationOutcome<T> {
    Completed(T),
    Cancelled,
    TimedOut,
}

async fn await_with_cancellation<F, T>(
    cancellation_token: &CancellationToken,
    timeout: Duration,
    future: F,
) -> OperationOutcome<T>
where
    F: Future<Output = T>,
{
    tokio::select! {
        result = tokio::time::timeout(timeout, future) => match result {
            Ok(value) => OperationOutcome::Completed(value),
            Err(_) => OperationOutcome::TimedOut,
        },
        () = cancellation_token.cancelled() => OperationOutcome::Cancelled,
    }
}

#[cfg(test)]
mod tests {
    use std::future::pending;
    use std::time::Duration;

    use tokio_util::sync::CancellationToken;

    use super::{OperationOutcome, await_with_cancellation};

    #[tokio::test]
    async fn await_with_cancellation_returns_cancelled_before_completion() {
        let cancellation_token = CancellationToken::new();
        cancellation_token.cancel();

        let outcome =
            await_with_cancellation(&cancellation_token, Duration::from_secs(1), pending::<()>())
                .await;

        assert!(matches!(outcome, OperationOutcome::Cancelled));
    }

    #[tokio::test]
    async fn await_with_cancellation_returns_timed_out_for_slow_operations() {
        let outcome = await_with_cancellation(
            &CancellationToken::new(),
            Duration::from_millis(10),
            tokio::time::sleep(Duration::from_millis(50)),
        )
        .await;

        assert!(matches!(outcome, OperationOutcome::TimedOut));
    }
}
