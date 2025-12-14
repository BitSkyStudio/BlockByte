use std::{
    collections::HashMap,
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant},
};

use block_byte_common::{
    coord::ChunkPos,
    net::{NetworkMessageC2S, NetworkMessageS2C},
    registry::{self, load_registries},
};
use palettevec::PaletteVec;
use parking_lot::Mutex;
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};
use websocket::OwnedMessage;

use crate::{
    inventory::{ItemDurability, ItemStack},
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::Chunk,
};

mod inventory;
mod world;

fn main() {
    load_registries(&Path::new("assets"));
    let mut server = Server {
        chunks: HashMap::new(),
        users: SlotMap::with_key(),
    };
    let incoming_users = network_server();
    let start_time = Instant::now();
    let mut tick_count: u32 = 0;
    let tps = 40;
    loop {
        while let Ok(user) = incoming_users.try_recv() {
            server.users.insert(User {
                message_sender: Mutex::new(user.message_sender),
                message_receiver: user.message_receiver,
                view_position: ChunkPos { x: 0, y: 0, z: 0 },
            });
        }

        let sleep_time = (tick_count as i64 * (1000 / tps))
            - Instant::now().duration_since(start_time).as_millis() as i64;
        if sleep_time > 0 {
            std::thread::sleep(Duration::from_millis(sleep_time as u64));
        } else if sleep_time < 0 {
            println!("server is running {}ms behind", -sleep_time);
        }
        tick_count += 1;
    }
}

pub struct Server {
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub users: SlotMap<UserIndex, User>,
}

fn data<T>(key: Key<T>) -> &'static T
where
    RegistryStorage: RegistryProvider<T>,
{
    REGISTRIES.get().unwrap().get_registry().by_key(key)
}

pub fn network_server() -> std::sync::mpsc::Receiver<UserLoginInfo> {
    let server = websocket::server::sync::Server::bind("127.0.0.1:2794").unwrap();

    let (user_tx, user_rx) = std::sync::mpsc::channel();

    std::thread::spawn(move || {
        for request in server.filter_map(Result::ok) {
            // Spawn a new thread for each connection.
            let user_tx = user_tx.clone();
            std::thread::spawn(move || {
                let mut client = request.use_protocol("rust-websocket").accept().unwrap();

                let ip = client.peer_addr().unwrap();

                println!("Connection from {}", ip);

                let message = OwnedMessage::Text("Hello".to_string());
                client.send_message(&message).unwrap();

                let (mut receiver, mut sender) = client.split().unwrap();

                let (message_tx, message_rx) = std::sync::mpsc::channel();
                user_tx
                    .send(UserLoginInfo {
                        message_sender: sender,
                        message_receiver: message_rx,
                    })
                    .unwrap();

                for message in receiver.incoming_messages() {
                    let message = message.unwrap();

                    match message {
                        OwnedMessage::Close(_) => {
                            /*
                            let message = OwnedMessage::Close(None);
                            sender.send_message(&message).unwrap();
                            */
                            println!("Client {} disconnected", ip);
                            return;
                        }
                        OwnedMessage::Binary(data) => {
                            let (message, _) = bincode::serde::decode_from_slice(
                                &data,
                                bincode::config::standard(),
                            )
                            .unwrap();
                            message_tx.send(message).unwrap();
                        }
                        _ => {}
                    }
                }
            });
        }
    });
    user_rx
}

new_key_type! {pub struct UserIndex;}
pub struct User {
    message_sender: Mutex<websocket::sender::Writer<std::net::TcpStream>>,
    message_receiver: std::sync::mpsc::Receiver<NetworkMessageC2S>,
    view_position: ChunkPos,
}
impl User {
    pub fn send_message(&self, message: &NetworkMessageS2C) {
        let message = bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap();
        self.message_sender
            .lock()
            .send_message(&OwnedMessage::Binary(message))
            .unwrap();
    }
}

pub struct UserLoginInfo {
    message_sender: websocket::sender::Writer<std::net::TcpStream>,
    message_receiver: std::sync::mpsc::Receiver<NetworkMessageC2S>,
}
