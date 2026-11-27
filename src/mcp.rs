use crate::{EncodedFrame, FrameOptions, Window, service::Service};
use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, Content, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct InspectInput {
    pub source: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FrameInput {
    pub source: String,
    #[serde(default)]
    pub options: FrameOptions,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TranscriptInput {
    pub source: String,
    #[serde(default)]
    pub window: Window,
    pub language: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CloseInput {
    pub video_id: String,
}
#[derive(Clone)]
pub struct VideoServer {
    service: Arc<Service>,
    tool_router: ToolRouter<Self>,
    response_slots: Arc<tokio::sync::Semaphore>,
}
#[tool_router(router=watch_router)]
impl VideoServer {
    #[tool(
        description = "Watch bounded temporal windows: seconds 0.25..30, fps 0.5..30, seconds*fps <=16, width 160..640. First call source; continue with video_id. Omit start to advance, set start to seek. Returns portrait-aware periodic filmstrip plus samples around regional/gradual change hints (event_budget 0..4, default 2; 0 is cheapest). Defaults 15s, 1fps, width 320, visual-only. For motion use 0.5s at 30fps. Frames are upright: rotation/flip display metadata (e.g. portrait phone video) is applied, and metadata reports rotation plus display_width/display_height. For small text use inspect_video_frames with a full-frame crop normalized in that display orientation and optional local OCR. include_speech adds local transcription. Images are samples, not continuous perception. Close handles."
    )]
    pub async fn watch_video(
        &self,
        Parameters(input): Parameters<crate::watch::WatchRequest>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let permit = match self.response_slot() {
            Ok(p) => p,
            Err(e) => return Ok(e),
        };
        let result = tokio::select! {biased;_=context.ct.cancelled()=>return Ok(error(anyhow::anyhow!("request cancelled"))),result=self.service.watch(input)=>result};
        Ok(match result {
            Ok(page) => encode_response(permit,move || {
                let mut content = vec![Content::text(
                    match serde_json::to_string(&page){Ok(s)=>s,Err(e)=>return error(e.into())},
                )];
                if !page.jpeg.is_empty() {
                    content.push(Content::image(STANDARD.encode(&page.jpeg), "image/jpeg"));
                }
                if !page.event_jpeg.is_empty() {
                    content.push(Content::text("Event evidence filmstrip; map cells using event_frames[].frame.image_rect and actual timestamps."));
                    content.push(Content::image(
                        STANDARD.encode(&page.event_jpeg),
                        "image/jpeg",
                    ));
                }
                CallToolResult::success(content)
            }).await?,
            Err(e) => error(e),
        })
    }
    #[tool(
        description = "Resolve visual uncertainty with 1..8 strictly increasing timestamps spanning <=30s. Use retained video_id or source. Crop is a full-frame rectangle {x,y,width,height}, each 0..1, entirely inside frame, normalized in display orientation (the upright orientation of returned images, rotation metadata applied), not filmstrip pixels. max_width/max_height 1..2048, default 1280; no upscaling. Returns individual upright JPEGs, actual/requested timestamps, display-oriented source-pixel crop bounds, optional local Windows OCR and tentative text changes/motion. Optional track_region inside crop follows an appearance patch using dense timestamps and reports lost/ambiguous states, not semantic identity. include_ocr defaults false. OCR can be wrong or unavailable: verify against images. Exact repeated video_id requests reuse cache without decoding/OCR; detail inspection does not advance watch playhead. At most 12 MiB of images. Use denser short windows to check direction/order; unobserved intervals are unknown."
    )]
    pub async fn inspect_video_frames(
        &self,
        Parameters(input): Parameters<crate::detail::DetailRequest>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let permit = match self.response_slot() {
            Ok(p) => p,
            Err(e) => return Ok(e),
        };
        let result = tokio::select! {biased;_=context.ct.cancelled()=>return Ok(error(anyhow::anyhow!("request cancelled"))),result=self.service.details(input)=>result};
        Ok(match result {
            Ok(page) => encode_response(permit, move || detail_content(page)).await?,
            Err(e) => error(e),
        })
    }
    #[tool(
        description = "Release a watch_video handle and its native decoder, cached views and temporary source files. Handles are local to this server process; idle handles expire after ten minutes and are pruned on the next viewing operation."
    )]
    pub async fn close_video(
        &self,
        Parameters(input): Parameters<CloseInput>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(match self.service.close_video(input.video_id).await {
            Ok(closed) => json(serde_json::json!({"closed":closed})),
            Err(e) => error(e),
        })
    }
    fn response_slot(&self) -> Result<tokio::sync::OwnedSemaphorePermit, CallToolResult> {
        self.response_slots
            .clone()
            .try_acquire_owned()
            .map_err(|_| {
                error(anyhow::anyhow!(
                    "server busy preparing images; retry after current work"
                ))
            })
    }
    pub fn new(service: Service) -> Self {
        Self {
            service: Arc::new(service),
            tool_router: Self::tool_router() + Self::watch_router(),
            response_slots: Arc::new(tokio::sync::Semaphore::new(4)),
        }
    }
}
async fn encode_response(
    permit: tokio::sync::OwnedSemaphorePermit,
    work: impl FnOnce() -> CallToolResult + Send + 'static,
) -> Result<CallToolResult, rmcp::ErrorData> {
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .map_err(|e| {
        rmcp::ErrorData::internal_error(format!("image response worker failed: {e}"), None)
    })
}
fn error(e: anyhow::Error) -> CallToolResult {
    tracing::warn!(error=%e,"tool failed");
    CallToolResult::error(vec![Content::text(format!("{e:#}"))])
}
fn json(value: impl serde::Serialize) -> CallToolResult {
    match serde_json::to_string(&value) {
        Ok(text) => CallToolResult::success(vec![Content::text(text)]),
        Err(e) => error(e.into()),
    }
}
pub fn detail_content(page: crate::detail::DetailPage) -> CallToolResult {
    let mut content = vec![Content::text(match serde_json::to_string(&page) {
        Ok(s) => s,
        Err(e) => return error(e.into()),
    })];
    for (i, frame) in page.frames.into_iter().enumerate() {
        content.push(Content::text(format!(
            "Detail image {i}, actual {:.6}s (requested {:.6}s). Source pixels {:?}.",
            frame.timestamp, frame.requested_timestamp, frame.source_pixels
        )));
        content.push(Content::image(
            STANDARD.encode(frame.image.bytes),
            frame.image.format.mime(),
        ));
    }
    CallToolResult::success(content)
}
pub fn frame_content(frames: Vec<EncodedFrame>) -> CallToolResult {
    let mut content = Vec::with_capacity(frames.len() * 2);
    for f in frames {
        content.push(Content::text(serde_json::json!({"timestamp":f.timestamp,"estimated_timestamp":f.estimated_timestamp,"width":f.width,"height":f.height}).to_string()));
        content.push(Content::image(STANDARD.encode(f.bytes), f.format.mime()));
    }
    CallToolResult::success(content)
}
#[tool_router]
impl VideoServer {
    #[tool(
        description = "Inspect container and streams first. Source is a local path or an explicitly enabled HTTP(S) URL. Video width/height are the coded (stored) size; rotation is the display-matrix angle as ffprobe reports it, display_transform what returned frames have applied, and display_width/display_height the upright size images are returned in."
    )]
    pub async fn inspect_video_metadata(
        &self,
        Parameters(input): Parameters<InspectInput>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let result = tokio::select! { biased; _=context.ct.cancelled()=>return Ok(error(anyhow::anyhow!("request cancelled"))), result=self.service.inspect(input.source)=>result };
        Ok(match result {
            Ok(m) => json(m),
            Err(e) => error(e),
        })
    }
    #[tool(
        description = "Scout speech in a bounded time window using a local Whisper model. Empty speech does not establish an absence of visual events."
    )]
    pub async fn get_timestamped_transcript(
        &self,
        Parameters(input): Parameters<TranscriptInput>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let result = tokio::select! { biased; _=context.ct.cancelled()=>return Ok(error(anyhow::anyhow!("request cancelled"))), result=self.service.transcript(input.source,input.window,input.language)=>result };
        Ok(match result {
            Ok(t) => json(t),
            Err(e) => error(e),
        })
    }
    #[tool(
        description = "Extract timestamp-sampled JPEG/WebP images. count 1..32, optional interval >=0.001s; max_width/max_height each 1..2048, no upscaling; total encoded <=12 MiB. Images are upright (rotation/flip display metadata applied). Optional crop is a normalized full-frame rectangle {x,y,width,height} in that display orientation, entirely inside frame. Use inspect_video_frames(video_id,...) for cached targeted evidence/OCR. This tool does not detect scene changes."
    )]
    pub async fn extract_keyframes(
        &self,
        Parameters(input): Parameters<FrameInput>,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let permit = match self.response_slot() {
            Ok(p) => p,
            Err(e) => return Ok(e),
        };
        let result = tokio::select! { biased; _=context.ct.cancelled()=>return Ok(error(anyhow::anyhow!("request cancelled"))), result=self.service.frames(input.source,input.options)=>result };
        Ok(match result {
            Ok(f) => encode_response(permit, move || frame_content(f)).await?,
            Err(e) => error(e),
        })
    }
}
#[tool_handler]
impl ServerHandler for VideoServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo { capabilities:ServerCapabilities::builder().enable_tools().build(),instructions:Some("Use watch_video(source) for a temporal overview with metadata, labeled samples and optional speech. Continue with video_id; omit start to advance or set start to seek. Use inspect_video_frames(video_id,timestamps,crop) for small text and uncertain events; optional OCR and patch tracking remain unverified evidence. Increase fps on short windows to inspect motion. These are samples, not continuous perception. Close handles when finished. Silent videos still require visual inspection.".into()),..Default::default() }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn images_are_standard_mcp_content() {
        let r = frame_content(vec![EncodedFrame {
            timestamp: 1.25,
            estimated_timestamp: false,
            width: 1,
            height: 1,
            format: crate::ImageFormat::Webp,
            bytes: vec![1, 2, 3],
        }]);
        let v = serde_json::to_value(r).unwrap();
        assert_eq!(v["content"][1]["type"], "image");
        assert_eq!(v["content"][1]["mimeType"], "image/webp");
        assert_eq!(v["content"][1]["data"], "AQID");
    }
    #[test]
    fn exact_tools_are_registered() {
        let server = VideoServer::new(Service::new(1, Default::default(), None, 4).unwrap());
        let tools = server.tool_router.list_all();
        assert_eq!(tools.len(), 6);
        for name in [
            "inspect_video_metadata",
            "get_timestamped_transcript",
            "extract_keyframes",
            "watch_video",
            "close_video",
            "inspect_video_frames",
        ] {
            assert!(tools.iter().any(|t| t.name == name));
        }
    }
    #[tokio::test]
    async fn bad_requests_return_tool_error() {
        let server = VideoServer::new(Service::new(1, Default::default(), None, 4).unwrap());
        let result = error(
            server
                .service
                .inspect("does-not-exist.mp4".into())
                .await
                .unwrap_err(),
        );
        assert_eq!(result.is_error, Some(true));
    }
    #[test]
    fn numeric_tool_limits_are_exposed_in_schema() {
        let schema =
            serde_json::to_value(schemars::schema_for!(crate::watch::WatchRequest)).unwrap();
        assert_eq!(schema["properties"]["width"]["maximum"], 640);
        assert_eq!(schema["properties"]["fps"]["maximum"], 30.0);
        let schema = serde_json::to_value(schemars::schema_for!(FrameOptions)).unwrap();
        assert_eq!(schema["properties"]["max_height"]["maximum"], 2048);
        assert_eq!(schema["properties"]["count"]["maximum"], 32);
    }
    #[tokio::test]
    async fn cancelled_image_waiter_retains_response_slot_until_worker_exits() {
        let slots = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = slots.clone().acquire_owned().await.unwrap();
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let task = tokio::spawn(async move {
            encode_response(permit, move || {
                started_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                CallToolResult::success(Vec::new())
            })
            .await
        });
        started_rx.await.unwrap();
        task.abort();
        let _ = task.await;
        assert_eq!(slots.available_permits(), 0);
        release_tx.send(()).unwrap();
        let _permit = slots.acquire().await.unwrap();
    }
}
