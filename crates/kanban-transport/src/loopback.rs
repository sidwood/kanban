//! Optional authenticated Streamable HTTP; application authority is service-bound.
use http_body_util::{BodyExt, Full, Limited, combinators::BoxBody};
use hyper::{
    Request, Response,
    body::{Bytes, Incoming},
    service::service_fn,
};
use hyper_util::rt::{TokioIo, TokioTimer};
use kanban_app::secrets::InstallationSecret;
use kanban_dto::ApiError;
use rmcp::transport::streamable_http_server::{
    StreamableHttpServerConfig, StreamableHttpService, session::local::LocalSessionManager,
};
use std::{
    convert::Infallible,
    io,
    net::{SocketAddr, TcpListener},
    sync::Arc,
    thread::JoinHandle,
    time::Duration,
};
use subtle::ConstantTimeEq;
use tokio_util::sync::CancellationToken;

type Body = BoxBody<Bytes, Infallible>;

#[derive(Clone, Copy, Debug, Default)]
pub struct LoopbackHttpConfig {
    pub bind: Option<SocketAddr>,
}

pub struct LoopbackHttp {
    address: Option<SocketAddr>,
    stop: CancellationToken,
    worker: Option<JoinHandle<io::Result<()>>>,
}

/// Run-scoped agent MCP and optional installation operator MCP.
pub struct LoopbackHttpHandlers<S> {
    agent: Arc<dyn Fn(u64) -> Result<S, ApiError> + Send + Sync>,
    operator: Option<Arc<dyn Fn() -> Result<S, ApiError> + Send + Sync>>,
}

impl<S> LoopbackHttpHandlers<S> {
    pub fn agent_only(agent: impl Fn(u64) -> Result<S, ApiError> + Send + Sync + 'static) -> Self {
        Self {
            agent: Arc::new(agent),
            operator: None,
        }
    }

    pub fn with_operator(
        agent: impl Fn(u64) -> Result<S, ApiError> + Send + Sync + 'static,
        operator: impl Fn() -> Result<S, ApiError> + Send + Sync + 'static,
    ) -> Self {
        Self {
            agent: Arc::new(agent),
            operator: Some(Arc::new(operator)),
        }
    }
}

