use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use swink_agent_auth::{ExpiringValue, SingleFlightTokenSource};

#[tokio::test]
async fn concurrent_refresh_deduplicates_single_token_source() {
    let source = Arc::new(SingleFlightTokenSource::new(Duration::from_secs(60)));
    let refreshes = Arc::new(AtomicUsize::new(0));

    let left_source = Arc::clone(&source);
    let left_refreshes = Arc::clone(&refreshes);
    let right_source = Arc::clone(&source);
    let right_refreshes = Arc::clone(&refreshes);

    let (left, right) = tokio::join!(
        tokio::spawn(async move {
            left_source
                .get_or_refresh(move || async move {
                    left_refreshes.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok::<_, String>(ExpiringValue::new(
                        "shared-token".to_string(),
                        Instant::now() + Duration::from_secs(300),
                    ))
                })
                .await
        }),
        tokio::spawn(async move {
            right_source
                .get_or_refresh(move || async move {
                    right_refreshes.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok::<_, String>(ExpiringValue::new(
                        "shared-token".to_string(),
                        Instant::now() + Duration::from_secs(300),
                    ))
                })
                .await
        }),
    );

    assert_eq!(left.unwrap().unwrap(), "shared-token");
    assert_eq!(right.unwrap().unwrap(), "shared-token");
    assert_eq!(refreshes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn cached_token_skips_refresh() {
    let source = SingleFlightTokenSource::new(Duration::from_secs(60));

    let first = source
        .get_or_refresh(|| async {
            Ok::<_, String>(ExpiringValue::new(
                "initial-token".to_string(),
                Instant::now() + Duration::from_secs(300),
            ))
        })
        .await
        .unwrap();
    assert_eq!(first, "initial-token");

    let refreshes = Arc::new(AtomicUsize::new(0));
    let cached = source
        .get_or_refresh({
            let refreshes = Arc::clone(&refreshes);
            move || async move {
                refreshes.fetch_add(1, Ordering::SeqCst);
                Ok::<_, String>(ExpiringValue::new(
                    "unexpected-refresh".to_string(),
                    Instant::now() + Duration::from_secs(300),
                ))
            }
        })
        .await
        .unwrap();

    assert_eq!(cached, "initial-token");
    assert_eq!(refreshes.load(Ordering::SeqCst), 0);
}
