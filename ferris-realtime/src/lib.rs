//! Ferris Realtime - WebSocket/Socket.IO server for realtime updates

pub mod events;
pub mod handler;
pub mod presence;
pub mod rooms;
pub mod server;

pub use events::{
    CommentEventData, EventPayload, EventPriority, EventTarget, EventType, RealtimeEvent,
};
pub use handler::{ConnectionInfo, RealtimeHandler};
pub use presence::{Presence, PresenceConnection, PresenceManager, UserStatus};
pub use rooms::{Room, RoomManager, RoomType};
pub use server::{
    RealtimeConfig, RealtimeServer, install_handler, notify_doc_cancel, notify_doc_delete,
    notify_doc_insert, notify_doc_submit, notify_doc_update, start_realtime_server,
};
