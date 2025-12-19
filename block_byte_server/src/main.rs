use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos},
    net::{NetworkMessageC2S, NetworkMessageS2C},
    registry::{self, load_registries},
};
use palettevec::PaletteVec;
use parking_lot::Mutex;
use renet::{ChannelConfig, ClientId, ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};

use crate::{
    inventory::{ItemDurability, ItemStack},
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::Chunk,
};

mod inventory;
mod world;

fn main() {
    load_registries(&Path::new("assets"));
    let mut network_server = RenetServer::new(ConnectionConfig::default());
    const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 5000);
    let network_socket: UdpSocket = UdpSocket::bind(SERVER_ADDR).unwrap();
    let network_server_config = ServerConfig {
        current_time: SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap(),
        max_clients: 64,
        protocol_id: 0,
        public_addresses: vec![SERVER_ADDR],
        authentication: ServerAuthentication::Unsecure,
    };
    let mut network_transport =
        NetcodeServerTransport::new(network_server_config, network_socket).unwrap();

    let mut server = Server {
        chunks: HashMap::new(),
        users: SlotMap::with_key(),
        message_queue: Mutex::new(Vec::new()),
    };
    let start_time = Instant::now();
    let mut tick_count: u32 = 0;
    let tps = 40;
    let mut net_users = HashMap::new();
    loop {
        {
            let delta_time = Duration::from_millis(1000 / tps as u64); //probably make it smarter
            network_server.update(delta_time);
            network_transport
                .update(delta_time, &mut network_server)
                .unwrap();
        }
        while let Some(event) = network_server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    println!("Client {client_id} connected");
                    let user = server.users.insert(User {
                        client_id,
                        view_position: ChunkPos { x: 0, y: 0, z: 0 },
                    });
                    net_users.insert(client_id, user);
                    for chunk_position in
                        User::loading_area_for_view_position(ChunkPos { x: 0, y: 0, z: 0 })
                    {
                        let message = {
                            let chunk = server
                                .chunks
                                .entry(chunk_position)
                                .or_insert_with(|| Chunk::generate(chunk_position));
                            chunk.viewers.insert(user);
                            NetworkMessageS2C::LoadChunk {
                                position: chunk_position,
                                blocks: chunk.blocks.read().clone(),
                            }
                        };
                        server.send_message(user, message);
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    println!("Client {client_id} disconnected: {reason}");
                    let user = net_users.remove(&client_id).unwrap();
                    let view_position = server.users.get(user).unwrap().view_position;
                    for chunk_position in User::loading_area_for_view_position(view_position) {
                        let chunk = server.chunks.get_mut(&chunk_position).unwrap();
                        chunk.viewers.remove(&user);
                    }
                    server.users.remove(user);
                }
            }
        }
        for user in server.users.values() {
            while let Some(message) =
                network_server.receive_message(user.client_id, DefaultChannel::ReliableOrdered)
            {
                let (message, _): (NetworkMessageC2S, _) =
                    bincode::serde::decode_from_slice(&message, bincode::config::standard())
                        .unwrap();
            }
        }

        for (user, message) in server.message_queue.lock().drain(..) {
            network_server.send_message(user, DefaultChannel::ReliableOrdered, message);
        }

        network_transport.send_packets(&mut network_server);

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
    chunks: HashMap<ChunkPos, Chunk>,
    users: SlotMap<UserIndex, User>,
    message_queue: Mutex<Vec<(ClientId, renet::Bytes)>>,
}
impl Server {
    pub fn get_chunk(&self, position: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&position)
    }
    pub fn get_user(&self, user: UserIndex) -> Option<&User> {
        self.users.get(user)
    }
    pub fn send_message(&self, user: UserIndex, message: NetworkMessageS2C) {
        self.send_message_multiple(std::iter::once(user), message);
    }
    pub fn send_message_multiple(
        &self,
        users: impl Iterator<Item = UserIndex>,
        message: NetworkMessageS2C,
    ) {
        let message = bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap();
        let message: renet::Bytes = message.into();
        let mut message_queue = self.message_queue.lock();
        for user in users {
            if let Some(user) = self.users.get(user) {
                message_queue.push((user.client_id, message.clone()));
            }
        }
    }
}

new_key_type! {pub struct UserIndex;}
pub struct User {
    client_id: ClientId,
    view_position: ChunkPos,
}
impl User {
    pub fn loading_area_for_view_position(view_position: ChunkPos) -> AABB<i16> {
        let distance = 2;
        let world_height = 3;
        AABB {
            min: ChunkPos {
                x: view_position.x - distance,
                y: 0,
                z: view_position.z - distance,
            },
            max: ChunkPos {
                x: view_position.x + distance,
                y: world_height,
                z: view_position.z + distance,
            },
        }
    }
}
