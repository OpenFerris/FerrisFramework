//! Presence management for realtime user status

use chrono::{DateTime, Duration, Utc};
use ferris_core::{Error, Result, ValidationError};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// User status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UserStatus {
    Online,
    Away,
    Busy,
    Offline,
    Invisible,
}

impl UserStatus {
    pub fn as_str(&self) -> &str {
        match self {
            UserStatus::Online => "online",
            UserStatus::Away => "away",
            UserStatus::Busy => "busy",
            UserStatus::Offline => "offline",
            UserStatus::Invisible => "invisible",
        }
    }
}

/// Presence information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Presence {
    pub user_id: String,
    pub user_name: String,
    pub status: UserStatus,
    pub status_message: Option<String>,
    pub last_seen: DateTime<Utc>,
    pub connections: Vec<PresenceConnection>,
    pub metadata: HashMap<String, String>,
}

/// One connection belonging to a presence record.
///
/// Named `PresenceConnection`, not `ConnectionInfo`: `handler::ConnectionInfo`
/// is a different public struct with a different field set, and two public
/// types sharing a name made the resolved type depend on the import path
/// (RT12).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PresenceConnection {
    pub connection_id: String,
    pub ip_address: Option<String>,
    pub user_agent: Option<String>,
    pub device: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub connected_at: DateTime<Utc>,
    pub last_activity: DateTime<Utc>,
    pub rooms: Vec<String>,
}

/// Presence manager
pub struct PresenceManager {
    presences: Arc<RwLock<HashMap<String, Presence>>>,
    connection_to_user: Arc<RwLock<HashMap<String, String>>>, // connection_id -> user_id
    idle_timeout: Duration,
    away_timeout: Duration,
}

impl PresenceManager {
    pub fn new() -> Self {
        Self {
            presences: Arc::new(RwLock::new(HashMap::new())),
            connection_to_user: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout: Duration::minutes(5),
            away_timeout: Duration::minutes(15),
        }
    }

    pub fn with_timeouts(idle_timeout: Duration, away_timeout: Duration) -> Self {
        Self {
            presences: Arc::new(RwLock::new(HashMap::new())),
            connection_to_user: Arc::new(RwLock::new(HashMap::new())),
            idle_timeout,
            away_timeout,
        }
    }

    /// User connects
    pub fn user_connected(
        &self,
        user_id: String,
        user_name: String,
        connection: PresenceConnection,
    ) -> Presence {
        let mut presences = self.presences.write();
        let now = Utc::now();

        let presence = presences
            .entry(user_id.clone())
            .or_insert_with(|| Presence {
                user_id: user_id.clone(),
                user_name: user_name.clone(),
                status: UserStatus::Online,
                status_message: None,
                last_seen: now,
                connections: Vec::new(),
                metadata: HashMap::new(),
            });

        // Update presence
        presence.user_name = user_name;
        presence.status = UserStatus::Online;
        presence.last_seen = now;
        // A reconnect carrying the same connection id must not leave two
        // entries behind (RT14).
        presence
            .connections
            .retain(|c| c.connection_id != connection.connection_id);
        presence.connections.push(connection.clone());

        // Map connection to user
        self.connection_to_user
            .write()
            .insert(connection.connection_id.clone(), user_id.clone());

        presence.clone()
    }

    /// User disconnects
    pub fn user_disconnected(&self, connection_id: &str) -> Option<Presence> {
        let user_id = self.connection_to_user.write().remove(connection_id)?;

        let mut presences = self.presences.write();
        let presence = presences.get_mut(&user_id)?;

        // Remove connection
        presence
            .connections
            .retain(|c| c.connection_id != connection_id);
        presence.last_seen = Utc::now();

        // Update status based on remaining connections
        if presence.connections.is_empty() {
            presence.status = UserStatus::Offline;
            // Don't remove presence, keep for "last seen" info
        }

        Some(presence.clone())
    }

    /// Update connection activity.
    ///
    /// The `user_id` is copied out of `connection_to_user` and its guard is
    /// released **before** `presences` is taken. Holding the first read guard
    /// across the second write lock inverted the lock order used by
    /// `user_connected`/`add_connection`/`remove_connection` and was a real
    /// deadlock (RT1).
    pub fn update_activity(&self, connection_id: &str) {
        let user_id = {
            let connections = self.connection_to_user.read();
            connections.get(connection_id).cloned()
        };
        let Some(user_id) = user_id else {
            return;
        };

        // One timestamp for the whole logical event, so `last_seen` and
        // `last_activity` cannot differ by a tick (RT14).
        let now = Utc::now();
        let mut presences = self.presences.write();
        if let Some(presence) = presences.get_mut(&user_id) {
            presence.last_seen = now;

            if let Some(conn) = presence
                .connections
                .iter_mut()
                .find(|c| c.connection_id == connection_id)
            {
                conn.last_activity = now;
            }

            // Update status from Away to Online if user was away
            if presence.status == UserStatus::Away {
                presence.status = UserStatus::Online;
            }
        }
    }

