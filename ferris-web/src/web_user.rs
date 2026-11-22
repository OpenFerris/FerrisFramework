//! Re-export of the Desk identity.
//!
//! `WebUser` lives in `ferris_schema::contract::identity` because it is the body
//! of `POST /api/auth/login` and `GET /api/auth/session`: `ferris-api` produces
//! it, `ferris-web` consumes it. Keeping a second copy here is what used to
//! force the hydrate build to know about the shape of the auth stack.

pub use ferris_schema::contract::identity::WebUser;
