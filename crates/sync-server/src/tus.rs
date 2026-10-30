//! The blob routes of §21: uploads over tus 1.0.0 core with the
//! `creation`, `termination` and `expiration` extensions, and range reads.
//! Errors carry the error body of §26 like every other route.

use axum::Router;
use axum::body::Body;
use axum::extract::State;
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use bytes::Bytes;
use sync_protocol::blob::{MAX_BLOB_FILE, SEGMENT_LEN, blob_ciphertext_len};
use sync_protocol::types::{Digest, Id};
use sync_store::{Backend, BlobCreation, BlobUpload, Recorded, Store};

use crate::blobs::segment_key;
use crate::http::{ApiError, AppState, Auth, Segments, id, refused};

/// The tus version the server speaks.
pub const TUS_VERSION: &str = "1.0.0";
/// The tus extensions it offers.
pub const TUS_EXTENSIONS: &str = "creation,termination,expiration";
const SEGMENT: u64 = SEGMENT_LEN as u64;

/// The headers every tus response carries.
fn tus(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert("Tus-Resumable", HeaderValue::from_static(TUS_VERSION));
    headers.insert("Tus-Version", HeaderValue::from_static(TUS_VERSION));
    headers.insert("Tus-Extension", HeaderValue::from_static(TUS_EXTENSIONS));
    headers.insert(
        "Tus-Max-Size",
        HeaderValue::from(blob_ciphertext_len(MAX_BLOB_FILE)),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

fn tus_result(result: Result<Response, ApiError>) -> Response {
    tus(result.unwrap_or_else(IntoResponse::into_response))
}

fn header_number(headers: &HeaderMap, name: &str) -> Result<u64, ApiError> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| sync_protocol::types::from_decimal(v).ok())
        .ok_or_else(|| refused("bad_request", &format!("{name} isn't an unsigned decimal")))
}

/// Refuses a request that names another tus version than 1.0.0.
fn resumable(headers: &HeaderMap) -> Result<(), ApiError> {
    match headers.get("Tus-Resumable") {
        Some(v) if v.as_bytes() != TUS_VERSION.as_bytes() => Err(refused(
            "version_unsupported",
            "the server speaks tus 1.0.0",
        )),
        _ => Ok(()),
    }
}

/// `Upload-Metadata: blob <base64>,digest <base64>,domain <base64>`, in
/// standard base64 as tus requires.
fn metadata(headers: &HeaderMap) -> Result<(Id, Digest, Id), ApiError> {
    let text = headers
        .get("Upload-Metadata")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| refused("bad_request", "Upload-Metadata is missing"))?;
    let (mut blob, mut digest, mut domain) = (None, None, None);
    for pair in text.split(',') {
        let (name, value) = pair
            .trim()
            .split_once(' ')
            .ok_or_else(|| refused("bad_request", "an Upload-Metadata pair has no value"))?;
        let bytes = STANDARD
            .decode(value.trim())
            .map_err(|_| refused("bad_request", "an Upload-Metadata value isn't base64"))?;
        let slot = match name {
            "blob" => &mut blob,
            "digest" => &mut digest,
            "domain" => &mut domain,
            _ => continue,
        };
        if slot.replace(bytes).is_some() {
            return Err(refused("bad_request", "Upload-Metadata names a key twice"));
        }
    }
    let id_of = |bytes: Option<Vec<u8>>| {
        bytes
            .and_then(|b| <[u8; 16]>::try_from(b).ok())
            .map(Id)
            .filter(|id| !id.is_zero())
            .ok_or_else(|| {
                refused(
                    "bad_request",
                    "Upload-Metadata needs a 16-byte blob and domain",
                )
            })
    };
    let digest = digest
        .and_then(|b| <[u8; 32]>::try_from(b).ok())
        .map(Digest)
        .ok_or_else(|| refused("bad_request", "Upload-Metadata needs a 32-byte digest"))?;
    Ok((id_of(blob)?, digest, id_of(domain)?))
}

fn location(workspace: &Id, blob: &Id) -> String {
    format!(
        "/v2/workspaces/{}/blobs/{}",
        workspace.to_base64url(),
        blob.to_base64url()
    )
}

/// An HTTP date, as `Upload-Expires` needs.
fn http_date(seconds: u64) -> String {
    let timestamp = jiff::Timestamp::from_second(i64::try_from(seconds).unwrap_or(i64::MAX))
        .unwrap_or(jiff::Timestamp::UNIX_EPOCH);
    timestamp.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string()
}

async fn options() -> Response {
    tus(StatusCode::NO_CONTENT.into_response())
}

