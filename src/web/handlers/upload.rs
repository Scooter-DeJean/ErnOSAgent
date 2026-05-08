//! Upload handler — multipart file upload endpoint.
//! POST /api/upload — accepts any file, saves to data/uploads/, returns path.

use axum::extract::Multipart;
use axum::response::Json;
use serde_json::json;

/// Maximum upload size: 100 MB per file.
const MAX_UPLOAD_BYTES: usize = 100 * 1024 * 1024;

/// POST /api/upload — accept a file upload, save it, return the path.
pub async fn upload_file(mut multipart: Multipart) -> Json<serde_json::Value> {
    let upload_dir = std::path::PathBuf::from("data/uploads");
    if let Err(e) = std::fs::create_dir_all(&upload_dir) {
        return Json(json!({"error": format!("Failed to create upload dir: {}", e)}));
    }

    let mut uploaded = Vec::new();

    while let Ok(Some(field)) = multipart.next_field().await {
        if let Some(entry) = process_upload_field(field, &upload_dir).await {
            uploaded.push(entry);
        }
    }

    if uploaded.is_empty() {
        Json(json!({"error": "No files received"}))
    } else {
        Json(json!({"files": uploaded}))
    }
}

/// Process a single multipart field: read bytes, sanitize name, write to disk.
async fn process_upload_field(
    field: axum::extract::multipart::Field<'_>,
    upload_dir: &std::path::Path,
) -> Option<serde_json::Value> {
    let filename = field.file_name().unwrap_or("unnamed").to_string();

    let data = match field.bytes().await {
        Ok(d) => d,
        Err(e) => { tracing::warn!(err = %e, "Failed to read upload field"); return None; }
    };

    // §13.2: Reject oversized uploads to prevent DoS
    if data.len() > MAX_UPLOAD_BYTES {
        tracing::warn!(filename = %filename, size = data.len(), limit = MAX_UPLOAD_BYTES, "Upload rejected: file too large");
        return None;
    }

    let ext = std::path::Path::new(&filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("bin");
    let safe_name = format!("{}_{}.{}",
        chrono::Utc::now().format("%Y%m%d_%H%M%S"),
        &uuid::Uuid::new_v4().to_string()[..8],
        ext,
    );

    let dest = upload_dir.join(&safe_name);
    if let Err(e) = std::fs::write(&dest, &data) {
        tracing::error!(err = %e, file = %safe_name, "Failed to write upload");
        return None;
    }

    let size = data.len();
    tracing::info!(file = %safe_name, original = %filename, size, "File uploaded");

    Some(json!({
        "original_name": filename,
        "path": dest.display().to_string(),
        "size": size,
    }))
}
