use crate::DownloadedFile;
use axum::{
    body::to_bytes,
    http::{StatusCode, header},
};

use super::signed_download_response;

fn sample() -> DownloadedFile {
    DownloadedFile {
        id: "f1".to_string(),
        // A hostile name must never reach the header verbatim.
        filename: "a\"b\nc\\d.bin".to_string(),
        content_type: "application/octet-stream".to_string(),
        bytes: b"hi".to_vec(),
    }
}

/// R-FW-2b: a verified fetch answers 200 with the bytes and a sanitized
/// `Content-Disposition`.
#[tokio::test]
async fn a_verified_fetch_streams_the_bytes_with_a_safe_filename() {
    let response = signed_download_response(Ok(sample()));
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(header::CONTENT_TYPE).unwrap(),
        "application/octet-stream"
    );
    let disposition = response
        .headers()
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    // The hostile characters were replaced; the value is header-safe.
    assert!(disposition.starts_with("inline; filename=\""));
    assert!(!disposition.contains('\n'), "{disposition}");
    assert!(!disposition.contains('\\'), "{disposition}");
    assert!(disposition.contains("a_b_c_d.bin"), "{disposition}");

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"hi");
}

/// A validly signed but unknown id is a 404; every other failure (forged or
/// expired signature, internal read error) is a 403.
#[test]
fn the_status_mapping_separates_unknown_ids_from_bad_signatures() {
    assert_eq!(
        signed_download_response(Err(ferris_core::Error::File(
            ferris_core::FileError::NotFound("gone".to_string())
        )))
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        signed_download_response(Err(ferris_core::Error::Authz(
            ferris_core::AuthzError::PermissionDenied {
                action: "read".to_string(),
                resource: "file:f1".to_string(),
            }
        )))
        .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        signed_download_response(Err(ferris_core::Error::Job(ferris_core::JobError::Failed(
            "boom".to_string()
        ))))
        .status(),
        StatusCode::FORBIDDEN
    );
}
