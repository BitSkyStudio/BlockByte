use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    sync::OnceLock,
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos},
    net::{NetworkMessageC2S, NetworkMessageS2C},
    registry::{self, BlockData, BlockKey, key_of_id, load_registries},
};
use palettevec::PaletteVec;
use parking_lot::Mutex;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use renet::{ChannelConfig, ClientId, ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};

use crate::{
    inventory::{ItemDurability, ItemStack},
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::{BlockEvent, Chunk},
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
        let mut chunk_viewing_manager = ChunkViewingManager::new();
        while let Some(event) = network_server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    println!("Client {client_id} connected");
                    let user = server.users.insert(User {
                        client_id,
                        view_position: Mutex::new(ChunkPos { x: 0, y: 0, z: 0 }),
                    });
                    net_users.insert(client_id, user);
                    for chunk_position in
                        User::loading_area_for_view_position(ChunkPos { x: 0, y: 0, z: 0 })
                    {
                        chunk_viewing_manager.add_viewer(chunk_position, user, &mut server);
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    println!("Client {client_id} disconnected: {reason}");
                    let user = net_users.remove(&client_id).unwrap();
                    let view_position = *server.users.get(user).unwrap().view_position.lock();
                    for chunk_position in User::loading_area_for_view_position(view_position) {
                        chunk_viewing_manager.remove_viewer(chunk_position, user, &mut server);
                    }
                    server.users.remove(user);
                }
            }
        }
        let mut view_chunk_chunk_changed_users = HashMap::new();
        for (user_id, user) in &server.users {
            while let Some(message) =
                network_server.receive_message(user.client_id, DefaultChannel::ReliableOrdered)
            {
                let (message, _): (NetworkMessageC2S, _) =
                    bincode::serde::decode_from_slice(&message, bincode::config::standard())
                        .unwrap();
                match message {
                    NetworkMessageC2S::PlayerPosition { position } => {
                        let chunk_position = position.to_block_pos().to_chunk_pos();
                        let view_position = *user.view_position.lock();
                        if chunk_position != view_position {
                            view_chunk_chunk_changed_users
                                .insert(user_id, (view_position, chunk_position));
                            *user.view_position.lock() = chunk_position;
                        }
                    }
                    NetworkMessageC2S::AttackBlock { position } => {
                        server.schedule_block_event(position, BlockEvent::Damage { damage: 1 });
                    }
                    NetworkMessageC2S::InteractBlock { position, face } => {
                        server.place(
                            position + face.get_block_offset(),
                            key_of_id("nature.grass").unwrap(),
                        );
                    }
                }
            }
        }
        for (user, (previous_position, new_position)) in view_chunk_chunk_changed_users {
            let previous_loading = User::loading_area_for_view_position(previous_position);
            let new_loading = User::loading_area_for_view_position(new_position);
            for chunk_position in new_loading {
                if !previous_loading.contains(chunk_position) {
                    chunk_viewing_manager.add_viewer(chunk_position, user, &mut server);
                }
            }
            for chunk_position in previous_loading {
                if !new_loading.contains(chunk_position) {
                    chunk_viewing_manager.remove_viewer(chunk_position, user, &mut server);
                }
            }
        }
        chunk_viewing_manager.manage(&mut server);

        server.chunks.par_iter().for_each(|(_, chunk)| {
            chunk.tick(&server);
        });

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
    pub fn place(&self, position: BlockPos, block: BlockKey) -> bool {
        let (chunk, offset) = position.to_chunk_pos_offset();
        let chunk = match self.get_chunk(chunk) {
            Some(chunk) => chunk,
            None => {
                return false;
            }
        };
        let mut blocks = chunk.blocks.write();
        if *blocks.get(offset.index()).unwrap() != key_of_id::<BlockData>("air").unwrap() {
            return false;
        }
        blocks.set(offset.index(), &block);
        self.send_message_multiple(
            chunk.viewers.iter().cloned(),
            NetworkMessageS2C::SetBlock { position, block },
        );
        true
    }
    pub fn schedule_block_event(&self, position: BlockPos, event: BlockEvent) {
        let (chunk, offset) = position.to_chunk_pos_offset();
        if let Some(chunk) = self.get_chunk(chunk) {
            chunk.block_events.lock().push((offset, event));
        }
    }
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

pub struct ChunkViewingManager {
    pub load: HashMap<ChunkPos, Vec<UserIndex>>,
    pub unload: HashSet<ChunkPos>,
}
impl ChunkViewingManager {
    pub fn new() -> Self {
        ChunkViewingManager {
            load: HashMap::new(),
            unload: HashSet::new(),
        }
    }
    pub fn add_viewer(&mut self, position: ChunkPos, user: UserIndex, server: &mut Server) {
        if let Some(chunk) = server.chunks.get_mut(&position) {
            if chunk.viewers.is_empty() {
                self.unload.remove(&position);
            }
            chunk.viewers.insert(user);
            let message = NetworkMessageS2C::LoadChunk {
                position,
                blocks: chunk.blocks.read().clone(),
            };
            server.send_message(user, message);
        } else {
            self.load.entry(position).or_default().push(user);
        }
    }
    pub fn remove_viewer(&mut self, position: ChunkPos, user: UserIndex, server: &mut Server) {
        let chunk = server.chunks.get_mut(&position).unwrap();
        chunk.viewers.remove(&user);
        if chunk.viewers.len() == 0 {
            self.unload.insert(position);
        }
        server.send_message(user, NetworkMessageS2C::UnloadChunk { position });
    }
    pub fn manage(self, server: &mut Server) {
        for position in self.unload {
            server.chunks.remove(&position);
        }
        use rayon::iter::ParallelIterator;
        for (position, users, mut chunk) in self
            .load
            .into_par_iter()
            .map(|(position, users)| (position, users, Chunk::generate(position)))
            .collect::<Vec<_>>()
        {
            chunk.viewers = users.iter().cloned().collect();
            server.send_message_multiple(
                users.iter().cloned(),
                NetworkMessageS2C::LoadChunk {
                    position,
                    blocks: chunk.blocks.read().clone(),
                },
            );
            server.chunks.insert(position, chunk);
        }
    }
}

new_key_type! {pub struct UserIndex;}
pub struct User {
    client_id: ClientId,
    view_position: Mutex<ChunkPos>,
}
impl User {
    pub fn loading_area_for_view_position(view_position: ChunkPos) -> AABB<i16> {
        let distance = 8;
        let world_height = 8;
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
