//! Realtime events and payloads

use chrono::{DateTime, Utc};
use ferris_core::{Error, RealtimeError, Result, Value};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

/// Realtime event types
///
/// The wire form is always the bare string [`EventType::as_str`] returns, for
/// built-ins **and** for [`EventType::Custom`]. The derived, externally-tagged
/// representation this used to have serialized `Custom("x")` as
/// `{"custom":"x"}` while `as_str`/`From<String>` used the bare `"x"`, so a
/// client could round-trip through one path but not the other (RT6). The
/// manual impls below make `serde` use exactly the same spelling as
/// `as_str`/`From<String>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventType {
    // Document events
    DocInsert,
    DocUpdate,
    DocDelete,
    DocSubmit,
    DocCancel,
    DocAmend,
    DocRename,

    // List events
    ListUpdate,
    ListInsert,
    ListRemove,
    ListReorder,

    // Progress events
    Progress,
    TaskStarted,
    TaskCompleted,
    TaskFailed,

    // Notification events
    Notification,
    Alert,
    Message,

    // Discussion events
    Comment,
    CommentDelete,

    // Presence events
    UserOnline,
    UserOffline,
    UserAway,
    UserBusy,
    UserTyping,
    UserStoppedTyping,

    // System events
    Broadcast,
    Maintenance,
    VersionUpdate,

    // Custom events
    Custom(String),
}

impl EventType {
    pub fn as_str(&self) -> &str {
        match self {
            EventType::DocInsert => "doc_insert",
            EventType::DocUpdate => "doc_update",
            EventType::DocDelete => "doc_delete",
            EventType::DocSubmit => "doc_submit",
            EventType::DocCancel => "doc_cancel",
            EventType::DocAmend => "doc_amend",
            EventType::DocRename => "doc_rename",
            EventType::ListUpdate => "list_update",
            EventType::ListInsert => "list_insert",
            EventType::ListRemove => "list_remove",
            EventType::ListReorder => "list_reorder",
            EventType::Progress => "progress",
            EventType::TaskStarted => "task_started",
            EventType::TaskCompleted => "task_completed",
            EventType::TaskFailed => "task_failed",
            EventType::Notification => "notification",
            EventType::Alert => "alert",
            EventType::Message => "message",
            EventType::Comment => "comment",
            EventType::CommentDelete => "comment_delete",
            EventType::UserOnline => "user_online",
            EventType::UserOffline => "user_offline",
            EventType::UserAway => "user_away",
            EventType::UserBusy => "user_busy",
            EventType::UserTyping => "user_typing",
            EventType::UserStoppedTyping => "user_stopped_typing",
            EventType::Broadcast => "broadcast",
            EventType::Maintenance => "maintenance",
            EventType::VersionUpdate => "version_update",
            EventType::Custom(name) => name,
        }
    }
}

impl Serialize for EventType {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EventType {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        Ok(Self::from(raw))
    }
}

impl From<String> for EventType {
    fn from(s: String) -> Self {
        match s.as_str() {
            "doc_insert" => EventType::DocInsert,
            "doc_update" => EventType::DocUpdate,
            "doc_delete" => EventType::DocDelete,
            "doc_submit" => EventType::DocSubmit,
            "doc_cancel" => EventType::DocCancel,
            "doc_amend" => EventType::DocAmend,
            "doc_rename" => EventType::DocRename,
            "list_update" => EventType::ListUpdate,
            "list_insert" => EventType::ListInsert,
            "list_remove" => EventType::ListRemove,
            "list_reorder" => EventType::ListReorder,
            "progress" => EventType::Progress,
            "task_started" => EventType::TaskStarted,
            "task_completed" => EventType::TaskCompleted,
            "task_failed" => EventType::TaskFailed,
            "notification" => EventType::Notification,
            "alert" => EventType::Alert,
            "message" => EventType::Message,
            "comment" => EventType::Comment,
            "comment_delete" => EventType::CommentDelete,
            "user_online" => EventType::UserOnline,
            "user_offline" => EventType::UserOffline,
            "user_away" => EventType::UserAway,
            "user_busy" => EventType::UserBusy,
            "user_typing" => EventType::UserTyping,
            "user_stopped_typing" => EventType::UserStoppedTyping,
            "broadcast" => EventType::Broadcast,
            "maintenance" => EventType::Maintenance,
            "version_update" => EventType::VersionUpdate,
            _ => EventType::Custom(s),
        }
    }
}