async fn create<B: Backend>(
    State(state): State<AppState<B>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    headers: HeaderMap,
) -> Response {
    tus_result(
        async {
            resumable(&headers)?;
            let workspace = id(&workspace)?;
            let length = header_number(&headers, "Upload-Length")?;
            let (blob, digest, domain) = metadata(&headers)?;
            let row = state
                .store
                .create_blob(
                    session,
                    workspace,
                    BlobCreation {
                        blob,
                        length,
                        digest,
                        domain,
                    },
                )
                .await?;
            let mut response = StatusCode::CREATED.into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::LOCATION,
                HeaderValue::from_str(&location(&workspace, &blob))
                    .map_err(|_| refused("bad_request", "the location isn't a header"))?,
            );
            headers.insert("Upload-Offset", HeaderValue::from(row.offset));
            if !row.complete
                && let Some(at) = row.delete_after
                && let Ok(expires) = HeaderValue::from_str(&http_date(at))
            {
                headers.insert("Upload-Expires", expires);
            }
            Ok(response)
        }
        .await,
    )
}

async fn head<B: Backend>(
    State(state): State<AppState<B>>,
    Auth(session): Auth,
    Segments((workspace, blob)): Segments<(String, String)>,
) -> Response {
    tus_result(
        async {
            let upload = state
                .store
                .blob_upload(session, id(&workspace)?, id(&blob)?)
                .await?;
            let mut response = StatusCode::OK.into_response();
            let headers = response.headers_mut();
            headers.insert("Upload-Offset", HeaderValue::from(upload.row.offset));
            headers.insert("Upload-Length", HeaderValue::from(upload.row.length));
            Ok(response)
        }
        .await,
    )
}

async fn delete_segments(
    blobs: &dyn crate::blobs::SegmentStore,
    workspace: &Id,
    blob: &Id,
    segments: &[Digest],
) {
    for (index, hash) in segments.iter().enumerate() {
        let key = segment_key(workspace, blob, index as u64, hash);
        if let Err(error) = blobs.delete(&key).await {
            tracing::warn!("can't delete a discarded segment: {error}");
        }
    }
}

async fn patch<B: Backend>(
    State(state): State<AppState<B>>,
    Auth(session): Auth,
    Segments((workspace, blob)): Segments<(String, String)>,
    headers: HeaderMap,
    request: Body,
) -> Response {
    tus_result(
        async {
            resumable(&headers)?;
            let (workspace, blob) = (id(&workspace)?, id(&blob)?);
            let content_type = headers.get(header::CONTENT_TYPE).map(HeaderValue::as_bytes);
            if content_type != Some(b"application/offset+octet-stream") {
                return Err(refused(
                    "bad_request",
                    "a PATCH carries application/offset+octet-stream",
                ));
            }
            let offset = header_number(&headers, "Upload-Offset")?;
            let upload = state.store.blob_upload(session, workspace, blob).await?;
            // Refuse a wrong offset before reading the body.
            let next = SEGMENT.min(upload.row.length.saturating_sub(upload.row.offset));
            Store::<B>::check_segment(&upload.row, offset, next)?;
            // The body lands in one buffer of the segment's size as it
            // arrives, so a PATCH holds about one segment in memory.
            let too_large = || refused("too_large", "a PATCH appends at most 1 MiB");
            let mut body = bytes::BytesMut::with_capacity(usize::try_from(next).unwrap_or(0));
            let mut frames = request.into_data_stream();
            while let Some(frame) = futures::StreamExt::next(&mut frames).await {
                let frame = frame.map_err(|_| refused("bad_request", "the body broke off"))?;
                if body.len() + frame.len() > SEGMENT_LEN {
                    return Err(too_large());
                }
                body.extend_from_slice(&frame);
            }
            let bytes = body.freeze();
            let len = bytes.len() as u64;
            Store::<B>::check_segment(&upload.row, offset, len)?;
            let hash = sync_protocol::crypto::hash(&bytes);
            let index = offset / SEGMENT;
            // The segment reaches durable storage before the store records
            // the new offset.
            let key = segment_key(&workspace, &blob, index, &hash);
            state
                .blobs
                .put(&key, bytes)
                .await
                .map_err(|e| ApiError(sync_store::Error::internal(e)))?;
            let recorded = state
                .store
                .record_segment(session, workspace, blob, offset, len, hash)
                .await;
            // A segment the store didn't record, such as one that lost a
            // race with the collector, names no row (§19).
            if recorded.is_err()
                && let Err(error) = state.blobs.delete(&key).await
            {
                tracing::warn!("can't delete an unrecorded segment: {error}");
            }
            match recorded? {
                Recorded::Stored(row) => {
                    let mut response = StatusCode::NO_CONTENT.into_response();
                    response
                        .headers_mut()
                        .insert("Upload-Offset", HeaderValue::from(row.offset));
                    Ok(response)
                }
                Recorded::Discarded(segments) => {
                    delete_segments(state.blobs.as_ref(), &workspace, &blob, &segments).await;
                    Err(refused(
                        "blob_digest_mismatch",
                        "the upload's segments don't match its digest",
                    ))
                }
            }
        }
        .await,
    )
}

