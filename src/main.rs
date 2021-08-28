use futures::{SinkExt, StreamExt, TryFutureExt};
use std::collections::HashMap;
use tokio::sync::mpsc::{self, UnboundedSender};
use tokio_stream::wrappers::UnboundedReceiverStream;
use uuid::Uuid;
use warp::ws::WebSocket;
use warp::Filter;
use xtra::prelude::*;
use xtra::spawn::Tokio;

// User
struct User {
    id: Uuid,
    tx: UnboundedSender<String>,
}
impl Actor for User {}
impl User {
    fn new(id: Uuid, tx: UnboundedSender<String>) -> Self {
        Self { id, tx }
    }
}

// ToUser - sends message back up to user
struct ToUser(String);
impl Message for ToUser {
    type Result = ();
}
#[async_trait::async_trait]
impl Handler<ToUser> for User {
    async fn handle(&mut self, msg: ToUser, _ctx: &mut Context<Self>) {
        self.tx.send(msg.0).expect("Could not pipe message back");
    }
}

// Room
struct Room {
    users: HashMap<Uuid, Address<User>>,
}
impl Actor for Room {}
impl Room {
    fn new() -> Self {
        Self {
            users: HashMap::new(),
        }
    }
}

// GotUserMessage
struct GotUserMessage(Uuid, String);
impl Message for GotUserMessage {
    type Result = ();
}
#[async_trait::async_trait]
impl Handler<GotUserMessage> for Room {
    async fn handle(&mut self, msg: GotUserMessage, _ctx: &mut Context<Self>) {
        for (id, addr) in self.users.iter() {
            println!("sending!");
            // Send to all but sender
            if id != &msg.0 {
                addr.send(ToUser(msg.1.clone()))
                    .await
                    .expect("Could not send");
            }
        }
    }
}

// Join
struct Join(Uuid, Address<User>);
impl Message for Join {
    type Result = ();
}
#[async_trait::async_trait]
impl Handler<Join> for Room {
    async fn handle(&mut self, msg: Join, _ctx: &mut Context<Self>) {
        self.users.insert(msg.0, msg.1);
        println!("Joined! now there are {}", &self.users.len());
    }
}

// Leave
struct Leave(Uuid);
impl Message for Leave {
    type Result = ();
}
#[async_trait::async_trait]
impl Handler<Leave> for Room {
    async fn handle(&mut self, msg: Leave, _ctx: &mut Context<Self>) {
        println!("left!");
        self.users.remove(&msg.0);
    }
}

// Main
#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    // Keep track of all connected users, key is usize, value
    // is a websocket sender.
    let room = Room::new().create(None).spawn(&mut Tokio::Global);
    let room = warp::any().map(move || room.clone());

    let chat = warp::path("ws")
        .and(warp::ws())
        .and(room)
        .map(|ws: warp::ws::Ws, room| ws.on_upgrade(move |socket| user_connected(socket, room)));

    let index = warp::path::end().map(|| warp::reply::html(INDEX_HTML));

    let routes = index.or(chat);

    warp::serve(routes).run(([127, 0, 0, 1], 3030)).await;
}

async fn user_connected(ws: WebSocket, room: xtra::Address<Room>) {
    let (mut user_ws_tx, mut user_ws_rx) = ws.split();
    let (tx, rx) = mpsc::unbounded_channel();
    let mut rx = UnboundedReceiverStream::new(rx);

    let id = Uuid::new_v4();
    let addr = User::new(id, tx).create(None).spawn(&mut Tokio::Global);
    room.send(Join(id, addr))
        .await
        .expect("Could not join the room");

    // Pipe mesesages back up to the user
    tokio::task::spawn(async move {
        while let Some(value) = rx.next().await {
            let message = warp::ws::Message::text(value);
            user_ws_tx
                .send(message)
                .unwrap_or_else(|e| {
                    eprintln!("websocket send error: {}", e);
                })
                .await;
        }
    });

    // Receive messages
    while let Some(result) = user_ws_rx.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(_) => {
                break;
            }
        };

        // Send in to actor
        if let Ok(s) = msg.to_str() {
            room.send(GotUserMessage(id, s.to_string()))
                .await
                .expect("Could not receive message");
        };
    }

    room.send(Leave(id))
        .await
        .expect("Could not leave the room");
}

static INDEX_HTML: &str = r#"<!DOCTYPE html>
<html lang="en">
    <head>
        <title>Warp Chat</title>
    </head>
    <body>
        <h1>Warp chat</h1>
        <div id="chat">
            <p><em>Connecting...</em></p>
        </div>
        <input type="text" id="text" />
        <button type="button" id="send">Send</button>
        <script type="text/javascript">
        const chat = document.getElementById('chat');
        const text = document.getElementById('text');
        const uri = 'ws://' + location.host + '/ws';
        const ws = new WebSocket(uri);
        function message(data) {
            const line = document.createElement('p');
            line.innerText = data;
            chat.appendChild(line);
        }
        ws.onopen = function() {
            chat.innerHTML = '<p><em>Connected!</em></p>';
        };
        ws.onmessage = function(msg) {
            message(msg.data);
        };
        ws.onclose = function() {
            chat.getElementsByTagName('em')[0].innerText = 'Disconnected!';
        };
        send.onclick = function() {
            const msg = text.value;
            ws.send(msg);
            text.value = '';
            message('<You>: ' + msg);
        };
        </script>
    </body>
</html>
"#;