/// Event payload
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventPayload {
    pub event_type: EventType,
    pub doctype: Option<String>,
    pub docname: Option<String>,
    pub data: Value,
    pub user: Option<String>,
    pub room: Option<String>,
    pub metadata: HashMap<String, Value>,
    pub timestamp: DateTime<Utc>,
    pub event_id: String,
}

impl EventPayload {
    pub fn new(event_type: EventType, data: Value) -> Self {
        Self {
            event_type,
            doctype: None,
            docname: None,
            data,
            user: None,
            room: None,
            metadata: HashMap::new(),
            timestamp: Utc::now(),
            event_id: Uuid::new_v4().to_string(),
        }
    }

    pub fn with_doctype(mut self, doctype: String) -> Self {
        self.doctype = Some(doctype);
        self
    }

    pub fn with_docname(mut self, docname: String) -> Self {
        self.docname = Some(docname);
        self
    }

    pub fn with_user(mut self, user: String) -> Self {
        self.user = Some(user);
        self
    }

    pub fn with_room(mut self, room: String) -> Self {
        self.room = Some(room);
        self
    }

    pub fn with_metadata(mut self, key: String, value: Value) -> Self {
        self.metadata.insert(key, value);
        self
    }

    pub fn to_json(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|e| e.into())
    }
}

/// Realtime event (internal representation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RealtimeEvent {
    pub event_type: EventType,
    pub payload: EventPayload,
    pub target: EventTarget,
    pub priority: EventPriority,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventTarget {
    All,
    User(String),
    Room(String),
    Doctype(String),
    List {
        doctype: String,
        filters_hash: String,
    },
    Custom(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventPriority {
    Low,
    Normal,
    High,
    Critical,
}

impl RealtimeEvent {
    pub fn new(event_type: EventType, payload: EventPayload, target: EventTarget) -> Self {
        Self {
            event_type,
            payload,
            target,
            priority: EventPriority::Normal,
        }
    }

    pub fn with_priority(mut self, priority: EventPriority) -> Self {
        self.priority = priority;
        self
    }

    /// Event for a comment posted on a website route, targeting
    /// `comments:<route>`.
    ///
    /// # Errors
    ///
    /// Propagates an encoding failure instead of emitting a `null` payload.
    pub fn comment(route: &str, data: CommentEventData) -> Result<Self> {
        let room = format!("comments:{route}");
        let payload =
            EventPayload::new(EventType::Comment, data.try_into()?).with_room(room.clone());
        Ok(Self::new(
            EventType::Comment,
            payload,
            EventTarget::Room(room),
        ))
    }
}

/// Document event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocEventData {
    pub doctype: String,
    pub docname: String,
    pub data: Value,
    pub changes: Option<Value>,       // For updates
    pub previous_data: Option<Value>, // For updates/deletes
}

impl TryFrom<DocEventData> for Value {
    type Error = Error;

    fn try_from(value: DocEventData) -> Result<Self> {
        event_data_to_value(value)
    }
}

/// Encode event data, surfacing a serialization failure instead of turning it
/// into a `null` payload (RT9).
fn event_data_to_value<T: Serialize>(value: T) -> Result<Value> {
    serde_json::to_value(value)
        .map(Value::from_json)
        .map_err(|e| Error::Realtime(RealtimeError::Serialization(e.to_string())))
}

/// Discussion comment event data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommentEventData {
    pub route: String,
    pub comment_id: String,
    pub parent: Option<String>,
    pub author: String,
    pub body: String,
}

impl TryFrom<CommentEventData> for Value {
    type Error = Error;

    fn try_from(value: CommentEventData) -> Result<Self> {
        event_data_to_value(value)
    }
}

/// List event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListEventData {
    pub doctype: String,
    pub filters_hash: String,
    pub changes: ListChanges,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListChanges {
    pub inserted: Vec<Value>,
    pub updated: Vec<Value>,
    pub removed: Vec<String>,
    pub reordered: Option<Vec<String>>,
}

