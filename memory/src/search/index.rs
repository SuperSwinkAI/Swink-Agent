//! Tantivy-backed full-text index for cross-session search.
//!
//! The index is stored in `<sessions_dir>/.search_index/` alongside the JSONL
//! session files.  It is rebuilt (or opened) lazily on the first `search()`
//! call and updated incrementally when sessions are re-indexed via
//! [`TantivyIndex::index_session`].
//!
//! ## Schema
//!
//! | Field          | Type | Options        | Purpose                                   |
//! |----------------|------|----------------|-------------------------------------------|
//! | `session_id`   | text | STORED, raw    | Filter / reconstruct hit                  |
//! | `session_title`| text | STORED, raw    | Display title                             |
//! | `entry_type`   | text | STORED, raw    | Type discriminator for filter             |
//! | `timestamp`    | u64  | INDEXED, STORED| Timestamp range filter                    |
//! | `body`         | text | TEXT (en_stem) | Tokenised full-text content               |
//! | `entry_json`   | text | STORED         | Raw JSON for roundtrip to `SessionEntry`  |

use std::io;
use std::path::Path;
use std::sync::{Arc, Mutex};

use tantivy::collector::TopDocs;
use tantivy::query::QueryParser;
use tantivy::schema::{
    Field, IndexRecordOption, NumericOptions, Schema, TEXT, TextFieldIndexing, TextOptions, Value,
};
use tantivy::{Index, IndexWriter, ReloadPolicy, TantivyDocument};

use crate::entry::SessionEntry;
use crate::meta::SessionMeta;
use crate::search::{self, SessionHit, SessionSearchOptions, snippet_from_text};

/// Size of the indexer heap in bytes.
const INDEXER_HEAP_BYTES: usize = 15_000_000;

/// Sub-directory name inside the sessions directory.
const INDEX_SUBDIR: &str = ".search_index";

/// All tantivy fields used by [`TantivyIndex`].
#[derive(Clone, Copy)]
struct Fields {
    session_id: Field,
    session_title: Field,
    entry_type: Field,
    timestamp: Field,
    body: Field,
    entry_json: Field,
}

impl Fields {
    fn register(builder: &mut tantivy::schema::SchemaBuilder) -> Self {
        // `raw` tokenizer: no tokenization — stored as a single term.
        let raw = TextOptions::default()
            .set_indexing_options(
                TextFieldIndexing::default()
                    .set_tokenizer("raw")
                    .set_index_option(IndexRecordOption::Basic),
            )
            .set_stored();

        let stored_only = TextOptions::default().set_stored();

        Self {
            session_id: builder.add_text_field("session_id", raw.clone()),
            session_title: builder.add_text_field("session_title", stored_only.clone()),
            entry_type: builder.add_text_field("entry_type", raw),
            timestamp: builder.add_u64_field(
                "timestamp",
                NumericOptions::default().set_indexed().set_stored(),
            ),
            body: builder.add_text_field("body", TEXT),
            entry_json: builder.add_text_field("entry_json", stored_only),
        }
    }
}

/// Tantivy-backed full-text index.
///
/// `TantivyIndex` is internally `Arc<Mutex<_>>` so it can be shared across
/// threads and cloned cheaply.
#[derive(Clone)]
pub struct TantivyIndex {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    index: Index,
    fields: Fields,
}

impl TantivyIndex {
    /// Open an existing index or create a new one in `<sessions_dir>/.search_index/`.
    pub fn open_or_create(sessions_dir: &Path) -> io::Result<Self> {
        let index_dir = sessions_dir.join(INDEX_SUBDIR);
        std::fs::create_dir_all(&index_dir)?;

        let (index, fields) = open_or_create_index(&index_dir)?;
        Ok(Self {
            inner: Arc::new(Mutex::new(Inner { index, fields })),
        })
    }

    /// Index (or re-index) one session.
    ///
    /// Deletes any previously stored documents for this session ID, then
    /// inserts one document per `SessionEntry`.
    pub fn index_session(&self, meta: &SessionMeta, entries: &[SessionEntry]) -> io::Result<()> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut writer = inner
            .index
            .writer(INDEXER_HEAP_BYTES)
            .map_err(tantivy_to_io)?;

        delete_session_docs(&mut writer, &inner.fields, &meta.id);

        for entry in entries {
            let body = search::searchable_text_pub(entry);
            if body.is_empty() {
                continue;
            }
            let entry_json = serde_json::to_string(entry).map_err(io::Error::other)?;
            let timestamp = entry.timestamp().unwrap_or(0);

            let doc = tantivy::doc!(
                inner.fields.session_id => meta.id.as_str(),
                inner.fields.session_title => meta.title.as_str(),
                inner.fields.entry_type => entry.entry_type_name(),
                inner.fields.timestamp => timestamp,
                inner.fields.body => body.as_str(),
                inner.fields.entry_json => entry_json.as_str()
            );
            writer.add_document(doc).map_err(tantivy_to_io)?;
        }

