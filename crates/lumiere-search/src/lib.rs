use chrono::{DateTime, Utc};
use lumiere_models::config::MeilisearchConfig;
use lumiere_models::snowflake::Snowflake;
use meilisearch_sdk::client::Client;
use meilisearch_sdk::indexes::Index;
use meilisearch_sdk::search::SearchQuery;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use tracing::{info, instrument, warn};

// ---------------------------------------------------------------------------
// Error
// ---------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum SearchError {
    #[error("meilisearch error: {0}")]
    Meilisearch(#[from] meilisearch_sdk::errors::Error),

    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("index configuration failed: {0}")]
    IndexConfig(String),
}

pub type Result<T> = std::result::Result<T, SearchError>;

// ---------------------------------------------------------------------------
// Document
// ---------------------------------------------------------------------------

/// Document stored in the Meilisearch `messages` index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageDocument {
    /// Primary key — Snowflake ID as string.
    pub id: String,
    pub channel_id: String,
    pub server_id: String,
    pub author_id: String,
    pub content: String,
    /// Unix timestamp in seconds (for range filtering).
    pub timestamp: i64,
}

impl MessageDocument {
    pub fn new(
        id: Snowflake,
        channel_id: Snowflake,
        server_id: Snowflake,
        author_id: Snowflake,
        content: String,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            id: id.to_string(),
            channel_id: channel_id.to_string(),
            server_id: server_id.to_string(),
            author_id: author_id.to_string(),
            content,
            timestamp: created_at.timestamp(),
        }
    }
}

// ---------------------------------------------------------------------------
// Search parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
pub struct SearchParams {
    pub query: String,
    pub channel_id: Option<Snowflake>,
    pub server_id: Option<Snowflake>,
    pub author_id: Option<Snowflake>,
    pub before: Option<DateTime<Utc>>,
    pub after: Option<DateTime<Utc>>,
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

impl SearchParams {
    pub fn new(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Default::default()
        }
    }

    pub fn channel_id(mut self, id: Snowflake) -> Self {
        self.channel_id = Some(id);
        self
    }

    pub fn server_id(mut self, id: Snowflake) -> Self {
        self.server_id = Some(id);
        self
    }

    pub fn author_id(mut self, id: Snowflake) -> Self {
        self.author_id = Some(id);
        self
    }

    pub fn before(mut self, dt: DateTime<Utc>) -> Self {
        self.before = Some(dt);
        self
    }

    pub fn after(mut self, dt: DateTime<Utc>) -> Self {
        self.after = Some(dt);
        self
    }

    pub fn limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    pub fn offset(mut self, offset: usize) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Build the Meilisearch filter string from the parameters.
    fn build_filter(&self) -> Option<String> {
        let mut filters: Vec<String> = Vec::new();

        if let Some(ref id) = self.channel_id {
            filters.push(format!("channel_id = \"{}\"", id));
        }
        if let Some(ref id) = self.server_id {
            filters.push(format!("server_id = \"{}\"", id));
        }
        if let Some(ref id) = self.author_id {
            filters.push(format!("author_id = \"{}\"", id));
        }
        if let Some(ref dt) = self.after {
            filters.push(format!("timestamp >= {}", dt.timestamp()));
        }
        if let Some(ref dt) = self.before {
            filters.push(format!("timestamp <= {}", dt.timestamp()));
        }

        if filters.is_empty() {
            None
        } else {
            Some(filters.join(" AND "))
        }
    }
}

// ---------------------------------------------------------------------------
// Search result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub hits: Vec<MessageDocument>,
    pub estimated_total: Option<usize>,
    pub processing_time_ms: usize,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

const MESSAGES_INDEX: &str = "messages";
const DEFAULT_SEARCH_LIMIT: usize = 25;

const FILTERABLE_ATTRIBUTES: &[&str] = &["channel_id", "server_id", "author_id", "timestamp"];

const SORTABLE_ATTRIBUTES: &[&str] = &["timestamp"];

