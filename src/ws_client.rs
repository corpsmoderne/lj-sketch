use crate::gen_server::{State,GSMsg};

use axum::extract::ws::{ Message, Message::Text, Message::Close, WebSocket };
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::{
    mpsc:: { self, Sender, Receiver },
    Mutex
};
use serde::{Serialize,Deserialize};
use geo::Simplify;

use  core::ops::ControlFlow;

#[derive(Serialize, Deserialize, Debug)]
#[serde(tag = "t")]
pub enum JMsg {
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

pub type Line = Vec<(f32,f32,u32)>;

pub async fn handle_socket(
    mut socket: WebSocket,
    who: SocketAddr,
    state: Arc<Mutex<State>>
) {

    let (c_tx, mut c_rx) : (Sender<GSMsg>, Receiver<GSMsg>) = mpsc::channel(32);
    
    {
	state.lock()
	    .await
	    .gs_tx.send(GSMsg::NewClient((who, c_tx)))
	    .await.unwrap();
    }
    let mut line : Line = vec![];

    loop {
	tokio::select! {
	    Some(msg) = socket.recv() => {
		match process_ws_msg(&state, &who, &mut line, msg).await {
		    ControlFlow::Break(()) => { return; },
		    ControlFlow::Continue(()) => {}
		}
	    },
	    Some(msg) = c_rx.recv() => {
		match msg {
		    GSMsg::NewLine(line) => {
			socket.send(Message::Text(line_to_json(&line)))
			    .await.unwrap();
		    },
		    GSMsg::Clear => {
			let msg = serde_json::to_string(&JMsg::Clear).unwrap();
			socket.send(Message::Text(msg))
			    .await.unwrap();
		    },
		    msg => {
			tracing::info!("{who} should not get this: {:?}", msg)
		    }
		}
	    },
	    else => {
		tracing::warn!("{who}: Connection lost unexpectedly.");
		return;
	    }
	}
    }
}

async fn process_ws_msg(
    state: &Arc<Mutex<State>>,
    who: &SocketAddr,
    line: &mut Line,
    msg: Result<Message,axum::Error>
) -> ControlFlow<(),()> {
    match msg {
	Ok(Text(msg)) => {
	    let Ok(msg) : Result<JMsg,_> = serde_json::from_str(&msg) else {
		tracing::warn!("{who}: Can't parse JSON: {:?}", msg);
		return ControlFlow::Continue(());
	    };
	    tracing::debug!("{who}: '{:?}'", msg);
	    match msg {
		JMsg::Clear => {
		    state.lock()
			.await
			.gs_tx.send(GSMsg::Clear)
			.await.unwrap();			
		    line.clear();
		},
		JMsg::MoveTo { x, y, color } => {
		    *line = vec![ (x, y, parse_color(color)) ];
		},
		JMsg::LineTo { x, y, color } => {
		    line.push( (x, y, parse_color(color)) );
		},
		JMsg::Stroke => {
		    if line.len() > 1 {
			state.lock()
			    .await
			    .gs_tx.send(GSMsg::NewLine(simplify_line(line)))
			    .await.unwrap();		
		    }
		    *line = vec![];
		},
		JMsg::Line{..} => { panic!("recieved a line message :/"); }
	    }
	},
	Ok(Close(close)) => {
	    tracing::info!("{who}: closing: {:?}", close);
	    state.lock()
		.await
		.gs_tx.send(GSMsg::DeleteClient(*who))
		.await.unwrap();		
	    return ControlFlow::Break(());
	},
	_ => {
	    tracing::warn!("{who}: Can't handle message: {:?}", msg);
	}
    }
    ControlFlow::Continue(())
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


fn line_to_json(line: &Line) -> String {
    let line = line.iter()
	.map(| (x, y, c) | {
	    (*x, *y, format!("#{:06x}", c))
	})
	.collect();    
    serde_json::to_string(&JMsg::Line{ line }).unwrap()
}

fn parse_color(s: String) -> u32 {
    u32::from_str_radix(&s[1..], 16).unwrap()
}