    /// Set user status
    pub fn set_status(
        &self,
        user_id: &str,
        status: UserStatus,
        message: Option<String>,
    ) -> Result<()> {
        let mut presences = self.presences.write();
        let presence = presences.get_mut(user_id).ok_or_else(|| {
            Error::Validation(ValidationError::Invalid("User not found".to_string()))
        })?;

        presence.status = status;
        presence.status_message = message;
        presence.last_seen = Utc::now();

        Ok(())
    }

    /// Set status message
    pub fn set_status_message(&self, user_id: &str, message: Option<String>) -> Result<()> {
        let mut presences = self.presences.write();
        let presence = presences.get_mut(user_id).ok_or_else(|| {
            Error::Validation(ValidationError::Invalid("User not found".to_string()))
        })?;

        presence.status_message = message;
        presence.last_seen = Utc::now();

        Ok(())
    }

    /// Get presence for user
    pub fn get_presence(&self, user_id: &str) -> Option<Presence> {
        self.presences.read().get(user_id).cloned()
    }

    /// Get multiple presences
    pub fn get_presences(&self, user_ids: &[String]) -> Vec<Presence> {
        let presences = self.presences.read();
        user_ids
            .iter()
            .filter_map(|id| presences.get(id).cloned())
            .collect()
    }

    /// Get all online users
    pub fn get_online_users(&self) -> Vec<Presence> {
        self.presences
            .read()
            .values()
            .filter(|p| {
                p.status == UserStatus::Online
                    || p.status == UserStatus::Away
                    || p.status == UserStatus::Busy
            })
            .cloned()
            .collect()
    }

    /// Get user count by status
    pub fn get_status_counts(&self) -> HashMap<UserStatus, usize> {
        let mut counts = HashMap::new();
        for presence in self.presences.read().values() {
            *counts.entry(presence.status).or_insert(0) += 1;
        }
        counts
    }

    /// Check if user is online
    pub fn is_online(&self, user_id: &str) -> bool {
        self.presences
            .read()
            .get(user_id)
            .map(|p| {
                matches!(
                    p.status,
                    UserStatus::Online | UserStatus::Away | UserStatus::Busy
                )
            })
            .unwrap_or(false)
    }

    /// Get connections for user
    pub fn get_connections(&self, user_id: &str) -> Vec<PresenceConnection> {
        self.presences
            .read()
            .get(user_id)
            .map(|p| p.connections.clone())
            .unwrap_or_default()
    }

    /// Add connection to existing user
    pub fn add_connection(&self, user_id: &str, connection: PresenceConnection) -> Result<()> {
        let mut presences = self.presences.write();
        let presence = presences.get_mut(user_id).ok_or_else(|| {
            Error::Validation(ValidationError::Invalid("User not found".to_string()))
        })?;

        presence
            .connections
            .retain(|c| c.connection_id != connection.connection_id);
        presence.connections.push(connection.clone());
        self.connection_to_user
            .write()
            .insert(connection.connection_id.clone(), user_id.to_string());
        presence.last_seen = Utc::now();
        presence.status = UserStatus::Online;

        Ok(())
    }

    /// Remove connection from user
    pub fn remove_connection(&self, user_id: &str, connection_id: &str) -> Result<bool> {
        let mut presences = self.presences.write();
        let presence = presences.get_mut(user_id).ok_or_else(|| {
            Error::Validation(ValidationError::Invalid("User not found".to_string()))
        })?;

        let len_before = presence.connections.len();
        presence
            .connections
            .retain(|c| c.connection_id != connection_id);
        self.connection_to_user.write().remove(connection_id);

        if presence.connections.is_empty() {
            presence.status = UserStatus::Offline;
        }

        Ok(presence.connections.len() < len_before)
    }

    /// Update away/busy statuses based on activity
    pub fn update_statuses(&self) {
        let now = Utc::now();
        let mut presences = self.presences.write();

        for presence in presences.values_mut() {
            if presence.status == UserStatus::Online {
                // Check if any connection has recent activity
                let has_recent_activity = presence
                    .connections
                    .iter()
                    .any(|c| now - c.last_activity < self.idle_timeout);

                if !has_recent_activity {
                    // Check if user has been idle longer than away timeout
                    if now - presence.last_seen > self.away_timeout {
                        presence.status = UserStatus::Away;
                    }
                }
            }
        }
    }

    /// Clean up stale presences
    pub fn cleanup_stale(&self, max_age: Duration) -> usize {
        let mut presences = self.presences.write();
        let now = Utc::now();

        let stale_ids: Vec<String> = presences
            .iter()
            .filter(|(_, p)| p.status == UserStatus::Offline && now - p.last_seen > max_age)
            .map(|(id, _)| id.clone())
            .collect();

        let count = stale_ids.len();
        for id in stale_ids {
            presences.remove(&id);
        }
        count
    }
}

