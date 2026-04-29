//! JSON-RPC 2.0 peer — reads and writes NDJSON messages over an async I/O pair.

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use super::message::{RawMessage, RequestId, RpcError};

/// Maximum byte length of a single NDJSON line (1 MiB).
pub const MAX_LINE_BYTES: usize = 1024 * 1024;

// ─── IncomingMessage ──────────────────────────────────────────────────────────

/// A message received from the remote peer, ready for the application to handle.
#[derive(Debug)]
pub enum IncomingMessage {
    /// The remote is requesting something; the application must respond.
    Request {
        id: RequestId,
        method: String,
        params: Option<serde_json::Value>,
    },
    /// The remote is notifying; no response expected.
    Notification {
        method: String,
        params: Option<serde_json::Value>,
    },
}

// ─── PeerInner ────────────────────────────────────────────────────────────────

struct PeerInner {
    outbound_tx: mpsc::Sender<RawMessage>,
    pending: Mutex<HashMap<RequestId, oneshot::Sender<Result<serde_json::Value, RpcError>>>>,
    next_id: AtomicU64,
}

// ─── PeerSender ───────────────────────────────────────────────────────────────

/// Cloneable write handle to a [`JsonRpcPeer`].
///
/// Can be captured by the tool-approval callback to send `tool.approve`
/// requests back to the client while the agent stream is in flight.
#[derive(Clone)]
pub struct PeerSender {
    inner: Arc<PeerInner>,
}

impl PeerSender {
    /// Send a JSON-RPC notification to the remote peer (fire-and-forget).
    pub async fn notify<P: Serialize + Sync>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<(), RpcError> {
        let params = serde_json::to_value(params).map_err(|e| RpcError::internal(e.to_string()))?;
        let msg = RawMessage::notification(method, params);
        self.inner
            .outbound_tx
            .send(msg)
            .await
            .map_err(|_| RpcError::disconnected())
    }

    /// Send a JSON-RPC request and await the response.
    pub async fn request<P: Serialize + Sync, R: DeserializeOwned>(
        &self,
        method: &str,
        params: &P,
    ) -> Result<R, RpcError> {
        let id = RequestId::Number(self.inner.next_id.fetch_add(1, Ordering::Relaxed));
        let params = serde_json::to_value(params).map_err(|e| RpcError::internal(e.to_string()))?;

        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            guard.insert(id.clone(), tx);
        }

        let msg = RawMessage::request(id, method, params);
        if self.inner.outbound_tx.send(msg).await.is_err() {
            return Err(RpcError::disconnected());
        }

        let result = rx.await.map_err(|_| RpcError::disconnected())?;
        let value = result?;
        serde_json::from_value(value).map_err(|e| RpcError::internal(e.to_string()))
    }

    /// Send a success response to an inbound request.
    pub fn respond_ok<R: Serialize>(&self, id: RequestId, result: R) -> Result<(), RpcError> {
        let value = serde_json::to_value(result).map_err(|e| RpcError::internal(e.to_string()))?;
        let msg = RawMessage::success(id, value);
        self.inner
            .outbound_tx
            .try_send(msg)
            .map_err(|_| RpcError::disconnected())
    }

    /// Send an error response to an inbound request.
    pub fn respond_err(&self, id: RequestId, err: RpcError) -> Result<(), RpcError> {
        let msg = RawMessage::error_response(id, err);
        self.inner
            .outbound_tx
            .try_send(msg)
            .map_err(|_| RpcError::disconnected())
    }
}

// ─── JsonRpcPeer ──────────────────────────────────────────────────────────────

/// A JSON-RPC 2.0 peer over any async I/O pair.
///
/// Spawns a reader task and a writer task. The caller drives the peer by:
/// - Calling [`sender()`](Self::sender) to send notifications and requests.
/// - Calling [`recv_incoming()`](Self::recv_incoming) to receive requests and
///   notifications from the remote.
pub struct JsonRpcPeer {
    sender: PeerSender,
    incoming_rx: mpsc::Receiver<IncomingMessage>,
}

impl JsonRpcPeer {
    /// Create a peer from a split async I/O pair and spawn the I/O tasks.
    pub fn new<R, W>(read: R, write: W) -> Self
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        let (outbound_tx, outbound_rx) = mpsc::channel::<RawMessage>(64);
        let (incoming_tx, incoming_rx) = mpsc::channel::<IncomingMessage>(64);

        let inner = Arc::new(PeerInner {
            outbound_tx,
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        });

        tokio::spawn(writer_task(write, outbound_rx));
        tokio::spawn(reader_task(read, Arc::clone(&inner), incoming_tx));

