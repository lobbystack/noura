//! Folders and ordinary files: listings, moves, trash, PDFs, local assets,
//! and Markdown link targets.

use local_core::{CoreError, MarkdownLinkTarget, UnmanagedFile, WorkspaceEngine, WorkspaceEntry};
use serde::Deserialize;
use tauri::State;

use crate::{AppState, os_files, with_engine};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderInput {
    relative_path: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FolderMoveInput {
    from: String,
    to: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetInput {
    source_relative_path: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MarkdownLinkInput {
    source_relative_path: String,
    target: String,
}

const MIME_BY_EXTENSION: &[(&str, &str)] = &[
    ("apng", "image/apng"),
    ("avif", "image/avif"),
    ("gif", "image/gif"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
];

const MAX_ASSET_BYTES: i64 = 20 * 1024 * 1024;

#[tauri::command(async)]
pub fn files_inspect_pdf(
    state: State<AppState>,
    relative_path: String,
) -> Result<local_core::PdfInfo, CoreError> {
    with_engine(&state, "files_inspect_pdf", |engine| {
        engine.inspect_pdf(&relative_path)
    })
}

#[tauri::command(async)]
pub fn files_read_pdf_range(
    state: State<AppState>,
    input: local_core::PdfRangeInput,
) -> Result<tauri::ipc::Response, CoreError> {
    with_engine(&state, "files_read_pdf_range", |engine| {
        engine.read_pdf_range(&input).map(tauri::ipc::Response::new)
    })
}

#[tauri::command(async)]
pub fn files_open_pdf_link(url: String) -> Result<(), CoreError> {
    os_files::open_http_link(&url)
}

#[tauri::command(async)]
pub fn files_read_local_asset(
    state: State<AppState>,
    input: AssetInput,
) -> Result<serde_json::Value, CoreError> {
    with_engine(&state, "files_read_local_asset", |engine| {
        let (relative_path, bytes) =
            engine.read_local_asset(&input.source_relative_path, &input.target, MAX_ASSET_BYTES)?;
        let extension = relative_path
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mime = MIME_BY_EXTENSION
            .iter()
            .find(|(candidate, _)| *candidate == extension)
            .map(|(_, mime)| *mime)
            .unwrap_or("application/octet-stream");
        Ok(serde_json::json!({
            "dataUrl": format!("data:{mime};base64,{}", base64_encode(&bytes)),
        }))
    })
}

#[derive(Deserialize)]
pub struct RemoteImageInput {
    url: String,
}

const MAX_REMOTE_IMAGE_BYTES: usize = 20 * 1024 * 1024;

/// Download one remote image the user chose to load and return it as a data
/// URL. The page's content security policy blocks remote images, so a note
/// can never load one on its own; only this explicit request fetches it.
#[tauri::command]
pub async fn files_fetch_remote_image(
    input: RemoteImageInput,
) -> Result<serde_json::Value, CoreError> {
    let operation = "files_fetch_remote_image";
    let unavailable = || {
        CoreError::new(
            "image_unavailable",
            local_core::ErrorCategory::Transient,
            "The image couldn't be downloaded",
            operation,
        )
    };
    let url = url::Url::parse(&input.url)
        .ok()
        .filter(|url| matches!(url.scheme(), "https" | "http"))
        .filter(|url| url.username().is_empty() && url.password().is_none())
        .ok_or_else(|| {
            CoreError::validation(
                "invalid_url",
                "The image address isn't a web address",
                operation,
            )
        })?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(5))
        .build()
        .map_err(|_| unavailable())?;
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|_| unavailable())?
        .error_for_status()
        .map_err(|_| unavailable())?;
    let mime = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| value.starts_with("image/") && value.len() <= 64)
        .ok_or_else(|| {
            CoreError::validation("not_an_image", "That address isn't an image", operation)
        })?;
    let too_large = || {
        CoreError::validation(
            "image_too_large",
            "The image is larger than 20 MB",
            operation,
        )
    };
    if response
        .content_length()
        .is_some_and(|length| length > MAX_REMOTE_IMAGE_BYTES as u64)
    {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
        if bytes.len() + chunk.len() > MAX_REMOTE_IMAGE_BYTES {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::json!({
        "dataUrl": format!("data:{mime};base64,{}", base64_encode(&bytes)),
    }))
}

#[tauri::command(async)]
pub fn files_resolve_markdown_link(
    state: State<AppState>,
    input: MarkdownLinkInput,
) -> Result<MarkdownLinkTarget, CoreError> {
    with_engine(&state, "files_resolve_markdown_link", |engine| {
        engine.resolve_markdown_link(&input.source_relative_path, &input.target)
    })
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        output.push(TABLE[((triple >> 18) & 63) as usize] as char);
        output.push(TABLE[((triple >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}

#[tauri::command(async)]
pub fn folders_create(state: State<AppState>, input: FolderInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_create", |engine| {
        engine.create_folder(&input.relative_path)
    })
}

#[tauri::command(async)]
pub fn folders_list(state: State<AppState>) -> Result<Vec<local_core::FolderEntry>, CoreError> {
    with_engine(&state, "folders_list", WorkspaceEngine::list_folders)
}

#[tauri::command(async)]
pub fn files_list(state: State<AppState>) -> Result<Vec<WorkspaceEntry>, CoreError> {
    with_engine(
        &state,
        "files_list",
        WorkspaceEngine::list_workspace_entries,
    )
}

#[tauri::command(async)]
pub fn files_list_non_managed_markdown(
    state: State<AppState>,
) -> Result<Vec<UnmanagedFile>, CoreError> {
    with_engine(&state, "files_list_non_managed_markdown", |engine| {
        engine.list_non_managed_markdown()
    })
}

#[tauri::command(async)]
pub fn folders_move(state: State<AppState>, input: FolderMoveInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_move", |engine| {
        engine.move_folder(&input.from, &input.to)
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTrashInput {
    relative_path: String,
}

#[tauri::command(async)]
pub fn files_move(state: State<AppState>, input: FolderMoveInput) -> Result<(), CoreError> {
    with_engine(&state, "files_move", |engine| {
        engine.move_file(&input.from, &input.to)
    })
}

#[tauri::command(async)]
pub fn files_trash(
    state: State<AppState>,
    input: FileTrashInput,
) -> Result<Option<String>, CoreError> {
    with_engine(&state, "files_trash", |engine| {
        engine.trash_path(&input.relative_path)
    })
}

#[tauri::command(async)]
pub fn folders_remove(state: State<AppState>, input: FolderInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_remove", |engine| {
        engine.remove_empty_folder(&input.relative_path)
    })
}
