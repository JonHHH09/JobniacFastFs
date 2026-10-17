//! The page-image transport: one resident PNG raster
//! served to the webview by the custom URI scheme `jobniac-page`.
//!
//! The URL carries only an opaque handle (a random UUID, one per raster) and the revision, never a
//! file path. The scheme serves only the current raster: any other handle or revision is not found.
//! A request must use GET, come from the app's own origin, carry no query, and name exactly two
//! path segments. Responses are PNG with `Cache-Control: no-store`.
//!
//! The handler is a pure function over the store so that it can be tested without a window; the
//! Tauri registration in `lib.rs` only looks up the store and forwards the request.

use std::sync::Arc;

use jobniac_core::ids::is_canonical_uuid_text;
use serde::Serialize;
use specta::Type;
use tauri::http::{self, Method, Request, Response, StatusCode, header};
use uuid::Uuid;

/// The custom URI scheme of the page images (the scheme of the `jobniac-page` URL).
pub const PAGE_SCHEME: &str = "jobniac-page";

/// The host of the page URL. Windows and Android serve a custom scheme as `http://<scheme>.localhost`;
/// the other platforms use `<scheme>://localhost`.
#[cfg(any(windows, target_os = "android"))]
pub const PAGE_HOST: &str = "jobniac-page.localhost";
/// The host of the page URL on this platform.
#[cfg(not(any(windows, target_os = "android")))]
pub const PAGE_HOST: &str = "localhost";

/// The URL scheme of the page images on this platform.
#[cfg(any(windows, target_os = "android"))]
pub const PAGE_URL_SCHEME: &str = "http";
/// The URL scheme of the page images on this platform.
#[cfg(not(any(windows, target_os = "android")))]
pub const PAGE_URL_SCHEME: &str = PAGE_SCHEME;

/// The origins of the webview that may request a page: the bundled app on each platform and the
/// development server. A request with any other `Origin` is refused.
pub const APP_ORIGINS: [&str; 3] = [
    "tauri://localhost",
    "http://tauri.localhost",
    "http://localhost:1420",
];

/// One resident raster: its handle, its revision, its size and the PNG bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    /// The opaque handle of the raster, a UUID text.
    pub handle: String,
    /// The revision of the raster, increasing for each replacement.
    pub revision: i64,
    /// The width of the PNG in pixels.
    pub width: u32,
    /// The height of the PNG in pixels.
    pub height: u32,
    /// The PNG bytes, shared with the responses that serve them.
    pub png: Arc<Vec<u8>>,
}

/// The single resident raster (one raster at a time). Replacing it makes the previous
/// handle and revision not found; clearing it drops the bytes.
#[derive(Debug, Default)]
pub struct PageRasterStore {
    current: Option<Raster>,
    last_revision: i64,
}

impl PageRasterStore {
    /// Makes `png` the current raster under a new handle and the next revision. Returns the raster
    /// that now serves, or `None` when the revision counter would overflow (it never does in
    /// practice; the refusal keeps the store unchanged).
    pub fn replace(&mut self, png: Vec<u8>, width: u32, height: u32) -> Option<Raster> {
        let revision = self.last_revision.checked_add(1)?;
        let raster = Raster {
            handle: Uuid::new_v4().to_string(),
            revision,
            width,
            height,
            png: Arc::new(png),
        };
        self.last_revision = revision;
        self.current = Some(raster.clone());
        Some(raster)
    }

    /// Drops the current raster (preview failure, shutdown or close).
    pub fn clear(&mut self) {
        self.current = None;
    }

    /// The current raster, if one is resident.
    #[must_use]
    pub fn current(&self) -> Option<&Raster> {
        self.current.as_ref()
    }
}

/// The path segment that names the artifact viewer's rasters (`/artifact/<handle>/<revision>`).
pub const ARTIFACT_PATH_SEGMENT: &str = "artifact";

/// The viewer a page image belongs to. Each viewer has its own raster store (each controller has its own
/// image provider), so a preview in one viewer never replaces the other's page. The
/// documents viewer keeps the two-segment URL shape; the artifact viewer adds the `artifact` segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageViewer {
    /// The documents viewer: `<scheme>://<host>/<handle>/<revision>`.
    Documents,
    /// The artifacts viewer (also the generation preview): `<scheme>://<host>/artifact/<handle>/<revision>`.
    Artifacts,
}