        writer.commit().map_err(tantivy_to_io)?;
        Ok(())
    }

    /// Delete every document in the index, leaving it empty.
    ///
    /// Used by `rebuild_search_index` to guarantee replacement semantics when
    /// sessions have been removed outside the store API.
    pub fn clear_all(&self) -> io::Result<()> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut writer: tantivy::IndexWriter<TantivyDocument> = inner
            .index
            .writer(INDEXER_HEAP_BYTES)
            .map_err(tantivy_to_io)?;
        writer.delete_all_documents().map_err(tantivy_to_io)?;
        writer.commit().map_err(tantivy_to_io)?;
        Ok(())
    }

    /// Remove all indexed documents for a session.
    pub fn delete_session(&self, session_id: &str) -> io::Result<()> {
        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let mut writer = inner
            .index
            .writer(INDEXER_HEAP_BYTES)
            .map_err(tantivy_to_io)?;
        delete_session_docs(&mut writer, &inner.fields, session_id);
        writer.commit().map_err(tantivy_to_io)?;
        Ok(())
    }

    /// Search the index and return hits respecting `options`.
    pub fn search(
        &self,
        query: &str,
        options: &SessionSearchOptions,
    ) -> io::Result<Vec<SessionHit>> {
        let limit = options.limit();
        if query.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }

        let inner = self.inner.lock().unwrap_or_else(|e| e.into_inner());
        let reader = inner
            .index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()
            .map_err(tantivy_to_io)?;

        reader.reload().map_err(tantivy_to_io)?;
        let searcher = reader.searcher();

        let query_parser = QueryParser::for_index(&inner.index, vec![inner.fields.body]);
        // Sanitise the query: tantivy's default parser raises on many special chars.
        let sanitised = sanitise_query(query);
        let parsed_query = query_parser
            .parse_query(&sanitised)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e.to_string()))?;

        // Fetch more than `limit` to allow post-filtering.
        let fetch = (limit * 4).max(200);
        let top_docs: Vec<(tantivy::Score, tantivy::DocAddress)> = searcher
            .search(&parsed_query, &TopDocs::with_limit(fetch))
            .map_err(tantivy_to_io)?;

        let mut hits: Vec<SessionHit> = Vec::new();
        for (score, doc_address) in top_docs {
            if hits.len() >= limit {
                break;
            }

            let doc: TantivyDocument = searcher.doc(doc_address).map_err(tantivy_to_io)?;

            let session_id = get_text_field(&doc, inner.fields.session_id);
            let session_title = get_text_field(&doc, inner.fields.session_title);
            let entry_json = get_text_field(&doc, inner.fields.entry_json);
            let entry_type_name = get_text_field(&doc, inner.fields.entry_type);
            let timestamp_val: u64 = doc
                .get_first(inner.fields.timestamp)
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            // Session ID filter.
            if options
                .session_ids
                .as_ref()
                .is_some_and(|ids| !ids.contains(&session_id))
            {
                continue;
            }
            // Entry type filter.
            if options
                .entry_types
                .as_ref()
                .is_some_and(|types| !types.contains(&entry_type_name))
            {
                continue;
            }
            // Timestamp range filter.
            if options
                .start_time
                .is_some_and(|start| timestamp_val < start.timestamp().cast_unsigned())
            {
                continue;
            }
            if options
                .end_time
                .is_some_and(|end| timestamp_val > end.timestamp().cast_unsigned())
            {
                continue;
            }

            // Reconstruct `SessionEntry` from stored JSON.
            let entry: SessionEntry = match serde_json::from_str(&entry_json) {
                Ok(e) => e,
                Err(err) => {
                    tracing::warn!(
                        session_id = %session_id,
                        error = %err,
                        "skipping tantivy hit with undeserializable entry_json"
                    );
                    continue;
                }
            };

            let body = search::searchable_text_pub(&entry);
            let snippet = snippet_from_text(&body, 0);

            hits.push(SessionHit {
                session_id,
                session_title,
                entry,
                // Convert tantivy's f32 score to a comparable usize (×1000 for ordering).
                // score is always non-negative (BM25 score from tantivy).
                #[allow(clippy::cast_sign_loss)]
                score: (score.max(0.0) * 1000.0) as usize,
                snippet,
            });
        }

        Ok(hits)
    }
}

// ─── helpers ────────────────────────────────────────────────────────────────

fn open_or_create_index(index_dir: &Path) -> io::Result<(Index, Fields)> {
    let mut schema_builder = Schema::builder();
    let fields = Fields::register(&mut schema_builder);
    let schema = schema_builder.build();

    let dir = tantivy::directory::MmapDirectory::open(index_dir).map_err(tantivy_to_io)?;
    let index = Index::open_or_create(dir, schema).map_err(tantivy_to_io)?;
    Ok((index, fields))
}

#[allow(clippy::needless_pass_by_ref_mut)]
fn delete_session_docs(writer: &mut IndexWriter, fields: &Fields, session_id: &str) {
    let term = tantivy::Term::from_field_text(fields.session_id, session_id);
    writer.delete_term(term);
}

fn get_text_field(doc: &TantivyDocument, field: Field) -> String {
    doc.get_first(field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string()
}

/// Remove characters that confuse tantivy's default query parser when users
/// supply arbitrary search queries (e.g. colons, slashes, brackets).
fn sanitise_query(query: &str) -> String {
    query
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect()
}

fn tantivy_to_io(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}