impl LoopbackHttp {
    pub fn start<S: rmcp::ServerHandler + Clone + 'static>(
        config: LoopbackHttpConfig,
        secret: Option<Arc<InstallationSecret>>,
        session: impl Fn(u64) -> Result<S, ApiError> + Send + Sync + 'static,
    ) -> io::Result<Self> {
        Self::start_with_handlers(config, secret, LoopbackHttpHandlers::agent_only(session))
    }

    pub fn start_with_handlers<S: rmcp::ServerHandler + Clone + 'static>(
        config: LoopbackHttpConfig,
        secret: Option<Arc<InstallationSecret>>,
        handlers: LoopbackHttpHandlers<S>,
    ) -> io::Result<Self> {
        let mut handle = Self {
            address: None,
            stop: CancellationToken::new(),
            worker: None,
        };
        let Some(address) = config.bind else {
            return Ok(handle);
        };
        if !address.ip().is_loopback() {
            return Err(io::Error::other("HTTP requires a loopback address"));
        }
        let secret =
            secret.ok_or_else(|| io::Error::other("HTTP authentication is unavailable"))?;
        let listener = TcpListener::bind(address)?;
        listener.set_nonblocking(true)?;
        let bound = listener.local_addr()?;
        handle.address = Some(bound);
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let listener = {
            let _entered = runtime.enter();
            tokio::net::TcpListener::from_std(listener)?
        };
        let stop = handle.stop.clone();
        let handlers = Arc::new(handlers);
        handle.worker = Some(std::thread::Builder::new().name("kanban-http".into()).spawn(move || {
            runtime.block_on(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        biased;
                        _ = stop.cancelled() => break,
                        Some(_) = connections.join_next() => {},
                        incoming = listener.accept() => {
                            let (stream, _) = incoming?;
                            let secret = secret.clone();
                            let handlers = handlers.clone();
                            let stop = stop.clone();
                            connections.spawn(async move {
                                let service = service_fn(move |request| {
                                    respond(request, secret.clone(), handlers.clone(), stop.clone(), bound)
                                });
                                let _ = hyper::server::conn::http1::Builder::new()
                                    .timer(TokioTimer::new())
                                    .header_read_timeout(Duration::from_secs(2))
                                    .serve_connection(TokioIo::new(stream), service).await;
                            });
                        }
                    }
                }
                connections.abort_all();
                while connections.join_next().await.is_some() {}
                Ok(())
            })
        })?);
        Ok(handle)
    }
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.address
    }
    pub fn shutdown(mut self) -> io::Result<()> {
        self.close()
    }
    fn close(&mut self) -> io::Result<()> {
        self.stop.cancel();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| io::Error::other("HTTP worker failed"))??;
        }
        Ok(())
    }
}
impl Drop for LoopbackHttp {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

async fn respond<S: rmcp::ServerHandler + Clone + 'static>(
    request: Request<Incoming>,
    secret: Arc<InstallationSecret>,
    handlers: Arc<LoopbackHttpHandlers<S>>,
    stop: CancellationToken,
    bound: SocketAddr,
) -> Result<Response<Body>, Infallible> {
    if request.headers().contains_key("origin")
        || request.headers().get_all("host").iter().count() != 1
        || request.headers().get("host").and_then(|h| h.to_str().ok())
            != Some(bound.to_string().as_str())
    {
        return Ok(refused(403));
    }
    if request.headers().get_all("authorization").iter().count() > 1
        || request
            .headers()
            .get_all("x-kanban-capability")
            .iter()
            .count()
            > 1
    {
        return Ok(refused(400));
    }
    let credential = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .unwrap_or("");
    if !bool::from(credential.as_bytes().ct_eq(secret.expose().as_bytes())) {
        return Ok(refused(401));
    }
    let path = request.uri().path();
    if path == "/mcp"
        && request
            .uri()
            .query()
            .is_some_and(|query| query.split('&').any(|part| part.starts_with("operator=")))
    {
        return Ok(refused(404));
    }
    let operator_route = path == "/mcp/operator";
    if path != "/mcp" && !operator_route {
        return Ok(refused(404));
    }
    if request.method() != hyper::Method::POST {
        let mut response = refused(405);
        response
            .headers_mut()
            .insert("allow", "POST".parse().expect("static method"));
        return Ok(response);
    }
    if request.headers().iter().any(|(name, value)| {
        name != "authorization"
            && value
                .as_bytes()
                .windows(secret.expose().len())
                .any(|part| part == secret.expose().as_bytes())
    }) {
        return Ok(refused(400));
    }
    let (mut parts, body) = request.into_parts();
    parts.headers.remove("authorization");
    let body = match tokio::time::timeout(
        Duration::from_secs(2),
        Limited::new(body, 4 * 1024 * 1024).collect(),
    )
    .await
    {
        Ok(Ok(body)) => body,
        Ok(Err(_)) => return Ok(refused(413)),
        Err(_) => return Ok(refused(408)),
    };
    let bytes = body.to_bytes();
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Ok(refused(400));
    };
    let normalized = value.to_string();
    if normalized.contains(secret.expose())
        || bytes
            .windows(secret.expose().len())
            .any(|part| part == secret.expose().as_bytes())
    {
        return Ok(refused(400));
    }
    // Forward exactly what was checked, not shadowed fields from the raw envelope.
    parts.headers.remove("content-length");
    let request = Request::from_parts(parts, Full::new(Bytes::from(normalized)));
    let adapter = if operator_route {
        if request.headers().contains_key("x-kanban-capability") {
            return Ok(refused(400));
        }
        let Some(operator) = handlers.operator.as_ref() else {
            return Ok(refused(404));
        };
        match operator() {
            Ok(adapter) => adapter,
            Err(_) => return Ok(refused(403)),
        }
    } else {
        let capability = request
            .headers()
            .get("x-kanban-capability")
            .and_then(|h| h.to_str().ok())
            .filter(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|s| s.parse::<u64>().ok())
            .filter(|id| *id != 0);
        let Some(capability) = capability else {
            return Ok(refused(400));
        };
        match (handlers.agent)(capability) {
            Ok(adapter) => adapter,
            Err(_) => return Ok(refused(403)),
        }
    };
    let service: StreamableHttpService<S, LocalSessionManager> = StreamableHttpService::new(
        move || Ok(adapter.clone()),
        Default::default(),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .with_sse_keep_alive(None)
            .with_cancellation_token(stop),
    );
    let response = service.handle(request).await;
    let (mut parts, body) = response.into_parts();
    let Ok(body) = Limited::new(body, 4 * 1024 * 1024).collect().await else {
        return Ok(refused(500));
    };
    let bytes = body.to_bytes();
    if bytes
        .windows(secret.expose().len())
        .any(|part| part == secret.expose().as_bytes())
        || serde_json::from_slice::<serde_json::Value>(&bytes)
            .is_ok_and(|value| value.to_string().contains(secret.expose()))
    {
        return Ok(refused(500));
    }
    parts.headers.remove("content-length");
    Ok(Response::from_parts(parts, Full::new(bytes).boxed()))
}
fn refused(status: u16) -> Response<Body> {
    let mut builder = Response::builder().status(status);
    if status == 401 {
        builder = builder.header("www-authenticate", "Bearer realm=\"Kanban\"");
    }
    builder
        .body(Full::new(Bytes::from_static(b"request refused")).boxed())
        .expect("static response")
}
