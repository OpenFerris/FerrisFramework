//! Room management for realtime subscriptions

use ferris_core::{Error, Result};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use uuid::Uuid;

/// Room types
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomType {
    User,
    Doctype,
    List,
    Custom,
}

/// Room information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Room {
    pub id: String,
    pub name: String,
    pub room_type: RoomType,
    pub members: HashSet<String>, // connection IDs
    pub metadata: HashMap<String, String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_activity: chrono::DateTime<chrono::Utc>,
}

impl Room {
    pub fn new(name: String, room_type: RoomType) -> Self {
        let now = chrono::Utc::now();
        Self {
            id: Uuid::new_v4().to_string(),
            name,
            room_type,
            members: HashSet::new(),
            metadata: HashMap::new(),
            created_at: now,
            last_activity: now,
        }
    }

    pub fn user_room(user_id: &str) -> Self {
        Self::new(format!("user:{}", user_id), RoomType::User)
    }

    pub fn doctype_room(doctype: &str) -> Self {
        Self::new(format!("doctype:{}", doctype), RoomType::Doctype)
    }

    pub fn list_room(doctype: &str, filters_hash: &str) -> Self {
        Self::new(format!("list:{}:{}", doctype, filters_hash), RoomType::List)
    }

    pub fn add_member(&mut self, connection_id: String) {
        self.members.insert(connection_id);
        self.last_activity = chrono::Utc::now();
    }

    pub fn remove_member(&mut self, connection_id: &str) -> bool {
        let removed = self.members.remove(connection_id);
        self.last_activity = chrono::Utc::now();
        removed
    }

    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    pub fn has_member(&self, connection_id: &str) -> bool {
        self.members.contains(connection_id)
    }
}

/// Room manager
pub struct RoomManager {
    rooms: Arc<RwLock<HashMap<String, Room>>>,
    connection_rooms: Arc<RwLock<HashMap<String, HashSet<String>>>>, // connection_id -> room_names
}

