pub mod commands;
#[cfg(target_os = "macos")]
pub mod macos;
mod staging;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::Emitter;
#[cfg(target_os = "macos")]
use tauri::Manager;
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

pub const MAX_BYTES: u64 = 100 * 1024 * 1024;
const DEADLINE: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize)]
pub struct PreviewError {
    pub kind: String,
    pub message: String,
}
impl PreviewError {
    pub fn new(kind: &str, message: impl ToString) -> Self {
        Self {
            kind: kind.into(),
            message: message.to_string(),
        }
    }
    pub fn io(e: impl ToString) -> Self {
        Self::new("local_io", e)
    }
    pub fn remote(e: impl ToString) -> Self {
        Self::new("download_failed", e)
    }
    pub fn session(e: impl ToString) -> Self {
        Self::new("session_unavailable", e)
    }
    pub fn invalid_file() -> Self {
        Self::new(
            "invalid_selection",
            "Quick Look requires one regular file (no folders or symlinks)",
        )
    }
}
impl std::fmt::Display for PreviewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for PreviewError {}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum Target {
    Sftp {
        #[serde(rename = "sessionId")]
        session_id: String,
        path: String,
    },
    Scp {
        #[serde(rename = "sessionId")]
        session_id: String,
        path: String,
    },
    S3 {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(rename = "bucketName")]
        bucket_name: String,
        key: String,
    },
}
impl Target {
    fn name(&self) -> Result<&str, PreviewError> {
        let path = match self {
            Self::Sftp { path, .. } | Self::Scp { path, .. } => path,
            Self::S3 {
                key, bucket_name, ..
            } => {
                if bucket_name.is_empty() {
                    return Err(PreviewError::invalid_file());
                }
                key
            }
        };
        if path.contains('\0') {
            return Err(PreviewError::invalid_file());
        }
        let name = path
            .rsplit('/')
            .next()
            .ok_or_else(PreviewError::invalid_file)?;
        crate::sftp::validate_remote_name(name).map_err(|_| PreviewError::invalid_file())
    }
    pub fn session_id(&self) -> &str {
        match self {
            Self::Sftp { session_id, .. }
            | Self::Scp { session_id, .. }
            | Self::S3 { session_id, .. } => session_id,
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub request_id: String,
    pub owner_id: String,
    pub revision: u64,
}
impl Identity {
    fn matches(&self, other: &Self) -> bool {
        self.request_id == other.request_id && self.owner_id == other.owner_id
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StateEvent {
    #[serde(flatten)]
    identity: IdentityEvent,
    phase: String,
    bytes_received: u64,
    total_bytes: Option<u64>,
    error: Option<PreviewError>,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct IdentityEvent {
    request_id: String,
    owner_id: String,
    revision: u64,
}

struct Request {
    identity: Identity,
    target: Target,
    window: String,
    cancel: CancellationToken,
    native_closed: AtomicBool,
    snapshot: Mutex<StateEvent>,
    last_progress: Mutex<Instant>,
}
impl Request {
    fn emit(
        &self,
        app: &tauri::AppHandle,
        phase: &str,
        bytes: u64,
        total: Option<u64>,
        error: Option<PreviewError>,
    ) {
        let event = {
            let mut state = self.snapshot.lock().unwrap();
            state.phase = phase.into();
            state.bytes_received = bytes;
            state.total_bytes = total;
            state.error = error;
            state.clone()
        };
        let _ = app.emit_to(&self.window, "quicklook:state", event);
    }
    fn progress(&self, app: &tauri::AppHandle, bytes: u64, total: Option<u64>) {
        let mut last = self.last_progress.lock().unwrap();
        if last.elapsed() < Duration::from_millis(100) && bytes != 0 && Some(bytes) != total {
            return;
        }
        *last = Instant::now();
        self.emit(app, "downloading", bytes, total, None);
    }
}

#[derive(Default)]
struct WindowState {
    revision: u64,
    current: Option<Arc<Request>>,
    // Held through native close and staging cleanup, including panel lifetime.
    writer: Arc<AsyncMutex<()>>,
}
impl WindowState {
    fn open(&mut self, request: Arc<Request>) -> bool {
        if request.identity.revision <= self.revision {
            return false;
        }
        self.revision = request.identity.revision;
        if let Some(previous) = self.current.replace(request) {
            previous.cancel.cancel();
        }
        true
    }
    fn close(&mut self, identity: &Identity) {
        if identity.revision <= self.revision {
            return;
        }
        if self
            .current
            .as_ref()
            .is_some_and(|r| !r.identity.matches(identity))
        {
            return;
        }
        self.revision = identity.revision;
        if let Some(current) = self.current.take() {
            current.cancel.cancel();
        }
    }
}

async fn with_deadline<T>(
    cancel: &CancellationToken,
    deadline: tokio::time::Instant,
    work: impl std::future::Future<Output = Result<T, PreviewError>>,
) -> Result<T, PreviewError> {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => Err(PreviewError::new("cancelled", "Preview cancelled")),
        _ = tokio::time::sleep_until(deadline) => Err(PreviewError::new("timeout", "Quick Look preparation exceeded 60 seconds")),
        result = work => result,
    }
}

pub struct QuickLookManager {
    windows: Mutex<HashMap<String, WindowState>>,
    staging: staging::StagingRoot,
    shutting_down: AtomicBool,
}
impl QuickLookManager {
    pub fn new(cache: &std::path::Path) -> Result<Self, PreviewError> {
        Ok(Self {
            windows: Mutex::new(HashMap::new()),
            staging: staging::StagingRoot::new(cache)?,
            shutting_down: AtomicBool::new(false),
        })
    }

    pub fn open(
        self: &Arc<Self>,
        app: tauri::AppHandle,
        window: String,
        identity: Identity,
        target: Target,
    ) -> Result<bool, PreviewError> {
        if !cfg!(target_os = "macos") {
            return Err(PreviewError::new(
                "unsupported_platform",
                "Quick Look is available on macOS",
            ));
        }
        if self.shutting_down.load(Ordering::SeqCst) {
            return Err(PreviewError::new(
                "session_unavailable",
                "Application is closing",
            ));
        }
        target.name()?;
        if identity.owner_id.is_empty()
            || uuid::Uuid::parse_str(&identity.request_id).is_err()
            || identity.revision == 0
            || identity.revision > 9_007_199_254_740_991
        {
            return Err(PreviewError::new(
                "invalid_request",
                "Invalid preview request identity",
            ));
        }
        let snapshot = StateEvent {
            identity: IdentityEvent {
                request_id: identity.request_id.clone(),
                owner_id: identity.owner_id.clone(),
                revision: identity.revision,
            },
            phase: "preparing".into(),
            bytes_received: 0,
            total_bytes: None,
            error: None,
        };
        let request = Arc::new(Request {
            identity,
            target,
            window: window.clone(),
            cancel: CancellationToken::new(),
            native_closed: AtomicBool::new(false),
            snapshot: Mutex::new(snapshot),
            last_progress: Mutex::new(Instant::now()),
        });
        let writer = {
            let mut windows = self.windows.lock().unwrap();
            let state = windows.entry(window).or_default();
            if !state.open(request.clone()) {
                return Ok(false);
            }
            state.writer.clone()
        };
        let deadline = tokio::time::Instant::now() + DEADLINE;
        request.emit(&app, "preparing", 0, None, None);
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.run(app, request, writer, deadline).await;
        });
        Ok(true)
    }

    pub fn close(&self, window: &str, identity: &Identity) {
        self.windows
            .lock()
            .unwrap()
            .entry(window.into())
            .or_default()
            .close(identity);
    }

    fn current(&self, request: &Request) -> bool {
        !request.cancel.is_cancelled()
            && self
                .windows
                .lock()
                .unwrap()
                .get(&request.window)
                .and_then(|s| s.current.as_ref())
                .is_some_and(|r| r.identity.matches(&request.identity))
    }

    pub fn snapshot(&self, window: &str) -> Option<StateEvent> {
        self.windows
            .lock()
            .unwrap()
            .get(window)
            .and_then(|s| s.current.as_ref())
            .map(|r| r.snapshot.lock().unwrap().clone())
    }

    pub fn close_window(&self, window: &str) {
        if let Some(state) = self.windows.lock().unwrap().get_mut(window) {
            if let Some(request) = state.current.take() {
                request.cancel.cancel();
            }
        }
    }

    pub fn close_session(&self, id: &str) {
        for state in self.windows.lock().unwrap().values_mut() {
            if state
                .current
                .as_ref()
                .is_some_and(|r| r.target.session_id() == id)
            {
                if let Some(request) = state.current.take() {
                    request.cancel.cancel();
                }
            }
        }
    }

    pub fn begin_shutdown(&self) -> bool {
        if self.shutting_down.swap(true, Ordering::SeqCst) {
            return false;
        }
        for state in self.windows.lock().unwrap().values() {
            if let Some(request) = &state.current {
                request.cancel.cancel();
            }
        }
        true
    }

    pub async fn wait_for_cleanup(&self) {
        let gates: Vec<_> = self
            .windows
            .lock()
            .unwrap()
            .values()
            .map(|s| s.writer.clone())
            .collect();
        for gate in gates {
            let _guard = gate.lock().await;
        }
        self.staging.cleanup();
    }

    async fn run(
        self: Arc<Self>,
        app: tauri::AppHandle,
        request: Arc<Request>,
        writer: Arc<AsyncMutex<()>>,
        deadline: tokio::time::Instant,
    ) {
        // A queued request also consumes its original deadline. Do not spawn a
        // second writer until the predecessor has detached the native panel.
        let guard =
            match with_deadline(&request.cancel, deadline, async { Ok(writer.lock().await) }).await
            {
                Ok(guard) => guard,
                Err(error) => {
                    request.cancel.cancel();
                    let phase = if error.kind == "cancelled" {
                        "cancelled"
                    } else {
                        "error"
                    };
                    request.emit(&app, phase, 0, None, (phase == "error").then_some(error));
                    self.retire(&request);
                    return;
                }
            };
        let mut staged = None;
        let preparing = async {
            if !self.current(&request) {
                return Err(PreviewError::new("cancelled", "Preview cancelled"));
            }
            let file = self.staging.request(request.target.name()?)?;
            staged = Some(file);
            let file = staged.as_ref().unwrap();
            staging::download(&app, &request.target, file, |bytes, total| {
                request.progress(&app, bytes, total)
            })
            .await?;
            if !self.current(&request) {
                return Err(PreviewError::new("cancelled", "Preview cancelled"));
            }
            self.show(&app, request.clone(), file.complete.clone())
                .await
        };
        let result = with_deadline(&request.cancel, deadline, preparing).await;
        let phase = match &result {
            Ok(()) if self.current(&request) => {
                let state = request.snapshot.lock().unwrap().clone();
                request.emit(
                    &app,
                    "visible",
                    state.bytes_received,
                    state.total_bytes,
                    None,
                );
                request.cancel.cancelled().await;
                if request.native_closed.load(Ordering::SeqCst) {
                    "closed"
                } else {
                    "cancelled"
                }
            }
            Ok(()) => "cancelled",
            Err(error) if error.kind == "cancelled" => "cancelled",
            Err(_) => "error",
        };
        request.cancel.cancel();
        // Await the main-thread detach, then remove the files, then release the
        // writer gate. A stale main-thread show checks cancellation again.
        Self::hide(&app, request.identity.request_id.clone()).await;
        drop(staged);
        let error = result.err().filter(|e| e.kind != "cancelled");
        let state = request.snapshot.lock().unwrap().clone();
        request.emit(&app, phase, state.bytes_received, state.total_bytes, error);
        self.retire(&request);
        drop(guard);
    }

    fn retire(&self, request: &Request) {
        let mut windows = self.windows.lock().unwrap();
        if let Some(state) = windows.get_mut(&request.window) {
            if state
                .current
                .as_ref()
                .is_some_and(|r| r.identity.matches(&request.identity))
            {
                state.current = None;
            }
        }
    }

    #[cfg(target_os = "macos")]
    async fn show(
        self: &Arc<Self>,
        app: &tauri::AppHandle,
        request: Arc<Request>,
        path: std::path::PathBuf,
    ) -> Result<(), PreviewError> {
        let (send, receive) = tokio::sync::oneshot::channel();
        let manager = self.clone();
        let handle = app.clone();
        app.run_on_main_thread(move || {
            let result = if !manager.current(&request) {
                Err(PreviewError::new("cancelled", "Preview cancelled"))
            } else if let Some(window) = handle.get_webview_window(&request.window) {
                let closed = request.clone();
                macos::show(
                    &window,
                    request.identity.request_id.clone(),
                    &path,
                    Box::new(move || {
                        closed.native_closed.store(true, Ordering::SeqCst);
                        closed.cancel.cancel();
                    }),
                )
                .map_err(|e| PreviewError::new("native_panel", e))
            } else {
                Err(PreviewError::new("cancelled", "Preview window closed"))
            };
            let _ = send.send(result);
        })
        .map_err(|e| PreviewError::new("native_panel", e))?;
        receive
            .await
            .map_err(|e| PreviewError::new("native_panel", e))?
    }

    #[cfg(not(target_os = "macos"))]
    async fn show(
        self: &Arc<Self>,
        _app: &tauri::AppHandle,
        _request: Arc<Request>,
        _path: std::path::PathBuf,
    ) -> Result<(), PreviewError> {
        Err(PreviewError::new(
            "unsupported_platform",
            "Quick Look requires macOS",
        ))
    }

    async fn hide(app: &tauri::AppHandle, id: String) {
        let (send, receive) = tokio::sync::oneshot::channel();
        if app
            .run_on_main_thread(move || {
                #[cfg(target_os = "macos")]
                macos::close(Some(&id));
                #[cfg(not(target_os = "macos"))]
                let _ = id;
                let _ = send.send(());
            })
            .is_ok()
        {
            let _ = receive.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(owner: &str, request: &str, revision: u64) -> Identity {
        Identity {
            owner_id: owner.into(),
            request_id: request.into(),
            revision,
        }
    }
    fn request(owner: &str, id: &str, revision: u64) -> Arc<Request> {
        Arc::new(Request {
            identity: identity(owner, id, revision),
            window: "test".into(),
            target: Target::Sftp {
                session_id: "session".into(),
                path: "/file.txt".into(),
            },
            cancel: CancellationToken::new(),
            native_closed: AtomicBool::new(false),
            last_progress: Mutex::new(Instant::now()),
            snapshot: Mutex::new(StateEvent {
                identity: IdentityEvent {
                    owner_id: owner.into(),
                    request_id: id.into(),
                    revision,
                },
                phase: "preparing".into(),
                bytes_received: 0,
                total_bytes: None,
                error: None,
            }),
        })
    }
    #[test]
    fn close_before_old_open_leaves_a_revision_tombstone() {
        let mut window = WindowState::default();
        window.close(&identity("owner", "A", 2));
        assert!(!window.open(request("owner", "A", 1)));
        assert!(window.current.is_none());
    }
    #[test]
    fn replacement_cancels_old_request_and_stale_cleanup_cannot_close_new_owner() {
        let mut window = WindowState::default();
        let a = request("old", "A", 1);
        let b = request("new", "B", 2);
        assert!(window.open(a.clone()));
        assert!(window.open(b.clone()));
        assert!(a.cancel.is_cancelled());
        assert!(!b.cancel.is_cancelled());
        window.close(&identity("old", "A", 3));
        assert!(!b.cancel.is_cancelled());
        assert_eq!(window.current.as_ref().unwrap().identity.request_id, "B");
        window.close(&identity("new", "B", 4));
        window.close(&identity("new", "B", 4));
        assert!(b.cancel.is_cancelled());
        assert!(window.current.is_none());
        assert!(!window.open(request("old", "A", 3)));
    }
    #[test]
    fn unsafe_names_never_become_local_paths() {
        for path in ["/a/..", "/a/.", "/a/", "/a/a\\b", "/a/\0x"] {
            assert!(Target::Sftp {
                session_id: "s".into(),
                path: path.into()
            }
            .name()
            .is_err());
        }
        assert_eq!(
            Target::S3 {
                session_id: "s".into(),
                bucket_name: "b".into(),
                key: "a/b/測試.txt".into()
            }
            .name()
            .unwrap(),
            "測試.txt"
        );
    }
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[tokio::test(start_paused = true)]
    async fn preparation_deadline_interrupts_stalled_io_and_gate_wait() {
        let cancel = CancellationToken::new();
        let start = tokio::time::Instant::now();
        let result: Result<(), _> =
            with_deadline(&cancel, start + DEADLINE, std::future::pending()).await;
        assert_eq!(result.unwrap_err().kind, "timeout");
        assert_eq!(tokio::time::Instant::now() - start, DEADLINE);
        // Time already spent queued cannot be reset by the next stage.
        let result = with_deadline(&cancel, start + DEADLINE, async { Ok(()) }).await;
        assert_eq!(result.unwrap_err().kind, "timeout");
    }
    #[tokio::test]
    async fn cancelled_ready_completion_never_wins() {
        let cancel = CancellationToken::new();
        cancel.cancel();
        let result = with_deadline(&cancel, tokio::time::Instant::now() + DEADLINE, async {
            Ok(())
        })
        .await;
        assert_eq!(result.unwrap_err().kind, "cancelled");
    }
}
