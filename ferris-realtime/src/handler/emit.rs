//! Delivery and the document/list/user notification helpers.

use super::RealtimeHandler;
use crate::{EventPriority, EventTarget, RealtimeEvent, events::EventType};
use ferris_core::{Error, RealtimeError, Result, Value};
use tracing::debug;

impl RealtimeHandler {
    pub fn broadcast(&self, event: &RealtimeEvent) -> Result<()> {
        debug!("Broadcasting event: {:?}", event.event_type);
        let Some(io) = &self.socket_io else {
            return Ok(());
        };
        let data = Self::encode(event)?;
        io.emit("realtime", data).map_err(Self::delivery_error)
    }

    /// Send event to specific connection
    pub fn send_to_connection(&self, connection_id: &str, event: &RealtimeEvent) -> Result<()> {
        // Socket.IO puts every socket in a room named by its own id, so
        // `io.to(<socket id>)` reaches exactly that connection (V3-RT-2).
        let Some(io) = &self.socket_io else {
            return Ok(());
        };
        let data = Self::encode(event)?;
        io.to(connection_id.to_string())
            .emit("realtime", data)
            .map_err(Self::delivery_error)
    }

    /// Send event to room
    pub fn send_to_room(&self, room_name: &str, event: &RealtimeEvent) -> Result<()> {
        // Deliver over Socket.IO when the server wired the instance in. The
        // Socket.IO room registry is the sole delivery path: the in-memory
        // `RoomManager` mirrors membership for stats/authorization, but emitting
        // from both would double-deliver to every member (V3-RT-2).
        let Some(io) = &self.socket_io else {
            return Ok(());
        };
        let data = Self::encode(event)?;
        io.to(room_name.to_string())
            .emit("realtime", data)
            .map_err(Self::delivery_error)
    }

    /// Encode an event, reporting a failure instead of dropping the packet
    /// silently (RT9).
    fn encode(event: &RealtimeEvent) -> Result<serde_json::Value> {
        serde_json::to_value(event)
            .map_err(|e| Error::Realtime(RealtimeError::Serialization(e.to_string())))
    }

    fn delivery_error(error: socketioxide::BroadcastError) -> Error {
        Error::Realtime(RealtimeError::Delivery(error.to_string()))
    }

    /// Broadcast a discussion comment posted on a website route.
    pub fn notify_comment(&self, route: &str, data: crate::events::CommentEventData) -> Result<()> {
        let event = RealtimeEvent::comment(route, data)?;
        self.send_to_room(&format!("comments:{route}"), &event)
    }

    /// Send event to user
    pub fn send_to_user(&self, user_id: &str, event: &RealtimeEvent) -> Result<()> {
        let room_name = format!("user:{}", user_id);
        self.send_to_room(&room_name, event)
    }

    /// Send event to doctype room
    pub fn send_to_doctype(&self, doctype: &str, event: &RealtimeEvent) -> Result<()> {
        let room_name = format!("doctype:{}", doctype);
        self.send_to_room(&room_name, event)
    }

    /// Send event to list room
    pub fn send_to_list(
        &self,
        doctype: &str,
        filters_hash: &str,
        event: &RealtimeEvent,
    ) -> Result<()> {
        let room_name = format!("list:{}:{}", doctype, filters_hash);
        self.send_to_room(&room_name, event)
    }
}

/// Helper functions for common events
impl RealtimeHandler {
    /// Notify document insert
    pub fn notify_doc_insert(&self, doctype: &str, docname: &str, data: Value) -> Result<()> {
        let payload = Value::try_from(crate::events::DocEventData {
            doctype: doctype.to_string(),
            docname: docname.to_string(),
            data,
            changes: None,
            previous_data: None,
        })?;
        let event = RealtimeEvent {
            event_type: EventType::DocInsert,
            payload: crate::events::EventPayload::new(EventType::DocInsert, payload)
                .with_doctype(doctype.to_string())
                .with_docname(docname.to_string()),
            target: EventTarget::Doctype(doctype.to_string()),
            priority: EventPriority::Normal,
        };

        self.send_to_doctype(doctype, &event)
    }

    /// Notify document update
    pub fn notify_doc_update(
        &self,
        doctype: &str,
        docname: &str,
        data: Value,
        changes: Value,
        previous: Value,
    ) -> Result<()> {
        let payload = Value::try_from(crate::events::DocEventData {
            doctype: doctype.to_string(),
            docname: docname.to_string(),
            data,
            changes: Some(changes),
            previous_data: Some(previous),
        })?;
        let event = RealtimeEvent {
            event_type: EventType::DocUpdate,
            payload: crate::events::EventPayload::new(EventType::DocUpdate, payload)
                .with_doctype(doctype.to_string())
                .with_docname(docname.to_string()),
            target: EventTarget::Doctype(doctype.to_string()),
            priority: EventPriority::Normal,
        };

        self.send_to_doctype(doctype, &event)
    }