const SEARCHABLE_ATTRIBUTES: &[&str] = &["content"];

// ---------------------------------------------------------------------------
// SearchService
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SearchService {
    client: Client,
}

impl SearchService {
    /// Create a new SearchService and configure the messages index.
    #[instrument(skip_all, fields(url = %config.url))]
    pub async fn connect(config: &MeilisearchConfig) -> Result<Self> {
        let client = Client::new(&config.url, Some(&config.api_key)).map_err(|e| {
            SearchError::IndexConfig(format!("failed to create meilisearch client: {}", e))
        })?;

        let service = Self { client };
        service.configure_index().await?;

        info!("connected to meilisearch and configured messages index");
        Ok(service)
    }

    /// Ensure the messages index exists with the correct settings.
    async fn configure_index(&self) -> Result<()> {
        let timeout = Some(Duration::from_secs(30));
        let interval = Some(Duration::from_millis(200));

        // Create the index if it doesn't exist.
        // We check first to avoid a 30s wait_for_completion on an "already exists" failure.
        let index_exists = self.client.get_index(MESSAGES_INDEX).await.is_ok();

        if !index_exists {
            let task = self.client.create_index(MESSAGES_INDEX, Some("id")).await?;
            task.wait_for_completion(&self.client, timeout, interval)
                .await?;
        }

        let index = self.client.index(MESSAGES_INDEX);

        // Set filterable attributes.
        let task = index
            .set_filterable_attributes(FILTERABLE_ATTRIBUTES)
            .await?;
        task.wait_for_completion(&self.client, timeout, interval)
            .await?;

        // Set sortable attributes.
        let task = index.set_sortable_attributes(SORTABLE_ATTRIBUTES).await?;
        task.wait_for_completion(&self.client, timeout, interval)
            .await?;

        // Set searchable attributes.
        let task = index
            .set_searchable_attributes(SEARCHABLE_ATTRIBUTES)
            .await?;
        task.wait_for_completion(&self.client, timeout, interval)
            .await?;

        info!("messages index configured");
        Ok(())
    }

    /// Get a handle to the messages index.
    fn messages_index(&self) -> Index {
        self.client.index(MESSAGES_INDEX)
    }

    // -----------------------------------------------------------------------
    // Index operations
    // -----------------------------------------------------------------------

    /// Index a single message. Fire-and-forget: Meilisearch processes asynchronously.
    #[instrument(skip(self, doc), fields(message_id = %doc.id))]
    pub async fn index_message(&self, doc: &MessageDocument) -> Result<()> {
        self.messages_index()
            .add_documents(&[doc], Some("id"))
            .await?;

        Ok(())
    }

    /// Index a batch of messages. More efficient than individual calls.
    /// Fire-and-forget: Meilisearch processes asynchronously.
    #[instrument(skip(self, docs), fields(count = docs.len()))]
    pub async fn index_messages(&self, docs: &[MessageDocument]) -> Result<()> {
        if docs.is_empty() {
            return Ok(());
        }

        self.messages_index()
            .add_documents(docs, Some("id"))
            .await?;

        info!(count = docs.len(), "batch indexed messages");
        Ok(())
    }

    /// Delete a message from the index. Fire-and-forget.
    #[instrument(skip(self))]
    pub async fn delete_message(&self, message_id: Snowflake) -> Result<()> {
        self.messages_index()
            .delete_document(&message_id.to_string())
            .await?;

        Ok(())
    }

    /// Delete all messages in a channel from the index.
    #[allow(clippy::needless_borrow)]
    #[instrument(skip(self))]
    pub async fn delete_channel_messages(&self, channel_id: Snowflake) -> Result<()> {
        let filter = format!("channel_id = \"{}\"", channel_id);
        let task = self
            .messages_index()
            .delete_documents_with(
                &meilisearch_sdk::documents::DocumentDeletionQuery::new(&self.messages_index())
                    .with_filter(&filter),
            )
            .await?;

        let result = task.wait_for_completion(&self.client, None, None).await?;
        if result.is_failure() {
            warn!(%channel_id, "failed to delete channel messages from index");
        }

        Ok(())
    }