async fn terminate<B: Backend>(
    State(state): State<AppState<B>>,
    Auth(session): Auth,
    Segments((workspace, blob)): Segments<(String, String)>,
    headers: HeaderMap,
) -> Response {
    tus_result(
        async {
            resumable(&headers)?;
            let (workspace, blob) = (id(&workspace)?, id(&blob)?);
            let segments = state.store.abandon_blob(session, workspace, blob).await?;
            delete_segments(state.blobs.as_ref(), &workspace, &blob, &segments).await;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        .await,
    )
}

/// `Range: bytes=a-b` or `bytes=a-`, as a start and an inclusive end.
fn range(headers: &HeaderMap) -> Result<(u64, Option<u64>), ApiError> {
    let Some(value) = headers.get(header::RANGE) else {
        return Ok((0, None));
    };
    let bad = || refused("bad_request", "Range names one range of bytes");
    let text = value.to_str().map_err(|_| bad())?;
    let spec = text.strip_prefix("bytes=").ok_or_else(bad)?;
    let (start, end) = spec.split_once('-').ok_or_else(bad)?;
    let start = sync_protocol::types::from_decimal(start).map_err(|_| bad())?;
    let end = if end.is_empty() {
        None
    } else {
        Some(sync_protocol::types::from_decimal(end).map_err(|_| bad())?)
    };
    if end.is_some_and(|end| end < start) {
        return Err(bad());
    }
    Ok((start, end))
}

/// Reads bytes `start..end` of a stored blob across its segments.
async fn read(
    blobs: &dyn crate::blobs::SegmentStore,
    workspace: &Id,
    upload: &BlobUpload,
    start: u64,
    end: u64,
) -> std::io::Result<Bytes> {
    let mut out = Vec::new();
    let mut at = start;
    while at < end {
        let index = at / SEGMENT;
        let hash = upload
            .segments
            .get(usize::try_from(index).map_err(std::io::Error::other)?)
            .ok_or_else(|| std::io::Error::other("a segment row is missing"))?;
        let within = at - index * SEGMENT;
        let take = (SEGMENT - within).min(end - at);
        let key = segment_key(workspace, &upload.row.blob, index, hash);
        let bytes = blobs.get(&key, within..within + take).await?;
        if at == start && take == end - start {
            // A range inside one segment, as clients ask for, goes out as
            // the store read it, without a copy.
            return Ok(bytes);
        }
        out.extend_from_slice(&bytes);
        at += take;
    }
    Ok(Bytes::from(out))
}

async fn download<B: Backend>(
    State(state): State<AppState<B>>,
    Auth(session): Auth,
    Segments((workspace, blob)): Segments<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let (start, end) = range(&headers)?;
    let upload = state
        .store
        .blob_download(session, workspace, id(&blob)?)
        .await?;
    let length = upload.row.length;
    if start >= length {
        let mut response = refused("bad_request", "the range starts past the blob").into_response();
        *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            HeaderValue::from_str(&format!("bytes */{length}")).expect("a header"),
        );
        return Ok(response);
    }
    // At most 1 MiB per response.
    let last = end
        .unwrap_or(u64::MAX)
        .min(length - 1)
        .min(start + SEGMENT - 1);
    let bytes = read(state.blobs.as_ref(), &workspace, &upload, start, last + 1)
        .await
        .map_err(|e| ApiError(sync_store::Error::internal(e)))?;
    let mut response = (StatusCode::PARTIAL_CONTENT, bytes).into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::CONTENT_RANGE,
        HeaderValue::from_str(&format!("bytes {start}-{last}/{length}")).expect("a header"),
    );
    headers.insert(header::ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    Ok(response)
}

/// The blob routes.
pub fn routes<B: Backend>() -> Router<AppState<B>> {
    Router::new()
        .route(
            "/v2/workspaces/{w}/blobs",
            post(create::<B>).options(options),
        )
        .route(
            "/v2/workspaces/{w}/blobs/{id}",
            axum::routing::get(download::<B>)
                .head(head::<B>)
                .patch(patch::<B>)
                .delete(terminate::<B>)
                .options(options),
        )
}