    /// Notify document delete
    pub fn notify_doc_delete(&self, doctype: &str, docname: &str, previous: Value) -> Result<()> {
        let payload = Value::try_from(crate::events::DocEventData {
            doctype: doctype.to_string(),
            docname: docname.to_string(),
            data: Value::Null,
            changes: None,
            previous_data: Some(previous),
        })?;
        let event = RealtimeEvent {
            event_type: EventType::DocDelete,
            payload: crate::events::EventPayload::new(EventType::DocDelete, payload)
                .with_doctype(doctype.to_string())
                .with_docname(docname.to_string()),
            target: EventTarget::Doctype(doctype.to_string()),
            priority: EventPriority::High,
        };

        self.send_to_doctype(doctype, &event)
    }

    /// Notify document submit
    pub fn notify_doc_submit(&self, doctype: &str, docname: &str, data: Value) -> Result<()> {
        let payload = Value::try_from(crate::events::DocEventData {
            doctype: doctype.to_string(),
            docname: docname.to_string(),
            data,
            changes: None,
            previous_data: None,
        })?;
        let event = RealtimeEvent {
            event_type: EventType::DocSubmit,
            payload: crate::events::EventPayload::new(EventType::DocSubmit, payload)
                .with_doctype(doctype.to_string())
                .with_docname(docname.to_string()),
            target: EventTarget::Doctype(doctype.to_string()),
            priority: EventPriority::High,
        };

        self.send_to_doctype(doctype, &event)
    }

    /// Notify document cancel
    pub fn notify_doc_cancel(&self, doctype: &str, docname: &str, data: Value) -> Result<()> {
        let payload = Value::try_from(crate::events::DocEventData {
            doctype: doctype.to_string(),
            docname: docname.to_string(),
            data,
            changes: None,
            previous_data: None,
        })?;
        let event = RealtimeEvent {
            event_type: EventType::DocCancel,
            payload: crate::events::EventPayload::new(EventType::DocCancel, payload)
                .with_doctype(doctype.to_string())
                .with_docname(docname.to_string()),
            target: EventTarget::Doctype(doctype.to_string()),
            priority: EventPriority::High,
        };

        self.send_to_doctype(doctype, &event)
    }

    /// Notify list update
    pub fn notify_list_update(
        &self,
        doctype: &str,
        filters_hash: &str,
        changes: crate::events::ListChanges,
    ) -> Result<()> {
        let payload = Value::try_from(crate::events::ListEventData {
            doctype: doctype.to_string(),
            filters_hash: filters_hash.to_string(),
            changes,
        })?;
        let event = RealtimeEvent {
            event_type: EventType::ListUpdate,
            payload: crate::events::EventPayload::new(EventType::ListUpdate, payload)
                .with_doctype(doctype.to_string()),
            target: EventTarget::List {
                doctype: doctype.to_string(),
                filters_hash: filters_hash.to_string(),
            },
            priority: EventPriority::Normal,
        };

        self.send_to_list(doctype, filters_hash, &event)
    }

    /// Notify one user about their task's progress.
    ///
    /// Progress belongs to the task owner, so this targets `user:<id>` instead
    /// of the namespace-wide broadcast it used to perform: a `TaskStarted` for
    /// one user's import must not reach every connected (possibly anonymous)
    /// socket (RT3).
    pub fn notify_progress(
        &self,
        user_id: &str,
        task_id: &str,
        progress: f32,
        message: String,
    ) -> Result<()> {
        let payload = Value::try_from(crate::events::ProgressEventData {
            task_id: task_id.to_string(),
            progress,
            message,
            total: None,
            completed: None,
        })?;
        let event = RealtimeEvent {
            event_type: EventType::Progress,
            payload: crate::events::EventPayload::new(EventType::Progress, payload)
                .with_user(user_id.to_string()),
            target: EventTarget::User(user_id.to_string()),
            priority: EventPriority::Normal,
        };

        self.send_to_user(user_id, &event)
    }

    /// Notify user
    pub fn notify_user(
        &self,
        user_id: &str,
        notification: crate::events::NotificationEventData,
    ) -> Result<()> {
        let payload = Value::try_from(notification)?;
        let event = RealtimeEvent {
            event_type: EventType::Notification,
            payload: crate::events::EventPayload::new(EventType::Notification, payload)
                .with_user(user_id.to_string()),
            target: EventTarget::User(user_id.to_string()),
            priority: EventPriority::Normal,
        };

        self.send_to_user(user_id, &event)
    }
}
