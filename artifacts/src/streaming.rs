use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bytes::Bytes;
use futures::{StreamExt, stream};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use swink_agent::{ArtifactByteStream, ArtifactError, ArtifactVersion, StreamingArtifactStore};

use crate::fs_store::{
    FileArtifactStore, VersionRecord, missing_content_err, orphan_content_err, storage_err,
};

/// 64 KiB chunk size for buffered streaming I/O.
const CHUNK_SIZE: usize = 64 * 1024;
static STREAM_TMP_SEQ: AtomicU64 = AtomicU64::new(0);

fn temp_version_path(target: &Path) -> Result<PathBuf, ArtifactError> {
    let parent = target.parent().ok_or_else(|| {
        storage_err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "target path has no parent directory",
        ))
    })?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            storage_err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "target path has no file name",
            ))
        })?;
    let seq = STREAM_TMP_SEQ.fetch_add(1, Ordering::Relaxed);

    Ok(parent.join(format!(
        ".{file_name}.stream.tmp.{}.{seq}",
        std::process::id()
    )))
}

async fn write_stream_to_temp_file(
    temp_path: &Path,
    mut stream: ArtifactByteStream,
) -> Result<usize, ArtifactError> {
    let write_result = async {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp_path)
            .await
            .map_err(storage_err)?;
        let mut writer = BufWriter::with_capacity(CHUNK_SIZE, file);
        let mut size = 0usize;

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            size = size.checked_add(bytes.len()).ok_or_else(|| {
                storage_err(std::io::Error::other("artifact stream size overflow"))
            })?;
            writer.write_all(&bytes).await.map_err(storage_err)?;
        }

        writer.flush().await.map_err(storage_err)?;
        let file = writer.into_inner();
        file.sync_all().await.map_err(storage_err)?;

        Ok(size)
    }
    .await;

    if write_result.is_err() {
        let _ = tokio::fs::remove_file(temp_path).await;
    }

    write_result
}

impl StreamingArtifactStore for FileArtifactStore {
    async fn save_stream(
        &self,
        session_id: &str,
        name: &str,
        content_type: String,
        metadata: HashMap<String, String>,
        stream: ArtifactByteStream,
    ) -> Result<ArtifactVersion, ArtifactError> {
        let dir = self.resolve_artifact_dir(session_id, name)?;

        let lock = self.artifact_lock(session_id, name).await;
        let _guard = lock.lock().await;

        tokio::fs::create_dir_all(&dir).await.map_err(storage_err)?;

        let mut meta = self.read_meta(session_id, name).await?;

        #[allow(clippy::cast_possible_truncation)]
        let next_version = meta.versions.len() as u32 + 1;
        let now = chrono::Utc::now();
        let content_path = self.version_path(session_id, name, next_version);
        let temp_path = temp_version_path(&content_path)?;
        let size = write_stream_to_temp_file(&temp_path, stream).await?;

        if let Err(error) = tokio::fs::rename(&temp_path, &content_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(storage_err(error));
        }

        let record = VersionRecord {
            name: name.to_string(),
            version: next_version,
            created_at: now,
            size,
            content_type: content_type.clone(),
            metadata: metadata.clone(),
        };
        let version = ArtifactVersion {
            name: name.to_string(),
            version: next_version,
            created_at: now,
            size,
            content_type,
        };
        meta.versions.push(record);
        if let Err(error) = self.write_meta(session_id, name, &meta).await {
            Self::rollback_version_file(&content_path).await;
            return Err(error);
        }

        tracing::info!(
            session_id,
            name,
            version = next_version,
            size,
            "artifact saved from stream"
        );

        Ok(version)
    }

    async fn load_stream(
        &self,
        session_id: &str,
        name: &str,
        version: Option<u32>,
    ) -> Result<Option<ArtifactByteStream>, ArtifactError> {
        self.resolve_artifact_dir(session_id, name)?;
        let meta = self.read_meta(session_id, name).await?;

        let target_record = if let Some(version) = version {
            if let Some(record) = meta
                .versions
                .iter()
                .find(|record| record.version == version)
            {
                record
            } else {
                let content_path = self.version_path(session_id, name, version);
                if tokio::fs::try_exists(&content_path)
                    .await
                    .map_err(storage_err)?
                {
                    return Err(orphan_content_err(session_id, name, version, &content_path));
                }
                return Ok(None);
            }
        } else {
            match meta.versions.last() {
                Some(entry) => entry,
                None => return Ok(None),
            }
        };

        let file_path = self.version_path(session_id, name, target_record.version);
        let file = match tokio::fs::File::open(&file_path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(missing_content_err(
                    session_id,
                    name,
                    target_record.version,
                    &file_path,
                ));
            }
            Err(e) => return Err(storage_err(e)),
        };

        let reader = BufReader::with_capacity(CHUNK_SIZE, file);

        // Use try_unfold to produce a stream of 64KB chunks.
        let chunk_stream = stream::try_unfold(reader, |mut reader| async move {
            let mut buf = vec![0u8; CHUNK_SIZE];
            let n = reader.read(&mut buf).await.map_err(storage_err)?;
            if n == 0 {
                return Ok(None);
            }
            buf.truncate(n);
            Ok(Some((Bytes::from(buf), reader)))
        });

        Ok(Some(Box::pin(chunk_stream)))
    }
}