        Self {
            sender: PeerSender { inner },
            incoming_rx,
        }
    }

    /// Get a cloneable sender handle (for use in callbacks / other tasks).
    pub fn sender(&self) -> PeerSender {
        self.sender.clone()
    }

    /// Await the next incoming message from the remote peer.
    ///
    /// Returns `None` when the peer has disconnected or the reader task has
    /// exited.
    pub async fn recv_incoming(&mut self) -> Option<IncomingMessage> {
        self.incoming_rx.recv().await
    }
}

// ─── Tasks ────────────────────────────────────────────────────────────────────

async fn writer_task<W: tokio::io::AsyncWrite + Unpin>(
    mut write: W,
    mut rx: mpsc::Receiver<RawMessage>,
) {
    while let Some(msg) = rx.recv().await {
        match serde_json::to_string(&msg) {
            Ok(line) => {
                if write.write_all(line.as_bytes()).await.is_err() {
                    break;
                }
                if write.write_all(b"\n").await.is_err() {
                    break;
                }
            }
            Err(e) => {
                warn!("failed to serialize outbound JSON-RPC message: {e}");
            }
        }
    }
}

async fn reader_task<R: tokio::io::AsyncRead + Unpin>(
    read: R,
    inner: Arc<PeerInner>,
    incoming_tx: mpsc::Sender<IncomingMessage>,
) {
    let mut buf = BufReader::new(read);
    loop {
        match read_bounded_line(&mut buf).await {
            Ok(Some(line)) => {
                dispatch_line(&line, &inner, &incoming_tx).await;
            }
            Ok(None) => break, // EOF
            Err(e) => {
                debug!("JSON-RPC reader I/O error: {e}");
                break;
            }
        }
    }
    // Drain pending requests with a disconnection error.
    let mut guard = inner.pending.lock().unwrap_or_else(|e| e.into_inner());
    for (_, tx) in guard.drain() {
        let _ = tx.send(Err(RpcError::disconnected()));
    }
}

async fn read_bounded_line<R: AsyncBufRead + Unpin>(reader: &mut R) -> io::Result<Option<String>> {
    let mut line = Vec::new();

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            if line.is_empty() {
                return Ok(None);
            }
            return decode_line(line);
        }

        if let Some(newline_index) = available.iter().position(|byte| *byte == b'\n') {
            if line.len() + newline_index > MAX_LINE_BYTES {
                return Err(line_too_long());
            }
            line.extend_from_slice(&available[..newline_index]);
            reader.consume(newline_index + 1);
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return decode_line(line);
        }

        if line.len() + available.len() > MAX_LINE_BYTES {
            return Err(line_too_long());
        }

        line.extend_from_slice(available);
        let consumed = available.len();
        reader.consume(consumed);
    }
}

fn decode_line(line: Vec<u8>) -> io::Result<Option<String>> {
    String::from_utf8(line)
        .map(Some)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
}

fn line_too_long() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("JSON-RPC line exceeds {MAX_LINE_BYTES} bytes"),
    )
}

async fn dispatch_line(
    line: &str,
    inner: &Arc<PeerInner>,
    incoming_tx: &mpsc::Sender<IncomingMessage>,
) {
    let raw: RawMessage = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => {
            warn!("failed to parse JSON-RPC message: {e}");
            return;
        }
    };

    if raw.jsonrpc != "2.0" {
        warn!(
            "received message with unsupported jsonrpc version: {}",
            raw.jsonrpc
        );
        return;
    }

    // Destructure to avoid borrow-after-move issues when routing params.
    let RawMessage {
        id,
        method,
        params,
        result,
        error,
        ..
    } = raw;

    match (id, method) {
        (Some(id), None) => {
            // Response.
            let mut guard = inner.pending.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(tx) = guard.remove(&id) {
                let res = if let Some(err) = error {
                    Err(err)
                } else {
                    Ok(result.unwrap_or(serde_json::Value::Null))
                };
                let _ = tx.send(res);
            } else {
                warn!("received response for unknown id: {id}");
            }
        }
        (Some(id), Some(method)) => {
            // Request.
            let msg = IncomingMessage::Request { id, method, params };
            if incoming_tx.send(msg).await.is_err() {
                debug!("incoming channel closed; dropping request");
            }
        }
        (None, Some(method)) => {
            // Notification.
            let msg = IncomingMessage::Notification { method, params };
            if incoming_tx.send(msg).await.is_err() {
                debug!("incoming channel closed; dropping notification");
            }
        }
        (None, None) => {
            warn!("received invalid JSON-RPC message (no id and no method)");
        }
    }
}