impl TryFrom<ListEventData> for Value {
    type Error = Error;

    fn try_from(value: ListEventData) -> Result<Self> {
        event_data_to_value(value)
    }
}

/// Progress event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProgressEventData {
    pub task_id: String,
    pub progress: f32, // 0.0 to 1.0
    pub message: String,
    pub total: Option<u64>,
    pub completed: Option<u64>,
}

impl TryFrom<ProgressEventData> for Value {
    type Error = Error;

    fn try_from(value: ProgressEventData) -> Result<Self> {
        event_data_to_value(value)
    }
}

/// Notification event data
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationEventData {
    pub notification_type: String,
    pub title: String,
    pub message: String,
    pub priority: NotificationPriority,
    pub action_url: Option<String>,
    pub action_label: Option<String>,
    pub icon: Option<String>,
    pub sound: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationPriority {
    Low,
    Normal,
    High,
    Urgent,
}

impl TryFrom<NotificationEventData> for Value {
    type Error = Error;

    fn try_from(value: NotificationEventData) -> Result<Self> {
        event_data_to_value(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_creation() {
        let event = EventPayload::new(EventType::DocInsert, Value::from("test"));
        assert_eq!(event.event_type, EventType::DocInsert);
    }

    #[test]
    fn test_comment_event() {
        assert_eq!(EventType::Comment.as_str(), "comment");
        assert_eq!(EventType::from("comment".to_string()), EventType::Comment);

        let data = CommentEventData {
            route: "/about".to_string(),
            comment_id: "c1".to_string(),
            parent: None,
            author: "user@example.com".to_string(),
            body: "hello".to_string(),
        };
        let event = RealtimeEvent::comment("/about", data).expect("comment event encodes");
        assert_eq!(event.event_type, EventType::Comment);
        match event.target {
            EventTarget::Room(room) => assert_eq!(room, "comments:/about"),
            other => panic!("expected a room target, got {other:?}"),
        }
    }

    #[test]
    fn test_event_type_conversion() {
        let event_type: EventType = "doc_update".to_string().into();
        assert_eq!(event_type, EventType::DocUpdate);

        let custom = EventType::from("custom_event".to_string());
        assert_eq!(custom.as_str(), "custom_event");
    }

    /// RT6: the JSON form must be the same bare string `as_str` returns, for a
    /// unit variant and for `Custom`. The derived representation used to emit
    /// `{"custom":"plugin_event"}` while `as_str` said `"plugin_event"`.
    #[test]
    fn event_type_serializes_as_its_str_form() {
        let builtin = serde_json::to_value(EventType::DocUpdate).expect("encode builtin");
        assert_eq!(builtin, serde_json::json!("doc_update"));

        let custom = EventType::Custom("plugin_event".to_string());
        let wire = serde_json::to_value(&custom).expect("encode custom");
        assert_eq!(wire, serde_json::json!("plugin_event"));

        let back: EventType = serde_json::from_value(wire).expect("decode custom");
        assert_eq!(back, custom);
    }

    /// RT9: encoding failure is reported, not silently turned into `null`.
    /// `serde_json` cannot fail for these structs today, so the test asserts
    /// the success path stays a real object (the failure branch is exercised by
    /// construction in `event_data_to_value`).
    #[test]
    fn event_data_encodes_to_an_object() {
        let value: Value = DocEventData {
            doctype: "ToDo".to_string(),
            docname: "TODO-1".to_string(),
            data: Value::Null,
            changes: None,
            previous_data: None,
        }
        .try_into()
        .expect("encodes");
        assert!(matches!(value, Value::Object(_)));
    }

    #[test]
    fn test_event_payload_builder() {
        let event = EventPayload::new(EventType::DocInsert, Value::from("data"))
            .with_doctype("Sales Order".to_string())
            .with_docname("SO-001".to_string())
            .with_user("admin".to_string())
            .with_metadata("source".to_string(), Value::from("api"));

        assert_eq!(event.doctype, Some("Sales Order".to_string()));
        assert_eq!(event.docname, Some("SO-001".to_string()));
        assert_eq!(event.user, Some("admin".to_string()));
    }
}
