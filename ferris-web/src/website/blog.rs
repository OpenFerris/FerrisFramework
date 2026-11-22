//! Blog: the `Blog Post` model and its in-memory store.

use std::collections::HashMap;
use std::sync::RwLock;

use ferris_core::{DocumentError, Error, Result, ValidationError};

/// A blog post (Frappe `Blog Post`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlogPost {
    pub slug: String,
    pub title: String,
    pub content: String,
    pub excerpt: Option<String>,
    pub author: String,
    pub tags: Vec<String>,
    pub published: bool,
    pub published_at: Option<chrono::DateTime<chrono::Utc>>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

impl BlogPost {
    pub fn new(
        slug: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
    ) -> Self {
        Self {
            slug: slug.into(),
            title: title.into(),
            content: content.into(),
            excerpt: None,
            author: "Administrator".to_string(),
            tags: Vec::new(),
            published: false,
            published_at: None,
            created_at: chrono::Utc::now(),
        }
    }

    pub fn published(mut self) -> Self {
        self.published = true;
        self.published_at = Some(chrono::Utc::now());
        self
    }

    pub fn with_author(mut self, author: impl Into<String>) -> Self {
        self.author = author.into();
        self
    }

    pub fn with_tag(mut self, tag: impl Into<String>) -> Self {
        self.tags.push(tag.into());
        self
    }
}

/// In-memory blog store.
#[derive(Default)]
pub struct Blog {
    posts: RwLock<HashMap<String, BlogPost>>,
}

impl Blog {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&self, post: BlogPost) -> Result<()> {
        let slug = normalize_slug(&post.slug)?;
        self.posts
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(slug.clone(), BlogPost { slug, ..post });
        Ok(())
    }

    /// Fetch a published post by slug.
    pub fn get(&self, slug: &str) -> Option<BlogPost> {
        let slug = slug.trim().to_lowercase();
        self.posts
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&slug)
            .filter(|post| post.published)
            .cloned()
    }

    /// Published posts, newest first.
    pub fn list_published(&self) -> Vec<BlogPost> {
        let mut posts: Vec<BlogPost> = self
            .posts
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .values()
            .filter(|post| post.published)
            .cloned()
            .collect();
        posts.sort_by(|a, b| b.published_at.cmp(&a.published_at));
        posts
    }

    /// Published posts carrying `tag`.
    pub fn by_tag(&self, tag: &str) -> Vec<BlogPost> {
        self.list_published()
            .into_iter()
            .filter(|post| post.tags.iter().any(|t| t.eq_ignore_ascii_case(tag)))
            .collect()
    }

    pub fn delete(&self, slug: &str) -> Result<()> {
        let slug = slug.trim().to_lowercase();
        self.posts
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&slug)
            .map(|_| ())
            .ok_or_else(|| {
                Error::Document(DocumentError::NotFound {
                    doctype: "Blog Post".to_string(),
                    name: slug.clone(),
                })
            })
    }
}

fn normalize_slug(slug: &str) -> Result<String> {
    let slug = slug.trim().to_lowercase();
    if slug.is_empty()
        || !slug
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(Error::Validation(ValidationError::Invalid(format!(
            "Invalid slug: {slug}"
        ))));
    }
    Ok(slug)
}
