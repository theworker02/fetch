use crate::{
    VERSION,
    config::FetchConfig,
    download::{DownloadRequest, Downloader},
    torrent,
};
use axum::{
    Json, Router,
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use tokio::sync::{RwLock, Semaphore, broadcast};
use url::Url;
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    jobs: Arc<RwLock<HashMap<Uuid, Job>>>,
    store: Arc<Mutex<Store>>,
    events: broadcast::Sender<Job>,
    slots: Arc<Semaphore>,
    tasks: Arc<Mutex<HashMap<Uuid, tokio::task::AbortHandle>>>,
    max_tries: usize,
    retry_wait_seconds: u64,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Job {
    gid: Uuid,
    status: JobStatus,
    sources: Vec<String>,
    output: PathBuf,
    error: Option<String>,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
enum JobStatus {
    Waiting,
    Active,
    Paused,
    Complete,
    Error,
    Removed,
}
#[derive(Deserialize)]
struct RpcRequest {
    jsonrpc: String,
    id: Value,
    method: String,
    #[serde(default)]
    params: Value,
}
#[derive(Serialize)]
struct RpcResponse {
    jsonrpc: &'static str,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<RpcError>,
}
#[derive(Serialize)]
struct RpcError {
    code: i32,
    message: String,
}
struct Store {
    connection: Connection,
}

impl Store {
    fn open() -> anyhow::Result<(Self, HashMap<Uuid, Job>)> {
        let connection = Connection::open("fetch.sqlite3")?;
        connection.execute_batch("CREATE TABLE IF NOT EXISTS jobs (gid TEXT PRIMARY KEY, status TEXT NOT NULL, sources TEXT NOT NULL, output TEXT NOT NULL, error TEXT);")?;
        connection.execute("UPDATE jobs SET status='error', error='daemon interrupted before completion' WHERE status='active'", [])?;
        let mut statement =
            connection.prepare("SELECT gid,status,sources,output,error FROM jobs")?;
        let jobs = statement
            .query_map([], |row| {
                let status = match row.get::<_, String>(1)?.as_str() {
                    "active" => JobStatus::Active,
                    "waiting" => JobStatus::Waiting,
                    "paused" => JobStatus::Paused,
                    "complete" => JobStatus::Complete,
                    "removed" => JobStatus::Removed,
                    _ => JobStatus::Error,
                };
                let gid = Uuid::parse_str(&row.get::<_, String>(0)?)
                    .map_err(|error| rusqlite::Error::ToSqlConversionFailure(Box::new(error)))?;
                Ok(Job {
                    gid,
                    status,
                    sources: serde_json::from_str(&row.get::<_, String>(2)?).unwrap_or_default(),
                    output: PathBuf::from(row.get::<_, String>(3)?),
                    error: row.get(4)?,
                })
            })?
            .filter_map(Result::ok)
            .map(|job| (job.gid, job))
            .collect();
        drop(statement);
        Ok((Self { connection }, jobs))
    }
    fn save(&self, job: &Job) -> anyhow::Result<()> {
        self.connection.execute("INSERT INTO jobs(gid,status,sources,output,error) VALUES(?1,?2,?3,?4,?5) ON CONFLICT(gid) DO UPDATE SET status=excluded.status,sources=excluded.sources,output=excluded.output,error=excluded.error", params![job.gid.to_string(), status_name(&job.status), serde_json::to_string(&job.sources)?, job.output.to_string_lossy(), job.error])?;
        Ok(())
    }
}

pub async fn serve(address: SocketAddr) -> anyhow::Result<()> {
    serve_with_config(address, FetchConfig::default()).await
}

pub async fn serve_with_config(address: SocketAddr, config: FetchConfig) -> anyhow::Result<()> {
    if !address.ip().is_loopback() {
        anyhow::bail!(
            "refusing non-loopback JSON-RPC listener; use an authenticated TLS proxy for remote control"
        );
    }
    let (store, jobs) = Store::open()?;
    let (events, _) = broadcast::channel(128);
    let state = AppState {
        jobs: Arc::new(RwLock::new(jobs)),
        store: Arc::new(Mutex::new(store)),
        events,
        slots: Arc::new(Semaphore::new(config.max_concurrent_downloads)),
        tasks: Arc::new(Mutex::new(HashMap::new())),
        max_tries: config.max_tries,
        retry_wait_seconds: config.retry_wait_seconds,
    };
    let app = Router::new()
        .route("/jsonrpc", post(rpc))
        .route("/events", get(websocket))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!("fetch daemon listening at http://{address}/jsonrpc (events: /events)");
    axum::serve(listener, app).await?;
    Ok(())
}
async fn websocket(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| event_socket(socket, state.events.subscribe()))
}
async fn event_socket(mut socket: WebSocket, mut events: broadcast::Receiver<Job>) {
    while let Ok(job) = events.recv().await {
        let Ok(payload) = serde_json::to_string(&json!({"event":"fetch.job","job":job})) else {
            continue;
        };
        if socket.send(Message::Text(payload.into())).await.is_err() {
            return;
        }
    }
}
async fn rpc(
    State(state): State<AppState>,
    Json(request): Json<RpcRequest>,
) -> (StatusCode, Json<RpcResponse>) {
    if request.jsonrpc != "2.0" {
        return response(request.id, Err((-32600, "jsonrpc must be 2.0")));
    }
    let outcome = match request.method.as_str() {
        "fetch.getVersion" | "aria2.getVersion" => Ok(
            json!({"version":VERSION,"enabledFeatures":["http","https","ftp","ftps","sftp","bittorrent","metalink","sha256","json-rpc","websocket-events","sqlite-state"]}),
        ),
        "fetch.tellStatus" | "aria2.tellStatus" => tell_status(&state, &request.params).await,
        "fetch.remove" | "aria2.remove" | "aria2.forceRemove" => {
            remove(&state, &request.params).await
        }
        "fetch.addUri" | "aria2.addUri" => add_uri(state, &request.params).await,
        "fetch.addTorrent" | "aria2.addTorrent" => add_torrent(state, &request.params).await,
        "aria2.tellActive" => list_jobs(&state, |status| matches!(status, JobStatus::Active)).await,
        "aria2.tellWaiting" => {
            list_jobs(&state, |status| matches!(status, JobStatus::Waiting)).await
        }
        "aria2.tellStopped" => {
            list_jobs(&state, |status| {
                !matches!(status, JobStatus::Active | JobStatus::Waiting)
            })
            .await
        }
        _ => Err((-32601, "method not found")),
    };
    response(request.id, outcome)
}
fn response(
    id: Value,
    outcome: Result<Value, (i32, &'static str)>,
) -> (StatusCode, Json<RpcResponse>) {
    match outcome {
        Ok(result) => (
            StatusCode::OK,
            Json(RpcResponse {
                jsonrpc: "2.0",
                id,
                result: Some(result),
                error: None,
            }),
        ),
        Err((code, message)) => (
            StatusCode::BAD_REQUEST,
            Json(RpcResponse {
                jsonrpc: "2.0",
                id,
                result: None,
                error: Some(RpcError {
                    code,
                    message: message.into(),
                }),
            }),
        ),
    }
}
async fn tell_status(state: &AppState, params: &Value) -> Result<Value, (i32, &'static str)> {
    let gid = parse_gid(params)?;
    let jobs = state.jobs.read().await;
    jobs.get(&gid)
        .map(|job| serde_json::to_value(job).map_err(|_| (-32603, "serialization failure")))
        .unwrap_or(Err((1, "unknown gid")))
}

async fn list_jobs(
    state: &AppState,
    include: impl Fn(&JobStatus) -> bool,
) -> Result<Value, (i32, &'static str)> {
    let jobs = state.jobs.read().await;
    serde_json::to_value(
        jobs.values()
            .filter(|job| include(&job.status))
            .collect::<Vec<_>>(),
    )
    .map_err(|_| (-32603, "serialization failure"))
}
async fn remove(state: &AppState, params: &Value) -> Result<Value, (i32, &'static str)> {
    let gid = parse_gid(params)?;
    let job = {
        let mut jobs = state.jobs.write().await;
        let job = jobs.get_mut(&gid).ok_or((1, "unknown gid"))?;
        job.status = JobStatus::Removed;
        job.clone()
    };
    if let Ok(mut tasks) = state.tasks.lock() {
        if let Some(task) = tasks.remove(&gid) {
            task.abort();
        }
    }
    persist(&state, job).map_err(|_| (-32603, "state persistence failed"))?;
    Ok(json!(gid))
}
async fn add_uri(state: AppState, params: &Value) -> Result<Value, (i32, &'static str)> {
    let values = params
        .as_array()
        .ok_or((-32602, "params must be [uris, output, splits?]"))?;
    let sources = values
        .first()
        .and_then(Value::as_array)
        .ok_or((-32602, "first parameter must be a URI array"))?
        .iter()
        .map(|v| {
            v.as_str()
                .ok_or((-32602, "URI must be a string"))
                .and_then(|s| Url::parse(s).map_err(|_| (-32602, "invalid URI")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let output = values
        .get(1)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or((-32602, "output path is required"))?;
    let splits = values
        .get(2)
        .and_then(Value::as_u64)
        .unwrap_or(4)
        .clamp(1, 64) as usize;
    let gid = Uuid::new_v4();
    let job = Job {
        gid,
        status: JobStatus::Waiting,
        sources: sources.iter().map(redact_url).collect(),
        output: output.clone(),
        error: None,
    };
    register(&state, job)
        .await
        .map_err(|_| (-32603, "state persistence failed"))?;
    start_http_job(state, gid, sources, output, splits);
    Ok(json!(gid))
}

fn start_http_job(state: AppState, gid: Uuid, sources: Vec<Url>, output: PathBuf, splits: usize) {
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let state = task_state;
        let Ok(_slot) = state.slots.clone().acquire_owned().await else {
            finish(&state, gid, Err(anyhow::anyhow!("scheduler shut down"))).await;
            return;
        };
        let should_run = {
            let mut jobs = state.jobs.write().await;
            let Some(job) = jobs.get_mut(&gid) else {
                return;
            };
            if !matches!(job.status, JobStatus::Waiting) {
                false
            } else {
                job.status = JobStatus::Active;
                let snapshot = job.clone();
                let _ = persist(&state, snapshot);
                true
            }
        };
        if !should_run {
            return;
        }
        let result = download_with_retry(&state, sources, output, splits).await;
        finish(&state, gid, result.map(|_| ())).await;
    });
    if let Ok(mut tasks) = state.tasks.lock() {
        tasks.insert(gid, task.abort_handle());
    }
}

async fn download_with_retry(
    state: &AppState,
    sources: Vec<Url>,
    output: PathBuf,
    splits: usize,
) -> anyhow::Result<()> {
    let mut last_error = None;
    for attempt in 1..=state.max_tries {
        match Downloader::new()
            .download(DownloadRequest {
                sources: sources.clone(),
                output: output.clone(),
                splits,
                sha256: None,
            })
            .await
        {
            Ok(_) => return Ok(()),
            Err(error) => last_error = Some(error),
        }
        if attempt < state.max_tries && state.retry_wait_seconds > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(state.retry_wait_seconds)).await;
        }
    }
    Err(last_error.expect("max_tries is validated positive"))
}
async fn add_torrent(state: AppState, params: &Value) -> Result<Value, (i32, &'static str)> {
    let values = params.as_array().ok_or((
        -32602,
        "params must be [magnet-or-torrent, outputDirectory]",
    ))?;
    let input = values
        .first()
        .and_then(Value::as_str)
        .ok_or((-32602, "torrent input is required"))?
        .to_owned();
    let output = values
        .get(1)
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or((-32602, "output directory is required"))?;
    let gid = Uuid::new_v4();
    register(
        &state,
        Job {
            gid,
            status: JobStatus::Waiting,
            sources: vec![input.clone()],
            output: output.clone(),
            error: None,
        },
    )
    .await
    .map_err(|_| (-32603, "state persistence failed"))?;
    let task_state = state.clone();
    let task = tokio::spawn(async move {
        let state = task_state;
        let Ok(_slot) = state.slots.clone().acquire_owned().await else {
            finish(&state, gid, Err(anyhow::anyhow!("scheduler shut down"))).await;
            return;
        };
        {
            let mut jobs = state.jobs.write().await;
            let Some(job) = jobs.get_mut(&gid) else {
                return;
            };
            if !matches!(job.status, JobStatus::Waiting) {
                return;
            }
            job.status = JobStatus::Active;
            let _ = persist(&state, job.clone());
        }
        finish(&state, gid, torrent::download(input, output).await).await;
    });
    if let Ok(mut tasks) = state.tasks.lock() {
        tasks.insert(gid, task.abort_handle());
    }
    Ok(json!(gid))
}
async fn register(state: &AppState, job: Job) -> anyhow::Result<()> {
    persist(state, job.clone())?;
    state.jobs.write().await.insert(job.gid, job);
    Ok(())
}
async fn finish(state: &AppState, gid: Uuid, result: anyhow::Result<()>) {
    if let Ok(mut tasks) = state.tasks.lock() {
        tasks.remove(&gid);
    }
    let job = {
        let mut jobs = state.jobs.write().await;
        let Some(job) = jobs.get_mut(&gid) else {
            return;
        };
        if !matches!(job.status, JobStatus::Removed) {
            match result {
                Ok(()) => job.status = JobStatus::Complete,
                Err(error) => {
                    job.status = JobStatus::Error;
                    job.error = Some(error.to_string())
                }
            }
        }
        job.clone()
    };
    let _ = persist(state, job);
}
fn persist(state: &AppState, job: Job) -> anyhow::Result<()> {
    state
        .store
        .lock()
        .map_err(|_| anyhow::anyhow!("state store poisoned"))?
        .save(&job)?;
    let _ = state.events.send(job);
    Ok(())
}
fn parse_gid(params: &Value) -> Result<Uuid, (i32, &'static str)> {
    let value = params
        .as_array()
        .and_then(|v| v.first())
        .and_then(Value::as_str)
        .ok_or((-32602, "gid is required"))?;
    Uuid::parse_str(value).map_err(|_| (-32602, "invalid gid"))
}
fn status_name(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Waiting => "waiting",
        JobStatus::Active => "active",
        JobStatus::Paused => "paused",
        JobStatus::Complete => "complete",
        JobStatus::Error => "error",
        JobStatus::Removed => "removed",
    }
}
fn redact_url(source: &Url) -> String {
    let mut source = source.clone();
    let _ = source.set_username("");
    let _ = source.set_password(None);
    source.to_string()
}
