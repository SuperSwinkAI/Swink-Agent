use swink_agent_rpc::jsonrpc::{IncomingMessage, JsonRpcPeer, RpcError};
use tokio::io::duplex;

fn make_peer_pair() -> (JsonRpcPeer, JsonRpcPeer) {
    let (a_read, b_write) = duplex(8192);
    let (b_read, a_write) = duplex(8192);
    let a = JsonRpcPeer::new(a_read, a_write);
    let b = JsonRpcPeer::new(b_read, b_write);
    (a, b)
}

#[tokio::test]
async fn notification_round_trip() {
    let (mut a, mut b) = make_peer_pair();

    a.sender().notify("ping", &serde_json::json!({"seq": 1})).await.unwrap();
    drop(a);

    let msg = b.recv_incoming().await.unwrap();
    match msg {
        IncomingMessage::Notification { method, params } => {
            assert_eq!(method, "ping");
            assert_eq!(params.unwrap()["seq"], 1);
        }
        other => panic!("expected notification, got {other:?}"),
    }
}

#[tokio::test]
async fn request_response_round_trip() {
    let (mut a, mut b) = make_peer_pair();

    let sender_b = b.sender();
    let server = tokio::spawn(async move {
        let msg = b.recv_incoming().await.unwrap();
        match msg {
            IncomingMessage::Request { id, method, params } => {
                assert_eq!(method, "echo");
                sender_b.respond_ok(id, params.unwrap()).unwrap();
            }
            other => panic!("expected request, got {other:?}"),
        }
    });

    let result: serde_json::Value = a
        .sender()
        .request("echo", &serde_json::json!({"text": "hello"}))
        .await
        .unwrap();

    assert_eq!(result["text"], "hello");
    server.await.unwrap();
}

#[tokio::test]
async fn error_response_surfaces_as_err() {
    let (mut a, mut b) = make_peer_pair();

    let sender_b = b.sender();
    tokio::spawn(async move {
        let msg = b.recv_incoming().await.unwrap();
        if let IncomingMessage::Request { id, .. } = msg {
            sender_b
                .respond_err(id, RpcError::method_not_found("unknown"))
                .unwrap();
        }
    });

    let result: Result<serde_json::Value, RpcError> =
        a.sender().request("unknown", &serde_json::Value::Null).await;

    let err = result.unwrap_err();
    assert_eq!(err.code, RpcError::METHOD_NOT_FOUND);
}

#[tokio::test]
async fn pending_requests_fail_on_disconnect() {
    let (a_read, b_write) = duplex(8192);
    let (b_read, a_write) = duplex(8192);
    let a = JsonRpcPeer::new(a_read, a_write);
    let b = JsonRpcPeer::new(b_read, b_write);

    // Drop b immediately to simulate peer disconnect.
    drop(b);

    let result: Result<serde_json::Value, RpcError> =
        a.sender().request("anything", &serde_json::Value::Null).await;

    assert_eq!(result.unwrap_err().code, RpcError::DISCONNECTED);
}

#[tokio::test]
async fn concurrent_requests_correlate_correctly() {
    let (mut a, mut b) = make_peer_pair();

    let sender_b = b.sender();
    tokio::spawn(async move {
        // Handle two requests, responding in reversed order.
        let msg1 = b.recv_incoming().await.unwrap();
        let msg2 = b.recv_incoming().await.unwrap();
        if let (IncomingMessage::Request { id: id2, .. }, IncomingMessage::Request { id: id1, .. }) =
            (msg2, msg1)
        {
            sender_b.respond_ok(id2, serde_json::json!("second")).unwrap();
            sender_b.respond_ok(id1, serde_json::json!("first")).unwrap();
        }
    });

    let sender_a = a.sender();
    let f1 = tokio::spawn({
        let s = sender_a.clone();
        async move { s.request::<_, serde_json::Value>("r1", &serde_json::Value::Null).await }
    });
    let f2 = tokio::spawn(async move {
        sender_a.request::<_, serde_json::Value>("r2", &serde_json::Value::Null).await
    });

    let (r1, r2) = tokio::join!(f1, f2);
    // Both should succeed regardless of ordering.
    assert!(r1.unwrap().is_ok());
    assert!(r2.unwrap().is_ok());
}

#[tokio::test]
async fn oversize_line_closes_connection() {
    use tokio::io::AsyncWriteExt as _;

    // Wire: raw_writer -> peer_reader.  peer_writer -> /dev/null (b_write dropped).
    let (raw_reader, mut raw_writer) = duplex(4 * 1024 * 1024);
    let (sink_reader, sink_writer) = duplex(8192);
    let mut peer = JsonRpcPeer::new(raw_reader, sink_writer);
    drop(sink_reader); // we don't care about output

    let oversized = "x".repeat(swink_agent_rpc::jsonrpc::MAX_LINE_BYTES + 1);
    raw_writer.write_all(oversized.as_bytes()).await.unwrap();
    raw_writer.write_all(b"\n").await.unwrap();
    drop(raw_writer);

    // After the oversized line the reader task exits, so recv returns None.
    let result = peer.recv_incoming().await;
    assert!(result.is_none(), "connection should close after oversized line");
}
