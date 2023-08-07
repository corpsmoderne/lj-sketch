mod gen_server;
mod ws_client;

use axum::{
    extract::{
        ws::WebSocketUpgrade,
        TypedHeader,
    },
    response::IntoResponse,
    routing::get,
    Router,
    Extension
};
use std::{net::SocketAddr, path::PathBuf};
use tower_http::{
    services::ServeDir,
    trace::{DefaultMakeSpan, TraceLayer},
};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};
use axum::extract::connect_info::ConnectInfo;

use std::sync::Arc;
use tokio::sync::{
    mpsc:: { self, Sender, Receiver },
    Mutex
};
use gen_server::{State,GSMsg,gen_server};

const LISTEN_ON : &str = "0.0.0.0:3000";

#[tokio::main]
async fn main() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "lj_sketch=info,tower_http=info"
				.into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();
    
    let assets_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets");

    let (gs_tx, gs_rx) : (Sender<GSMsg>, Receiver<GSMsg>) = mpsc::channel(32);
    
    let state = Arc::new(Mutex::new(State { gs_tx }));
    
    tokio::spawn(gen_server(gs_rx));
    
    let app = Router::new()
        .fallback_service(ServeDir::new(assets_dir)
			  .append_index_html_on_directories(true))
        .route("/ws", get(ws_handler))	
	.layer(Extension(state))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::default()
				.include_headers(false)),
        );
        
    let addr : SocketAddr = LISTEN_ON.parse().unwrap();
    
    tracing::info!("listening on {}", addr);
    axum::Server::bind(&addr)
        .serve(app.into_make_service_with_connect_info::<SocketAddr>())
        .await
        .unwrap();
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Extension(state): Extension<Arc<Mutex<State>>>,
    user_agent: Option<TypedHeader<axum::headers::UserAgent>>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
) -> impl IntoResponse {
    let user_agent = if let Some(TypedHeader(user_agent)) = user_agent {
        user_agent.to_string()
    } else {
        String::from("Unknown browser")
    };
    tracing::info!("`{user_agent}` at {addr} connected.");
    // finalize the upgrade process by returning upgrade callback.
    // we can customize the callback by sending additional info such as address.
    ws.on_upgrade(move |socket| ws_client::handle_socket(socket, addr, state))
}