impl RoomManager {
    pub fn new() -> Self {
        Self {
            rooms: Arc::new(RwLock::new(HashMap::new())),
            connection_rooms: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Get a room by name
    pub fn get(&self, name: &str) -> Option<Room> {
        self.rooms.read().get(name).cloned()
    }

    /// Join a room
    pub fn join(&self, room_name: &str, connection_id: &str) -> Result<()> {
        let mut rooms = self.rooms.write();
        let room = rooms
            .entry(room_name.to_string())
            .or_insert_with(|| Room::new(room_name.to_string(), RoomType::Custom));
        room.add_member(connection_id.to_string());

        // Track connection's rooms
        let mut conn_rooms = self.connection_rooms.write();
        conn_rooms
            .entry(connection_id.to_string())
            .or_default()
            .insert(room_name.to_string());

        Ok(())
    }

    /// Whether `user_id` is allowed to join `room_name`.
    ///
    /// Rooms are deny-by-default. `user:<id>` is restricted to its owner;
    /// `doctype:`/`list:` rooms require the supplied `doctype_access` predicate
    /// (typically "user can read this doctype"); anything else is rejected.
    pub fn can_join(
        &self,
        room_name: &str,
        user_id: &str,
        doctype_access: impl Fn(&str) -> bool,
    ) -> bool {
        if let Some(owner) = room_name.strip_prefix("user:") {
            owner == user_id
        } else if let Some(doctype) = room_name.strip_prefix("doctype:") {
            doctype_access(doctype)
        } else if let Some(rest) = room_name.strip_prefix("list:") {
            doctype_access(rest.split(':').next().unwrap_or(""))
        } else if room_name.starts_with("comments:") {
            // Discussion comments are public to read.
            true
        } else {
            false
        }
    }

    /// Join a room after enforcing [`RoomManager::can_join`].
    pub fn join_authorized(
        &self,
        room_name: &str,
        connection_id: &str,
        user_id: &str,
        doctype_access: impl Fn(&str) -> bool,
    ) -> Result<()> {
        if !self.can_join(room_name, user_id, doctype_access) {
            return Err(Error::Authz(ferris_core::AuthzError::PermissionDenied {
                action: "join".to_string(),
                resource: room_name.to_string(),
            }));
        }
        self.join(room_name, connection_id)
    }

    /// Leave a room
    pub fn leave(&self, room_name: &str, connection_id: &str) -> Result<bool> {
        let mut rooms = self.rooms.write();
        let removed = if let Some(room) = rooms.get_mut(room_name) {
            room.remove_member(connection_id)
        } else {
            false
        };

        // Clean up empty rooms
        if removed && rooms.get(room_name).is_some_and(Room::is_empty) {
            rooms.remove(room_name);
        }

        // Update connection's rooms
        let mut conn_rooms = self.connection_rooms.write();
        if let Some(user_rooms) = conn_rooms.get_mut(connection_id) {
            user_rooms.remove(room_name);
        }

        Ok(removed)
    }

    /// Leave all rooms for a connection
    pub fn leave_all(&self, connection_id: &str) -> Result<Vec<String>> {
        let room_names: Vec<String> = {
            let mut conn_rooms = self.connection_rooms.write();
            conn_rooms
                .remove(connection_id)
                .unwrap_or_default()
                .into_iter()
                .collect()
        };

        let mut rooms = self.rooms.write();
        let mut left_rooms = Vec::new();

        for room_name in room_names {
            if let Some(room) = rooms.get_mut(&room_name) {
                room.remove_member(connection_id);
                left_rooms.push(room_name.clone());

                if room.is_empty() {
                    rooms.remove(&room_name);
                }
            }
        }

        Ok(left_rooms)
    }

    /// Get members of a room
    pub fn get_members(&self, room_name: &str) -> Vec<String> {
        self.rooms
            .read()
            .get(room_name)
            .map(|r| r.members.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get rooms for a connection
    pub fn get_connection_rooms(&self, connection_id: &str) -> Vec<String> {
        self.connection_rooms
            .read()
            .get(connection_id)
            .map(|r| r.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Get room stats
    pub fn get_stats(&self) -> RoomStats {
        let rooms = self.rooms.read();
        let total_rooms = rooms.len();
        let total_members: usize = rooms.values().map(|r| r.members.len()).sum();

        RoomStats {
            total_rooms,
            total_members,
            room_types: rooms
                .values()
                .fold(std::collections::HashMap::new(), |mut acc, room| {
                    *acc.entry(format!("{:?}", room.room_type)).or_insert(0) += 1;
                    acc
                }),
        }
    }

    /// Clean up stale rooms
    pub fn cleanup_stale_rooms(&self, max_age: chrono::Duration) -> usize {
        let mut rooms = self.rooms.write();
        let now = chrono::Utc::now();
        let stale_names: Vec<String> = rooms
            .iter()
            .filter(|(_, room)| now - room.last_activity > max_age && room.is_empty())
            .map(|(name, _)| name.clone())
            .collect();

        let count = stale_names.len();
        for name in stale_names {
            rooms.remove(&name);
        }
        count
    }
}

impl Default for RoomManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Room statistics
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoomStats {
    pub total_rooms: usize,
    pub total_members: usize,
    pub room_types: HashMap<String, usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_room_creation() {
        let room = Room::user_room("user1");
        assert_eq!(room.name, "user:user1");
        assert_eq!(room.room_type, RoomType::User);
    }

    #[test]
    fn test_room_manager() {
        let manager = RoomManager::new();

        // Join room
        manager.join("user:user1", "conn1").unwrap();

        // Check membership
        let members = manager.get_members("user:user1");
        assert_eq!(members.len(), 1);
        assert!(members.contains(&"conn1".to_string()));

        // Check connection rooms
        let conn_rooms = manager.get_connection_rooms("conn1");
        assert_eq!(conn_rooms.len(), 1);

        // Leave room
        manager.leave("user:user1", "conn1").unwrap();
        let members = manager.get_members("user:user1");
        assert_eq!(members.len(), 0);
    }

    #[test]
    fn test_leave_all() {
        let manager = RoomManager::new();

        manager.join("room1", "conn1").unwrap();
        manager.join("room2", "conn1").unwrap();
        manager.join("room3", "conn1").unwrap();

        let left = manager.leave_all("conn1").unwrap();
        assert_eq!(left.len(), 3);

        let conn_rooms = manager.get_connection_rooms("conn1");
        assert_eq!(conn_rooms.len(), 0);
    }
}
