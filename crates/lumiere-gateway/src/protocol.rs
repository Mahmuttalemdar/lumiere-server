use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Gateway opcodes
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OpCode {
    /// Server → Client: Event dispatch
    Dispatch = 0,
    /// Client → Server: Heartbeat ping
    Heartbeat = 1,
    /// Client → Server: Auth + session start
    Identify = 2,
    /// Client → Server: Update presence
    PresenceUpdate = 3,
    /// Client → Server: Join/leave voice
    VoiceStateUpdate = 4,
    /// Client → Server: Resume disconnected session
    Resume = 6,
    /// Server → Client: Please reconnect
    Reconnect = 7,
    /// Server → Client: Session invalid, re-identify
    InvalidSession = 9,
    /// Server → Client: Sent on connect, contains heartbeat_interval
    Hello = 10,
    /// Server → Client: Heartbeat acknowledged
    HeartbeatAck = 11,
}

impl Serialize for OpCode {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u8(*self as u8)
    }
}

impl<'de> Deserialize<'de> for OpCode {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u8::deserialize(deserializer)?;
        match value {
            0 => Ok(OpCode::Dispatch),
            1 => Ok(OpCode::Heartbeat),
            2 => Ok(OpCode::Identify),
            3 => Ok(OpCode::PresenceUpdate),
            4 => Ok(OpCode::VoiceStateUpdate),
            6 => Ok(OpCode::Resume),
            7 => Ok(OpCode::Reconnect),
            9 => Ok(OpCode::InvalidSession),
            10 => Ok(OpCode::Hello),
            11 => Ok(OpCode::HeartbeatAck),
            _ => Err(serde::de::Error::custom(format!("unknown opcode: {}", value))),
        }
    }
}

/// Gateway message envelope
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayMessage {
    pub op: OpCode,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub d: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub s: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub t: Option<String>,
}

impl GatewayMessage {
    pub fn hello(heartbeat_interval: u64) -> Self {
        Self {
            op: OpCode::Hello,
            d: Some(serde_json::json!({ "heartbeat_interval": heartbeat_interval })),
            s: None,
            t: None,
        }
    }

    pub fn heartbeat_ack() -> Self {
        Self {
            op: OpCode::HeartbeatAck,
            d: None,
            s: None,
            t: None,
        }
    }

    pub fn dispatch(event_name: &str, sequence: u64, data: serde_json::Value) -> Self {
        Self {
            op: OpCode::Dispatch,
            d: Some(data),
            s: Some(sequence),
            t: Some(event_name.to_string()),
        }
    }

    pub fn invalid_session(resumable: bool) -> Self {
        Self {
            op: OpCode::InvalidSession,
            d: Some(serde_json::json!(resumable)),
            s: None,
            t: None,
        }
    }

    pub fn reconnect() -> Self {
        Self {
            op: OpCode::Reconnect,
            d: None,
            s: None,
            t: None,
        }
    }
}

/// Client → Server: Identify payload
#[derive(Debug, Deserialize)]
pub struct IdentifyPayload {
    pub token: String,
    pub properties: Option<ConnectionProperties>,
    pub presence: Option<PresenceUpdatePayload>,
    pub compress: Option<bool>,
    pub large_threshold: Option<u8>,
}

#[derive(Debug, Deserialize)]
pub struct ConnectionProperties {
    pub os: Option<String>,
    pub browser: Option<String>,
    pub device: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PresenceUpdatePayload {
    pub status: Option<String>,
    pub custom_status: Option<serde_json::Value>,
}

/// Client → Server: Resume payload
#[derive(Debug, Deserialize)]
pub struct ResumePayload {
    pub token: String,
    pub session_id: String,
    pub sequence: u64,
}

/// WebSocket close codes
pub mod close_codes {
    pub const UNKNOWN_ERROR: u16 = 4000;
    pub const UNKNOWN_OPCODE: u16 = 4001;
    pub const DECODE_ERROR: u16 = 4002;
    pub const NOT_AUTHENTICATED: u16 = 4003;
    pub const AUTHENTICATION_FAILED: u16 = 4004;
    pub const ALREADY_AUTHENTICATED: u16 = 4005;
    pub const INVALID_SEQUENCE: u16 = 4007;
    pub const RATE_LIMITED: u16 = 4008;
    pub const SESSION_TIMED_OUT: u16 = 4009;
}
