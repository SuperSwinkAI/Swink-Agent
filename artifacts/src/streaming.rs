use std::collections::HashMap;

use bytes::Bytes;
use futures::{StreamExt, stream};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader, BufWriter};

use swink_agent::{
    ArtifactByteStream, ArtifactData, ArtifactError, ArtifactStore, ArtifactVersion,
    StreamingArtifactStore, validate_artifact_name,
};

use crate::fs_store::FileArtifactStore;

/// 64 KiB chunk size for buffered streaming I/O.
const CHUNK_SIZE: usize = 64 * 1024;

fn storage_err(e: impl std::error::Error + Send + Sync + 'static) -> ArtifactError {
    ArtifactError::Storage(Box::new(e))
}

impl StreamingArtifactStore for FileArtifactStore {
    async fn save_stream(
        &self,
        session_id: &str,
        name: &str,
        content_type: String,
        metadata: HashMap<String, String>,
        mut stream: ArtifactByteStream,
    ) -> Result<ArtifactVersion, ArtifactError> {
        validate_artifact_name(name)?;

        // Collect the stream into a temp file, then delegate to the base save.
        // This keeps version numbering and meta.json logic in one place.
        let mut collected = Vec::new();
        let mut writer = BufWriter::with_capacity(CHUNK_SIZE, &mut collected);

        while let Some(chunk) = stream.next().await {
            let bytes = chunk?;
            writer.write_all(&bytes).await.map_err(storage_err)?;
        }
        writer.flush().await.map_err(storage_err)?;
        drop(writer);

        let data = ArtifactData {
            content: collected,
            content_type,
            metadata,
        };

        self.save(session_id, name, data).await
    }

    async fn load_stream(
        &self,
        session_id: &str,
        name: &str,
        version: Option<u32>,
    ) -> Result<Option<ArtifactByteStream>, ArtifactError> {
        #[derive(serde::Deserialize)]
        struct StreamMetaFile {
            versions: Vec<StreamVersionEntry>,
        }
        #[derive(serde::Deserialize)]
        struct StreamVersionEntry {
            version: u32,
        }

        // Resolve the target version number.
        let target_version = if let Some(v) = version {
            v
        } else {
            let meta_path = self.meta_path(session_id, name);
            let contents = match tokio::fs::read_to_string(&meta_path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(e) => return Err(storage_err(e)),
            };

            let meta: StreamMetaFile = serde_json::from_str(&contents).map_err(storage_err)?;
            match meta.versions.last() {
                Some(entry) => entry.version,
                None => return Ok(None),
            }
        };

        let file_path = self.version_path(session_id, name, target_version);
        let file = match tokio::fs::File::open(&file_path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
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
