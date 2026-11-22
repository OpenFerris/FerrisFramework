//! Construction of the `object_store` backend named by a [`StoreConfig`].

use ferris_core::{Error, FileError, Result, ValidationError};
use object_store::ObjectStore;
use std::path::PathBuf;
use std::sync::Arc;

use super::types::{StorageBackend, StoreConfig};

pub(super) async fn create_object_store(config: &StoreConfig) -> Result<Arc<dyn ObjectStore>> {
    let store: Arc<dyn ObjectStore> = match config.backend {
        StorageBackend::Local => {
            let path = config
                .local_path
                .clone()
                .unwrap_or_else(|| PathBuf::from("./files"));
            let store =
                object_store::local::LocalFileSystem::new_with_prefix(path).map_err(|e| {
                    Error::File(FileError::Storage(format!(
                        "Failed to create local store: {}",
                        e
                    )))
                })?;
            Arc::new(store)
        }
        StorageBackend::S3 => {
            if let Some(s3_config) = &config.s3 {
                let builder = object_store::aws::AmazonS3Builder::new()
                    .with_bucket_name(&s3_config.bucket)
                    .with_region(&s3_config.region)
                    .with_access_key_id(&s3_config.access_key_id)
                    .with_secret_access_key(&s3_config.secret_access_key);

                let builder = if let Some(endpoint) = &s3_config.endpoint {
                    builder.with_endpoint(endpoint)
                } else {
                    builder
                };

                let store = builder.build().map_err(|e| {
                    Error::File(FileError::Storage(format!(
                        "Failed to create S3 store: {}",
                        e
                    )))
                })?;
                Arc::new(store)
            } else {
                return Err(Error::Validation(ValidationError::Invalid(
                    "S3 config required for S3 backend".to_string(),
                )));
            }
        }
        StorageBackend::Azure => {
            if let Some(azure_config) = &config.azure {
                let store = object_store::azure::MicrosoftAzureBuilder::new()
                    .with_account(&azure_config.account)
                    .with_container_name(&azure_config.container)
                    .with_access_key(&azure_config.access_key)
                    .build()
                    .map_err(|e| {
                        Error::File(FileError::Storage(format!(
                            "Failed to create Azure store: {}",
                            e
                        )))
                    })?;
                Arc::new(store)
            } else {
                return Err(Error::Validation(ValidationError::Invalid(
                    "Azure config required for Azure backend".to_string(),
                )));
            }
        }
        StorageBackend::Gcs => {
            if let Some(gcs_config) = &config.gcs {
                let store = object_store::gcp::GoogleCloudStorageBuilder::new()
                    .with_bucket_name(&gcs_config.bucket)
                    .with_service_account_path(
                        gcs_config.credentials_path.to_string_lossy().as_ref(),
                    )
                    .build()
                    .map_err(|e| {
                        Error::File(FileError::Storage(format!(
                            "Failed to create GCS store: {}",
                            e
                        )))
                    })?;
                Arc::new(store)
            } else {
                return Err(Error::Validation(ValidationError::Invalid(
                    "GCS config required for GCS backend".to_string(),
                )));
            }
        }
        StorageBackend::Memory => {
            let store = object_store::memory::InMemory::new();
            Arc::new(store)
        }
    };

    Ok(store)
}
