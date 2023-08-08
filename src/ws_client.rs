use axum::extract::ws::{ Message, Message::Text, Message::Close, WebSocket };
use std::net::SocketAddr;
use tokio::sync::mpsc::{self, Sender};
use serde::{Serialize,Deserialize};
use  core::ops::ControlFlow;
use crate::gen_server::GSMsg;
use crate::line::{Line,simplify_line};
		  
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

pub async fn handle_socket(
    mut socket: WebSocket,
    who: SocketAddr,
    gs_tx: Sender<GSMsg>
) {
    let (c_tx, mut c_rx) = mpsc::channel(32); 
    gs_tx.send(GSMsg::NewClient((who, c_tx))).await.unwrap();
    let mut line : Line = vec![];

    loop {
	tokio::select! {
	    Some(msg) = socket.recv() => {
		let Ok(msg) = msg else {
		    tracing::warn!("{who}: Error receiving packet: {msg:?}");
		    continue;
		};
		match process_ws_msg(&gs_tx, &who, &mut line, msg).await {
		    ControlFlow::Break(()) => break,
		    ControlFlow::Continue(()) => {}
		}
	    },
	    Some(msg) = c_rx.recv() => {
		process_gs_msg(&mut socket, &who, msg).await
	    },
	    else => {
		tracing::warn!("{who}: Connection lost unexpectedly.");
		break;
	    }
	}
    }
}

async fn process_gs_msg(socket: &mut WebSocket, who: &SocketAddr, msg: GSMsg) {
    match msg {
	GSMsg::NewLine(line) => {
	    socket.send(Message::Text(line_to_json(&line))).await.unwrap();
	},
	GSMsg::Clear => {
	    let msg = serde_json::to_string(&JMsg::Clear).unwrap();
	    socket.send(Message::Text(msg)).await.unwrap();
	},
	msg => {
	    tracing::info!("{who} should not get this: {:?}", msg);
	}
    }   
}

async fn process_ws_msg(
    gs_tx: &Sender<GSMsg>,
    who: &SocketAddr,
    line: &mut Line,
    msg: Message
) -> ControlFlow<(),()> {
    match msg {
	Text(text) => {
	    match serde_json::from_str(&text) {
		Ok(json) => {
		    tracing::debug!("{who}: '{:?}'", json);
		    match handle_ws_msg(line, json) {
			Ok(Some(req)) => gs_tx.send(req).await.unwrap(),
			Ok(None) => {},
			Err(err) => {
			    tracing::warn!("{who}: message error: {err}");
			}
		    }
		},
		Err(err) => {
		    tracing::warn!("{who}: can't parse JSON: {err}");
		}
	    }
	},
	Close(close) => {
	    tracing::info!("{who}: closing: {:?}", close);
	    gs_tx.send(GSMsg::DeleteClient(*who)).await.unwrap();
	    return ControlFlow::Break(());
	},
	_ => {
	    tracing::warn!("{who}: can't handle message: {:?}", msg);
	}
    }
    ControlFlow::Continue(())
}

fn handle_ws_msg(
    line: &mut Line,
    msg: JMsg
) -> Result<Option<GSMsg>, &'static str> {
    match msg {
	JMsg::Clear => {
	    line.clear();
	    return Ok(Some(GSMsg::Clear));
	},
	JMsg::MoveTo { x, y, color } => {
	    *line = vec![ (x, y, parse_color(color)?) ];
	},
	JMsg::LineTo { x, y, color } => {
	    line.push( (x, y, parse_color(color)?) );
	},
	JMsg::Stroke => {
	    if line.len() > 1 {
		let line2 = simplify_line(line);
		*line = vec![];		
		return Ok(Some(GSMsg::NewLine(line2)));
	    }
	},
	JMsg::Line{..} => {
	    tracing::warn!("recieved a line message O_o");
	}
    };
    Ok(None)
}

fn line_to_json(line: &Line) -> String {
    let line = line.iter()
	.map(| (x, y, c) | {
	    (*x, *y, format!("#{:06x}", c))
	})
	.collect();    
    serde_json::to_string(&JMsg::Line{ line }).unwrap()
}

fn parse_color(s: String) -> Result<u32, &'static str> {
    if s.len() != 7 || &s[0..1] != "#" {
	Err("badly formated color.")
    } else {
	u32::from_str_radix(&s[1..], 16).map_err(|_| "unable to parse color")
    }
}