impl PageViewer {
    /// The viewer that a request for the page scheme addresses, from its path alone.
    #[must_use]
    pub fn of_request<B>(request: &Request<B>) -> Self {
        let artifact_prefix = format!("/{ARTIFACT_PATH_SEGMENT}/");
        if request.uri().path().starts_with(&artifact_prefix) {
            Self::Artifacts
        } else {
            Self::Documents
        }
    }

    /// The path text between the host and the handle of this viewer's URLs.
    fn path_prefix(self) -> String {
        match self {
            Self::Documents => String::new(),
            Self::Artifacts => format!("{ARTIFACT_PATH_SEGMENT}/"),
        }
    }
}

/// The URL of a raster of `viewer` on this platform: `<scheme>://<host>/[artifact/]<handle>/<revision>`.
#[must_use]
pub fn page_url_for(viewer: PageViewer, raster: &Raster) -> String {
    format!(
        "{PAGE_URL_SCHEME}://{PAGE_HOST}/{}{}/{}",
        viewer.path_prefix(),
        raster.handle,
        raster.revision
    )
}

/// The page-image reference the viewer receives. It carries the URL of
/// the resident raster and no path, token or byte array; the revision is a string because it is an
/// `i64`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(rename_all = "camelCase")]
pub struct PageImageRefDto {
    /// The canonical stored document ID the page belongs to.
    pub document_id: String,
    /// The zero-based page index that was rendered.
    pub page_index: u32,
    /// The number of pages of the document.
    pub page_count: u32,
    /// The PNG width in pixels.
    pub width: u32,
    /// The PNG height in pixels.
    pub height: u32,
    /// The revision of the raster, as a decimal string.
    pub revision: String,
    /// The `jobniac-page` URL that serves the raster while it is the current revision.
    pub url: String,
}

/// The reference of a resident raster of `viewer` rendered for `document_id`, page `page_index` of
/// `page_count` pages.
#[must_use]
pub fn page_image_ref(
    viewer: PageViewer,
    document_id: &str,
    page_index: u32,
    page_count: u32,
    raster: &Raster,
) -> PageImageRefDto {
    PageImageRefDto {
        document_id: document_id.to_owned(),
        page_index,
        page_count,
        width: raster.width,
        height: raster.height,
        revision: raster.revision.to_string(),
        url: page_url_for(viewer, raster),
    }
}

/// Answers one request to the page scheme from the store of its viewer (see [`PageViewer::of_request`]).
/// A handle of the other store is not found, because each store only serves its own current raster.
#[must_use]
pub fn serve(store: &PageRasterStore, request: &Request<Vec<u8>>) -> Response<Vec<u8>> {
    if request.method() != Method::GET {
        return respond(StatusCode::METHOD_NOT_ALLOWED, None);
    }
    if let Some(origin) = request.headers().get(header::ORIGIN) {
        let allowed = origin
            .to_str()
            .is_ok_and(|text| APP_ORIGINS.contains(&text));
        if !allowed {
            return respond(StatusCode::FORBIDDEN, None);
        }
    }
    let uri = request.uri();
    let host_matches = uri.host() == Some(PAGE_HOST) && uri.port_u16().is_none();
    if uri.scheme_str() != Some(PAGE_URL_SCHEME) || !host_matches {
        return respond(StatusCode::FORBIDDEN, None);
    }
    if uri.query().is_some() {
        return respond(StatusCode::BAD_REQUEST, None);
    }
    let Some(rest) = uri.path().strip_prefix('/') else {
        return respond(StatusCode::BAD_REQUEST, None);
    };
    let artifact_prefix = format!("{ARTIFACT_PATH_SEGMENT}/");
    let rest = rest.strip_prefix(artifact_prefix.as_str()).unwrap_or(rest);
    let segments: Vec<&str> = rest.split('/').collect();
    let [handle, revision] = segments.as_slice() else {
        return respond(StatusCode::BAD_REQUEST, None);
    };
    if !is_canonical_uuid_text(handle) || !is_decimal(revision) {
        return respond(StatusCode::BAD_REQUEST, None);
    }
    let Ok(revision) = revision.parse::<i64>() else {
        return respond(StatusCode::BAD_REQUEST, None);
    };
    match store.current() {
        Some(raster) if raster.handle == *handle && raster.revision == revision => respond(
            StatusCode::OK,
            Some(("image/png", raster.png.as_ref().clone())),
        ),
        _ => respond(StatusCode::NOT_FOUND, None),
    }
}

/// Whether the text is one or more ASCII digits (no sign, no spaces).
fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

