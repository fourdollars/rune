use serde::{Deserialize, Serialize};

/// Root payload sent by LINE Webhook.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct WebhookPayload {
    /// Bot's user ID for which the webhook event was sent.
    #[serde(default)]
    pub destination: Option<String>,
    /// Array of webhook event objects.
    #[serde(default)]
    pub events: Vec<WebhookEvent>,
}

/// A single webhook event from LINE.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct WebhookEvent {
    /// Event type (e.g. "message", "follow", "unfollow", "join", "leave", "postback").
    #[serde(rename = "type")]
    pub event_type: String,
    /// Channel state mode ("active" or "standby").
    #[serde(default)]
    pub mode: Option<String>,
    /// Time of the event in milliseconds (Unix timestamp).
    #[serde(default)]
    pub timestamp: i64,
    /// Source of the event (user, group, or room).
    #[serde(default)]
    pub source: Option<EventSource>,
    /// Reply token used to reply to this event.
    #[serde(rename = "replyToken", default)]
    pub reply_token: Option<String>,
    /// Message object if event_type is "message".
    #[serde(default)]
    pub message: Option<EventMessage>,
    /// Delivery context info (e.g. redelivery).
    #[serde(rename = "deliveryContext", default)]
    pub delivery_context: Option<DeliveryContext>,
}

/// Source of an event.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EventSource {
    /// Source type: "user", "group", or "room".
    #[serde(rename = "type")]
    pub source_type: String,
    /// User ID of the sender if source is user or group/room with sender info.
    #[serde(rename = "userId", default)]
    pub user_id: Option<String>,
    /// Group ID if sent from a group chat.
    #[serde(rename = "groupId", default)]
    pub group_id: Option<String>,
    /// Room ID if sent from a multi-person chat room.
    #[serde(rename = "roomId", default)]
    pub room_id: Option<String>,
}

impl EventSource {
    /// Returns the most specific ID available (user_id > group_id > room_id).
    pub fn id(&self) -> Option<&str> {
        self.user_id
            .as_deref()
            .or(self.group_id.as_deref())
            .or(self.room_id.as_deref())
    }
}

/// Content of a message event.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct EventMessage {
    /// Message ID.
    #[serde(default)]
    pub id: String,
    /// Message type: "text", "image", "video", "audio", "file", "location", "sticker".
    #[serde(rename = "type")]
    pub message_type: String,
    /// Message text if message_type is "text".
    #[serde(default)]
    pub text: Option<String>,
    /// Quote token for replying to this specific message.
    #[serde(rename = "quoteToken", default)]
    pub quote_token: Option<String>,
}

/// Delivery context metadata.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct DeliveryContext {
    /// Whether this is a redelivered webhook event.
    #[serde(rename = "isRedelivery", default)]
    pub is_redelivery: bool,
}

/// Outgoing message sent via LINE Messaging API.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct LineMessage {
    /// Message type (e.g. "text").
    #[serde(rename = "type")]
    pub message_type: String,
    /// Text content.
    pub text: String,
}

impl LineMessage {
    /// Create a plain text message.
    pub fn text(content: impl Into<String>) -> Self {
        Self {
            message_type: "text".to_string(),
            text: content.into(),
        }
    }
}

/// Request body for `POST /v2/bot/message/reply`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ReplyMessageReq {
    /// Reply token from webhook event.
    #[serde(rename = "replyToken")]
    pub reply_token: String,
    /// Array of messages to send (max 5).
    pub messages: Vec<LineMessage>,
    /// Optional: disable push notification.
    #[serde(
        rename = "notificationDisabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub notification_disabled: Option<bool>,
}

/// Request body for `POST /v2/bot/message/push`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct PushMessageReq {
    /// Recipient user ID, group ID, or room ID.
    pub to: String,
    /// Array of messages to send (max 5).
    pub messages: Vec<LineMessage>,
    /// Optional: disable push notification.
    #[serde(
        rename = "notificationDisabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub notification_disabled: Option<bool>,
}

/// Request body for `POST /v2/bot/chat/loading/start`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct LoadingStartReq {
    /// Chat ID (user_id).
    #[serde(rename = "chatId")]
    pub chat_id: String,
    /// The number of seconds the loading animation is displayed (default 20, max 60).
    #[serde(rename = "loadingSeconds", skip_serializing_if = "Option::is_none")]
    pub loading_seconds: Option<u32>,
}

/// User profile returned by `GET /v2/bot/profile/{userId}`.
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct LineProfile {
    #[serde(rename = "userId")]
    pub user_id: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    #[serde(rename = "pictureUrl", default)]
    pub picture_url: Option<String>,
    #[serde(rename = "statusMessage", default)]
    pub status_message: Option<String>,
}
