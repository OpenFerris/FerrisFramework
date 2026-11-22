//! The shared host state a print operation needs.
//!
//! The HTTP adapter gets it from `ApiState`; an API method does not receive a
//! request context, so it captures the same shape at boot. Keeping the database
//! and the engine together is what lets both entry points share one path
//! implementation instead of duplicating permission and format resolution.

use std::sync::{Arc, OnceLock};

use ferris_api::ApiState;
use ferris_core::{Error, Result, ValidationError};

use crate::{PrintConfig, PrintEngine};

/// Database plus the process-wide renderer.
#[derive(Clone)]
pub struct PrintHost {
    pub api: Arc<ApiState>,
    pub engine: Arc<PrintEngine>,
}

impl PrintHost {
    #[must_use]
    pub const fn new(api: Arc<ApiState>, engine: Arc<PrintEngine>) -> Self {
        Self { api, engine }
    }
}

/// The process-wide engine: Typst resources (fonts) are loaded once, not per
/// request.
///
/// # Errors
///
/// A validation error when the bundled/extra fonts cannot be loaded.
pub fn print_engine() -> Result<Arc<PrintEngine>> {
    static ENGINE: OnceLock<std::result::Result<Arc<PrintEngine>, String>> = OnceLock::new();
    ENGINE
        .get_or_init(|| {
            PrintEngine::new(print_config())
                .map(Arc::new)
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map(Arc::clone)
        .map_err(|message| {
            Error::Validation(ValidationError::Invalid(format!(
                "print engine unavailable: {message}"
            )))
        })
}

/// The host's print configuration.
///
/// A deployment opts into file-backed templates with
/// `FERRIS_PRINT_TEMPLATE_DIR` (Jinja includes/extends and Typst `read()`).
/// When the variable is unset and the conventional `templates/print` directory
/// exists, that directory is used; otherwise only inline templates are
/// available.
#[must_use]
pub fn print_config() -> PrintConfig {
    let template_dir = std::env::var("FERRIS_PRINT_TEMPLATE_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| {
            std::path::Path::new("templates/print")
                .is_dir()
                .then(|| "templates/print".to_string())
        });
    let font_dir = std::env::var("FERRIS_PRINT_FONT_DIR")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    PrintConfig {
        template_dir,
        font_dir,
        ..PrintConfig::default()
    }
}

/// Build the host for one HTTP request.
///
/// # Errors
///
/// The process-wide engine failed to initialize.
pub fn host_from_api(api: Arc<ApiState>) -> Result<PrintHost> {
    Ok(PrintHost::new(api, print_engine()?))
}
