use axum::{
    extract::{
        ws::{ Message, Message::Text, Message::Close,
	      WebSocket, WebSocketUpgrade},
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

//allows to extract the IP of connecting user
use axum::extract::{
    connect_info::ConnectInfo,
    //ws::CloseFrame
};

//allows to split the websocket stream into separate TX and RX branches
use futures::sink::SinkExt;
use futures::stream::{SplitSink,StreamExt};
use std::sync::Arc;
use tokio::sync::{
    mpsc:: { self, Sender, Receiver },
    Mutex
};
use serde::{Serialize,Deserialize};
use geo::Simplify;

use std::collections::HashMap;

const LISTEN_ON : &str = "0.0.0.0:3000";

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
enum JMsg {
    #[serde(rename = "clear")]     
    Clear,
    #[serde(rename = "moveTo")] 
    MoveTo { x: f32, y: f32, color: String },
    #[serde(rename = "lineTo")]     
    LineTo { x: f32, y: f32, color: String },
    #[serde(rename = "stroke")]     
    Stroke,
    #[serde(rename = "line")]     
    Line { line: Vec<(f32,f32,String)> }
}

type Line = Vec<(f32,f32,u32)>;

#[derive(Debug)]
enum GSMsg {
    NewClient((SocketAddr,SplitSink<WebSocket, Message>)),
    NewLine(Line),
    DeleteClient(SocketAddr),
    Clear
}


struct State {
    gs_tx: Sender<GSMsg>
}

async fn gen_server(mut rx: Receiver<GSMsg>) {
    let mut clients : HashMap<SocketAddr, SplitSink<WebSocket, Message>> =
	HashMap::new();
    
    let mut lines : Vec<Line> = vec![];
    
    while let Some(msg) = rx.recv().await {
	match msg {
	    GSMsg::NewClient((addr, mut tx)) => {
		for line in &lines {
		    tx
			.send(Message::Text(line_to_json(&line)))
			.await
			.unwrap();		    
		}
		clients.insert(addr, tx);
		tracing::info!("NewClient {addr}");
	    },
	    GSMsg::NewLine(line) => {
		let msg = line_to_json(&line);
		send_all(&mut clients, msg).await;
		lines.push(line);
	    },
	    GSMsg::DeleteClient(addr) => {
		tracing::info!("Client {addr} removed");		
		clients.remove(&addr);		
	    },
	    GSMsg::Clear => {
		let msg = serde_json::to_string(&JMsg::Clear).unwrap();
		send_all(&mut clients, msg).await;
		lines.clear();
	    }
	}
    }
}

async fn send_all(
    clients: &mut HashMap<SocketAddr, SplitSink<WebSocket, Message>>,
    msg: String
) {
    let mut to_remove : Vec<SocketAddr> = vec![];
		
    for (addr, ref mut tx) in &mut *clients {
	let ret = tx
	    .send(Message::Text(msg.clone()))
	    .await;
	if ret.is_err() {
	    tracing::warn!("Client {addr} abruptly disconnected");
	    to_remove.push(*addr);
	}
    }
    
    for addr in to_remove {
	clients.remove(&addr);
    }
}

fn line_to_json(line: &Line) -> String {
    let line = line.iter()
	.map(| (x, y, c) | {
	    (*x, *y, format!("#{:06x}", c))
	})
	.collect();    
    serde_json::to_string(&JMsg::Line{ line }).unwrap()
}


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

    let (tx, rx) : (Sender<GSMsg>, Receiver<GSMsg>) = mpsc::channel(32);
    
    let state = Arc::new(Mutex::new(State {
	gs_tx: tx
    }));
    
    tokio::spawn(gen_server(rx));
    
    // build our application with some routes
    let app = Router::new()
        .fallback_service(ServeDir::new(assets_dir)
			  .append_index_html_on_directories(true))
        .route("/ws", get(ws_handler))
	
	.layer(Extension(state))
    // logging so we can see whats going on
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
    ws.on_upgrade(move |socket| handle_socket(socket, addr, state))
}

async fn handle_socket(
    socket: WebSocket,
    who: SocketAddr,
    state: Arc<Mutex<State>>
) {
    let (tx, mut rx) = socket.split();
    {
	let st = state.lock().await;
	(*st).gs_tx.send(GSMsg::NewClient((who.clone(), tx))).await.unwrap();
    }
    let mut line : Line = vec![];

    while let Some(msg) = rx.next().await {
	match msg {
	    Ok(Text(msg)) => {
		let Ok(msg) : Result<JMsg,_> = serde_json::from_str(&msg) else {
		    tracing::warn!("{who}: Can't parse JSON: {:?}", msg);
		    continue;
		};
		tracing::debug!("{who}: '{:?}'", msg);
		match msg {
		    JMsg::Clear => {
			let st = state.lock().await;
			(*st).gs_tx.send(GSMsg::Clear)
			    .await.unwrap();			
			line.clear();
		    },
		    JMsg::MoveTo { x, y, color } => {
			line = vec![ (x, y, parse_color(color)) ];
		    },
		    JMsg::LineTo { x, y, color } => {
			line.push( (x, y, parse_color(color)) );
		    },
		    JMsg::Stroke => {
			if line.len() > 1 {
			    let line = simplify_line(&line);
			    
			    let st = state.lock().await;
			    (*st).gs_tx.send(GSMsg::NewLine(line))
				.await.unwrap();				
			}
			line = vec![];
		    },
		    JMsg::Line{..} => { panic!("recieved a line message :/"); }
		}
	    },
	    Ok(Close(close)) => {
		tracing::info!("{who}: closing: {:?}", close);
		let st = state.lock().await;
		(*st).gs_tx.send(GSMsg::DeleteClient(who))
		    .await.unwrap();		
		break;
	    },
	    _ => {
		tracing::warn!("{who}: Can't handle message: {:?}", msg);
	    }
	}
    }
}

fn simplify_line(line: &Line) -> Line {
    if line.len() < 2 {
	return line.to_vec();
    }
    let color = line[0].2;
    let linestring : geo::LineString =
	line.iter()
	.map(| (x, y, _) | (*x as f64, *y as f64 ))
	.collect();
    let linestring = linestring.simplify(&4.0);
    linestring.0.iter()
	.map(| c | (c.x as f32, c.y as f32, color))
	.collect()
}

fn parse_color(s: String) -> u32 {
    u32::from_str_radix(&s[1..], 16).unwrap()
}
