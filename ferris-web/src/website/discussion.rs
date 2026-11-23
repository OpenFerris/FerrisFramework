//! Discussion: threaded comments on a website route.

use std::collections::HashMap;
use std::sync::RwLock;

use ferris_core::i18n::Locale;
use ferris_core::{DocumentError, Error, I18n, Result, ValidationError};

use super::page::normalize_route;

/// Translate one of the discussion's own messages.
fn tr(key: &str) -> String {
    I18n::default()
        .translate(&Locale::default(), key)
        .to_string()
}

/// A threaded comment on a website route.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Comment {
    pub id: String,
    pub route: String,
    pub parent: Option<String>,
    pub author: String,
    pub body: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

/// In-memory discussion store.
#[derive(Default)]
pub struct Discussion {
    comments: RwLock<HashMap<String, Comment>>,
}

impl Discussion {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a comment (optionally replying to `parent` on the same route).
    pub fn add(
        &self,
        route: &str,
        author: &str,
        body: &str,
        parent: Option<String>,
    ) -> Result<Comment> {
        let route = normalize_route(route);
        if body.trim().is_empty() {
            return Err(Error::Validation(ValidationError::Required(
                "comment body".to_string(),
            )));
        }
        if let Some(parent_id) = &parent {
            let comments = self.comments.read().unwrap_or_else(|e| e.into_inner());
            let parent_comment = comments.get(parent_id).ok_or_else(|| {
                Error::Document(DocumentError::NotFound {
                    doctype: "Comment".to_string(),
                    name: parent_id.clone(),
                })
            })?;
            if parent_comment.route != route {
                return Err(Error::Validation(ValidationError::Invalid(tr(
                    "components.website.discussion.reply_same_page",
                ))));
            }
        }

        let comment = Comment {
            id: uuid::Uuid::new_v4().to_string(),
            route,
            parent,
            author: author.to_string(),
            body: body.to_string(),
            created_at: chrono::Utc::now(),
        };
        self.comments
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(comment.id.clone(), comment.clone());
        Ok(comment)
    }

    /// All comments on a route, oldest first.
    pub fn list(&self, route: &str) -> Vec<Comment> {
        let route = normalize_route(route);
        let mut comments: Vec<Comment> = self
            .comments
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|c| c.route == route)
            .cloned()
            .collect();
        comments.sort_by_key(|c| c.created_at);
        comments
    }

    /// Direct replies to `parent`.
    pub fn replies(&self, parent: &str) -> Vec<Comment> {
        let mut replies: Vec<Comment> = self
            .comments
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|c| c.parent.as_deref() == Some(parent))
            .cloned()
            .collect();
        replies.sort_by_key(|c| c.created_at);
        replies
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.comments
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id)
            .map(|_| ())
            .ok_or_else(|| {
                Error::Document(DocumentError::NotFound {
                    doctype: "Comment".to_string(),
                    name: id.to_string(),
                })
            })
    }
}
