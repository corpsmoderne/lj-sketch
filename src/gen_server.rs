use std::collections::HashMap;
use std::net::SocketAddr;
use tokio::sync::mpsc:: {Sender, Receiver};
use crate::ws_client::Line;

#[derive(Debug, Clone)]
pub enum GSMsg {
    NewClient((SocketAddr, Sender<GSMsg>)),
    NewLine(Line),
    DeleteClient(SocketAddr),
    Clear
}

pub struct State {
    pub gs_tx: Sender<GSMsg>
}

pub async fn gen_server(mut rx: Receiver<GSMsg>) {
    let mut clients : HashMap<SocketAddr, Sender<GSMsg>> =
	HashMap::new();
    
    let mut lines : Vec<Line> = vec![];
    
    while let Some(msg) = rx.recv().await {
	match msg {
	    GSMsg::NewClient((addr, c_tx)) => {
		for line in &lines {
		    c_tx.send(GSMsg::NewLine(line.clone()))
			.await.unwrap();
		}
		clients.insert(addr, c_tx);		
		tracing::info!("NewClient {addr}");
	    },
	    GSMsg::NewLine(line) => {
		send_all(&mut clients, &GSMsg::NewLine(line.clone())).await;
		lines.push(line);
	    },
	    GSMsg::DeleteClient(addr) => {
		tracing::info!("Client {addr} removed");		
		clients.remove(&addr);		
	    },
	    GSMsg::Clear => {
		send_all(&mut clients, &GSMsg::Clear).await;
		lines.clear();
	    }
	}
    }
}

async fn send_all(
    clients: &mut HashMap<SocketAddr, Sender<GSMsg>>,
    msg: &GSMsg
) {
    let mut to_remove : Vec<SocketAddr> = vec![];
		
    for (addr, ref mut tx) in &mut *clients {
	let ret = tx
	    .send(msg.clone())
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
