use anyhow::Result;
use async_nats::jetstream::consumer::PullConsumer;
use lumiere_models::config::NatsConfig;
use serde::Serialize;

#[derive(Clone)]
pub struct NatsService {
    pub client: async_nats::Client,
    pub jetstream: async_nats::jetstream::Context,
}

impl NatsService {
    pub async fn connect(config: &NatsConfig) -> Result<Self> {
        let client = async_nats::connect(&config.url).await?;
        let jetstream = async_nats::jetstream::new(client.clone());

        tracing::info!("Connected to NATS");
        Ok(Self { client, jetstream })
    }

    pub async fn setup_streams(&self) -> Result<()> {
        // Messages stream — durable, for persistence workers.
        // Uses Limits retention (not WorkQueue) so multiple consumers
        // (search indexer, push worker, read-state updater) can each
        // independently consume every message.
        self.jetstream
            .get_or_create_stream(async_nats::jetstream::stream::Config {
                name: "MESSAGES".to_string(),
                subjects: vec!["persist.messages.>".to_string()],
                retention: async_nats::jetstream::stream::RetentionPolicy::Limits,
                max_age: std::time::Duration::from_secs(86400 * 7),
                storage: async_nats::jetstream::stream::StorageType::File,
                ..Default::default()
            })
            .await?;

        tracing::info!("NATS JetStream streams configured");
        Ok(())
    }

    /// Create or get a durable pull consumer on the given stream.
    ///
    /// Each consumer tracks its own acknowledgment position independently,
    /// so multiple consumers on the same stream each see every message.
    pub async fn create_pull_consumer(
        &self,
        stream_name: &str,
        consumer_name: &str,
        filter_subject: Option<&str>,
    ) -> Result<PullConsumer> {
        let stream = self.jetstream.get_stream(stream_name).await?;

        let config = async_nats::jetstream::consumer::pull::Config {
            durable_name: Some(consumer_name.to_string()),
            ack_policy: async_nats::jetstream::consumer::AckPolicy::Explicit,
            max_deliver: 3,
            ack_wait: std::time::Duration::from_secs(30),
            filter_subject: filter_subject.unwrap_or_default().to_string(),
            ..Default::default()
        };

        let consumer = stream
            .get_or_create_consumer(consumer_name, config)
            .await?;
        Ok(consumer)
    }

    /// Publish to Core NATS (fire-and-forget, instant fanout)
    pub async fn publish<T: Serialize>(&self, subject: &str, payload: &T) -> Result<()> {
        let data = serde_json::to_vec(payload)?;
        self.client
            .publish(subject.to_string(), data.into())
            .await?;
        Ok(())
    }

    /// Publish to JetStream (durable, at-least-once)
    pub async fn publish_durable<T: Serialize>(&self, subject: &str, payload: &T) -> Result<()> {
        let data = serde_json::to_vec(payload)?;
        self.jetstream
            .publish(subject.to_string(), data.into())
            .await?
            .await?;
        Ok(())
    }

    pub async fn check_health(&self) -> bool {
        self.client.flush().await.is_ok()
    }
}