impl Default for PresenceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_user_connected() {
        let manager = PresenceManager::new();
        let conn = PresenceConnection {
            connection_id: "conn1".to_string(),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
            device: None,
            browser: None,
            os: None,
            connected_at: Utc::now(),
            last_activity: Utc::now(),
            rooms: Vec::new(),
        };

        let presence = manager.user_connected("user1".to_string(), "Test User".to_string(), conn);

        assert_eq!(presence.user_id, "user1");
        assert_eq!(presence.user_name, "Test User");
        assert_eq!(presence.status, UserStatus::Online);
        assert_eq!(presence.connections.len(), 1);
    }

    #[test]
    fn test_user_disconnected() {
        let manager = PresenceManager::new();
        let conn = PresenceConnection {
            connection_id: "conn1".to_string(),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
            device: None,
            browser: None,
            os: None,
            connected_at: Utc::now(),
            last_activity: Utc::now(),
            rooms: Vec::new(),
        };

        manager.user_connected("user1".to_string(), "Test User".to_string(), conn.clone());

        let presence = manager.user_disconnected("conn1").unwrap();
        assert_eq!(presence.status, UserStatus::Offline);
        assert_eq!(presence.connections.len(), 0);
    }

    #[test]
    fn test_multiple_connections() {
        let manager = PresenceManager::new();
        let conn1 = PresenceConnection {
            connection_id: "conn1".to_string(),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
            device: None,
            browser: None,
            os: None,
            connected_at: Utc::now(),
            last_activity: Utc::now(),
            rooms: Vec::new(),
        };
        let conn2 = PresenceConnection {
            connection_id: "conn2".to_string(),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("mobile".to_string()),
            device: None,
            browser: None,
            os: None,
            connected_at: Utc::now(),
            last_activity: Utc::now(),
            rooms: Vec::new(),
        };

        manager.user_connected("user1".to_string(), "Test User".to_string(), conn1);
        manager.add_connection("user1", conn2).unwrap();

        let presence = manager.get_presence("user1").unwrap();
        assert_eq!(presence.connections.len(), 2);

        manager.remove_connection("user1", "conn1").unwrap();

        let presence = manager.get_presence("user1").unwrap();
        assert_eq!(presence.connections.len(), 1);
        assert_eq!(presence.status, UserStatus::Online);
    }

    fn connection(id: &str) -> PresenceConnection {
        PresenceConnection {
            connection_id: id.to_string(),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
            device: None,
            browser: None,
            os: None,
            connected_at: Utc::now(),
            last_activity: Utc::now(),
            rooms: Vec::new(),
        }
    }

    /// RT14: a re-connect that reuses the same connection id must replace the
    /// entry, not append a duplicate.
    #[test]
    fn reconnecting_with_the_same_id_does_not_duplicate_the_connection() {
        let manager = PresenceManager::new();
        manager.user_connected(
            "user1".to_string(),
            "Test User".to_string(),
            connection("c1"),
        );
        manager.user_connected(
            "user1".to_string(),
            "Test User".to_string(),
            connection("c1"),
        );

        assert_eq!(manager.get_connections("user1").len(), 1);
    }

    /// RT1: activity must be recorded for both the presence and the specific
    /// connection. This exercises the path that used to invert the lock order
    /// (`connection_to_user` read held while taking `presences` write).
    #[test]
    fn activity_updates_presence_and_connection() {
        let manager = PresenceManager::new();
        let before = Utc::now();
        manager.user_connected(
            "user1".to_string(),
            "Test User".to_string(),
            connection("c1"),
        );
        manager
            .set_status("user1", UserStatus::Away, None)
            .expect("status");

        manager.update_activity("c1");

        let presence = manager.get_presence("user1").expect("presence");
        assert_eq!(presence.status, UserStatus::Online);
        assert!(presence.last_seen >= before);
        assert!(presence.connections[0].last_activity >= before);
    }

    #[test]
    fn test_status_management() {
        let manager = PresenceManager::new();
        let conn = PresenceConnection {
            connection_id: "conn1".to_string(),
            ip_address: Some("127.0.0.1".to_string()),
            user_agent: Some("test".to_string()),
            device: None,
            browser: None,
            os: None,
            connected_at: Utc::now(),
            last_activity: Utc::now(),
            rooms: Vec::new(),
        };

        manager.user_connected("user1".to_string(), "Test User".to_string(), conn);

        manager
            .set_status("user1", UserStatus::Busy, Some("In a meeting".to_string()))
            .unwrap();

        let presence = manager.get_presence("user1").unwrap();
        assert_eq!(presence.status, UserStatus::Busy);
        assert_eq!(presence.status_message, Some("In a meeting".to_string()));
    }
}