/// Builds a response with `Cache-Control: no-store`, and a content type when there is a body.
fn respond(status: StatusCode, content: Option<(&'static str, Vec<u8>)>) -> Response<Vec<u8>> {
    let mut builder = Response::builder()
        .status(status)
        .header(header::CACHE_CONTROL, "no-store");
    let body = match content {
        Some((media, bytes)) => {
            builder = builder.header(header::CONTENT_TYPE, media);
            bytes
        }
        None => Vec::new(),
    };
    builder
        .body(body)
        .unwrap_or_else(|_| http::Response::new(Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny PNG signature followed by bytes; the handler never decodes the image.
    const PNG: &[u8] = &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

    fn request(uri: &str) -> Request<Vec<u8>> {
        Request::builder()
            .method("GET")
            .uri(uri)
            .body(Vec::new())
            .unwrap_or_else(|_| Request::new(Vec::new()))
    }

    /// The documents-viewer URL of a raster.
    fn page_url(raster: &Raster) -> String {
        page_url_for(PageViewer::Documents, raster)
    }

    fn url_for(handle: &str, revision: &str) -> String {
        format!("{PAGE_URL_SCHEME}://{PAGE_HOST}/{handle}/{revision}")
    }

    fn store_with_one_page() -> (PageRasterStore, Raster) {
        let mut store = PageRasterStore::default();
        let raster = store.replace(PNG.to_vec(), 800, 600);
        (store, raster.unwrap_or_else(unreachable_raster))
    }

    fn unreachable_raster() -> Raster {
        Raster {
            handle: String::new(),
            revision: 0,
            width: 0,
            height: 0,
            png: Arc::new(Vec::new()),
        }
    }

    #[test]
    fn serves_the_current_revision_as_png_without_storing_it() {
        let (store, raster) = store_with_one_page();
        let response = serve(&store, &request(&page_url(&raster)));
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()[header::CONTENT_TYPE], "image/png");
        assert_eq!(response.headers()[header::CACHE_CONTROL], "no-store");
        assert_eq!(response.body().as_slice(), PNG);
    }

    #[test]
    fn refuses_an_unknown_handle_and_a_stale_revision() {
        let (mut store, first) = store_with_one_page();
        let other_handle = Uuid::new_v4().to_string();
        assert_eq!(
            serve(
                &store,
                &request(&url_for(&other_handle, &first.revision.to_string()))
            )
            .status(),
            StatusCode::NOT_FOUND
        );
        let second = store.replace(PNG.to_vec(), 1, 1);
        assert!(second.is_some());
        // The first raster is no longer served under its handle or its revision.
        assert_eq!(
            serve(&store, &request(&page_url(&first))).status(),
            StatusCode::NOT_FOUND
        );
    }

    #[test]
    fn refuses_every_request_while_no_raster_is_resident() {
        let mut store = PageRasterStore::default();
        let handle = Uuid::new_v4().to_string();
        assert_eq!(
            serve(&store, &request(&url_for(&handle, "1"))).status(),
            StatusCode::NOT_FOUND
        );
        let (populated, raster) = store_with_one_page();
        assert_eq!(
            serve(&populated, &request(&page_url(&raster))).status(),
            StatusCode::OK
        );
        store.clear();
        assert!(store.current().is_none());
    }

    #[test]
    fn refuses_malformed_requests_with_bad_request() {
        let (store, raster) = store_with_one_page();
        let handle = raster.handle.clone();
        let malformed = [
            url_for("not-a-uuid", "1"),
            url_for(&handle, "-1"),
            url_for(&handle, "+1"),
            url_for(&handle, "1x"),
            url_for(&handle, ""),
            format!("{PAGE_URL_SCHEME}://{PAGE_HOST}/{handle}"),
            format!("{PAGE_URL_SCHEME}://{PAGE_HOST}/{handle}/1/extra"),
            format!(
                "{PAGE_URL_SCHEME}://{PAGE_HOST}/{handle}/{}?page=0",
                raster.revision
            ),
        ];
        for uri in malformed {
            assert_eq!(
                serve(&store, &request(&uri)).status(),
                StatusCode::BAD_REQUEST,
                "{uri}"
            );
        }
    }

    #[test]
    fn refuses_other_origins_hosts_and_schemes() {
        let (store, raster) = store_with_one_page();
        let mut from_web = request(&page_url(&raster));
        from_web.headers_mut().insert(
            header::ORIGIN,
            http::HeaderValue::from_static("https://example.invalid"),
        );
        assert_eq!(serve(&store, &from_web).status(), StatusCode::FORBIDDEN);

        let other_host = format!(
            "{PAGE_URL_SCHEME}://evil.invalid/{}/{}",
            raster.handle, raster.revision
        );
        assert_eq!(
            serve(&store, &request(&other_host)).status(),
            StatusCode::FORBIDDEN
        );

        let with_port = format!(
            "{PAGE_URL_SCHEME}://{PAGE_HOST}:8080/{}/{}",
            raster.handle, raster.revision
        );
        assert_eq!(
            serve(&store, &request(&with_port)).status(),
            StatusCode::FORBIDDEN
        );
    }

    #[test]
    fn accepts_the_app_origin_and_refuses_other_methods() {
        let (store, raster) = store_with_one_page();
        let mut from_app = request(&page_url(&raster));
        from_app.headers_mut().insert(
            header::ORIGIN,
            http::HeaderValue::from_static("tauri://localhost"),
        );
        assert_eq!(serve(&store, &from_app).status(), StatusCode::OK);

        let post = Request::builder()
            .method("POST")
            .uri(page_url(&raster))
            .body(Vec::new())
            .unwrap_or_else(|_| Request::new(Vec::new()));
        assert_eq!(
            serve(&store, &post).status(),
            StatusCode::METHOD_NOT_ALLOWED
        );
    }

    #[test]
    fn the_reference_carries_the_url_and_string_revision_without_a_path() {
        let (_store, raster) = store_with_one_page();
        let reference = page_image_ref(
            PageViewer::Documents,
            "00000000-0000-4000-8000-0000000000ab",
            2,
            5,
            &raster,
        );
        assert_eq!(reference.revision, raster.revision.to_string());
        assert_eq!(reference.url, page_url(&raster));
        assert_eq!((reference.page_index, reference.page_count), (2, 5));
        assert_eq!((reference.width, reference.height), (800, 600));
        let json = serde_json::to_string(&reference).unwrap_or_default();
        assert!(json.contains("\"pageCount\":5"));
        assert!(!json.contains('\\'));
    }

    #[test]
    fn the_artifact_viewer_has_its_own_path_and_store() {
        let mut documents = PageRasterStore::default();
        let mut artifacts = PageRasterStore::default();
        let document = documents
            .replace(PNG.to_vec(), 1, 1)
            .unwrap_or_else(unreachable_raster);
        let artifact = artifacts
            .replace(PNG.to_vec(), 2, 2)
            .unwrap_or_else(unreachable_raster);
        let url = page_url_for(PageViewer::Artifacts, &artifact);
        assert!(url.contains(&format!("/{ARTIFACT_PATH_SEGMENT}/{}/", artifact.handle)));
        assert_eq!(
            PageViewer::of_request(&request(&url)),
            PageViewer::Artifacts
        );
        assert_eq!(
            PageViewer::of_request(&request(&page_url(&document))),
            PageViewer::Documents
        );
        // The artifact URL is served by the artifact store only.
        assert_eq!(serve(&artifacts, &request(&url)).status(), StatusCode::OK);
        assert_eq!(
            serve(&documents, &request(&url)).status(),
            StatusCode::NOT_FOUND
        );
        // The documents URL of the document raster is not served by the artifact store.
        assert_eq!(
            serve(&artifacts, &request(&page_url(&document))).status(),
            StatusCode::NOT_FOUND
        );
        // Replacing the documents raster leaves the artifact page served.
        assert!(documents.replace(PNG.to_vec(), 1, 1).is_some());
        assert_eq!(serve(&artifacts, &request(&url)).status(), StatusCode::OK);
        let reference = page_image_ref(PageViewer::Artifacts, "doc", 0, 1, &artifact);
        assert_eq!(reference.url, url);
    }

    #[test]
    fn the_artifact_path_keeps_the_strict_segment_checks() {
        let mut artifacts = PageRasterStore::default();
        let raster = artifacts
            .replace(PNG.to_vec(), 1, 1)
            .unwrap_or_else(unreachable_raster);
        let prefix = format!("{PAGE_URL_SCHEME}://{PAGE_HOST}/{ARTIFACT_PATH_SEGMENT}");
        for uri in [
            format!("{prefix}/{}", raster.handle),
            format!("{prefix}/{}/1/extra", raster.handle),
            format!("{prefix}/not-a-uuid/1"),
            format!("{prefix}/{}/{}?page=0", raster.handle, raster.revision),
        ] {
            assert_eq!(
                serve(&artifacts, &request(&uri)).status(),
                StatusCode::BAD_REQUEST,
                "{uri}"
            );
        }
    }
}
