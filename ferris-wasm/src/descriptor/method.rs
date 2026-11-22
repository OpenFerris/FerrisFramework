//! The API method an app declares in `describe()`.

use serde::Serialize;

use crate::ferris::framework::descriptor::Method;

/// One API method an app declares in `describe()`.
///
/// The app serves the method through its `handle-request` export at the
/// reserved `/api/method/<name>` route; the host registers it on the
/// framework's `/api/method/<name>` whitelist endpoint.
#[derive(Debug, Clone, Serialize)]
pub struct MethodSpec {
    pub name: String,
    pub params: Vec<(String, String)>,
    pub returns: String,
    pub description: String,
    /// The declaration replaces the handler of the method with the same name
    /// (requires the `overrides` capability).
    pub overrides: bool,
}

impl From<&Method> for MethodSpec {
    fn from(method: &Method) -> Self {
        Self {
            name: method.name.clone(),
            params: method.params.clone(),
            returns: method.returns.clone(),
            description: method.description.clone(),
            overrides: method.overrides,
        }
    }
}