    // -----------------------------------------------------------------------
    // Search
    // -----------------------------------------------------------------------

    /// Search messages with full filtering support.
    #[instrument(skip(self), fields(query = %params.query))]
    pub async fn search(&self, params: &SearchParams) -> Result<SearchResult> {
        if params.query.trim().is_empty() {
            return Ok(SearchResult {
                hits: Vec::new(),
                estimated_total: Some(0),
                processing_time_ms: 0,
            });
        }

        let index = self.messages_index();
        let limit = params.limit.unwrap_or(DEFAULT_SEARCH_LIMIT).min(100);
        let offset = params.offset.unwrap_or(0);

        let mut search = SearchQuery::new(&index);
        search.with_query(&params.query);
        search.with_limit(limit);
        search.with_offset(offset);
        search.with_sort(&["timestamp:desc"]);

        let filter = params.build_filter();
        if let Some(ref f) = filter {
            search.with_filter(f);
        }

        let results = search.execute::<MessageDocument>().await?;

        Ok(SearchResult {
            hits: results.hits.into_iter().map(|h| h.result).collect(),
            estimated_total: Some(results.estimated_total_hits.unwrap_or(0)),
            processing_time_ms: results.processing_time_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_params_filter_empty() {
        let params = SearchParams::new("hello");
        assert!(params.build_filter().is_none());
    }

    #[test]
    fn test_search_params_filter_channel() {
        let params = SearchParams::new("hello").channel_id(Snowflake::new(123));
        let filter = params.build_filter().unwrap();
        assert_eq!(filter, "channel_id = \"123\"");
    }

    #[test]
    fn test_search_params_filter_combined() {
        let params = SearchParams::new("hello")
            .channel_id(Snowflake::new(100))
            .author_id(Snowflake::new(200));
        let filter = params.build_filter().unwrap();
        assert!(filter.contains("channel_id = \"100\""));
        assert!(filter.contains("author_id = \"200\""));
        assert!(filter.contains(" AND "));
    }

    #[test]
    fn test_search_params_filter_time_range() {
        let after = DateTime::parse_from_rfc3339("2025-01-01T00:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let before = DateTime::parse_from_rfc3339("2025-12-31T23:59:59Z")
            .unwrap()
            .with_timezone(&Utc);

        let params = SearchParams::new("test").after(after).before(before);
        let filter = params.build_filter().unwrap();
        assert!(filter.contains("timestamp >= "));
        assert!(filter.contains("timestamp <= "));
    }

    #[test]
    fn test_message_document_new() {
        let now = Utc::now();
        let doc = MessageDocument::new(
            Snowflake::new(1),
            Snowflake::new(2),
            Snowflake::new(3),
            Snowflake::new(4),
            "hello world".to_string(),
            now,
        );
        assert_eq!(doc.id, "1");
        assert_eq!(doc.channel_id, "2");
        assert_eq!(doc.server_id, "3");
        assert_eq!(doc.author_id, "4");
        assert_eq!(doc.content, "hello world");
        assert_eq!(doc.timestamp, now.timestamp());
    }

    #[test]
    fn test_message_document_serialization() {
        let doc = MessageDocument {
            id: "123".to_string(),
            channel_id: "456".to_string(),
            server_id: "789".to_string(),
            author_id: "101".to_string(),
            content: "test message".to_string(),
            timestamp: 1700000000,
        };

        let json = serde_json::to_value(&doc).unwrap();
        assert_eq!(json["id"], "123");
        assert_eq!(json["content"], "test message");
        assert_eq!(json["timestamp"], 1700000000);

        let deserialized: MessageDocument = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.id, doc.id);
        assert_eq!(deserialized.content, doc.content);
    }
}
