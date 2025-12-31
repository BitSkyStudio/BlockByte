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
    registry::{self, BlockData, BlockKey, load_registries},
};
use palettevec::PaletteVec;
use parking_lot::{Mutex, RwLock};
use rayon::iter::{
    IntoParallelIterator, IntoParallelRefIterator, ParallelBridge, ParallelIterator,
};
use renet::{ChannelConfig, ClientId, ConnectionConfig, DefaultChannel, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};

use crate::{
    inventory::{ItemDurability, ItemStack},
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::{BlockEvent, Chunk, ChunkSaveData, Entity, EntityIndex, air_block},
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

    let database_path = Path::new("save.db3");
    let database = rusqlite::Connection::open(database_path).unwrap();
    {
        database.execute(
            "CREATE TABLE `chunks` (
            `x` INTEGER, `y` INTEGER, `z` INTEGER,
            `data` BLOB NOT NULL,
            PRIMARY KEY (`x`, `z`, `y`)
        )",
            (),
        );
        /*database
        .execute(
            "CREATE TABLE `structure_placement` (
            `chunk_x` INTEGER, `chunk_y` INTEGER, `chunk_z` INTEGER,
            `x` INTEGER, `y` INTEGER, `z` INTEGER,
            `structure_id` STRING, 'seed' INTEGER
        )",
            (),
        )
        .unwrap();*/
    }
    let database = Mutex::new(database);

    let mut server = Server {
        ticks_passed: 0,
        tps: 40,
        chunks: HashMap::new(),
        users: SlotMap::with_key(),
        message_queue: Default::default(),
        entities: SlotMap::with_key(),
        entity_add_queue: Mutex::new(Vec::new()),
    };
    let start_time = Instant::now();
    let mut net_users = HashMap::new();
    loop {
        {
            let delta_time = Duration::from_millis(1000 / server.tps as u64); //probably make it smarter
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
                        entity: None,
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
        let mut view_chunk_chunk_changed_users: HashMap<UserIndex, (ChunkPos, ChunkPos)> =
            HashMap::new();
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
                            if let Some(entry) = view_chunk_chunk_changed_users.get_mut(&user_id) {
                                entry.1 = chunk_position;
                            } else {
                                view_chunk_chunk_changed_users
                                    .insert(user_id, (view_position, chunk_position));
                            }
                            *user.view_position.lock() = chunk_position;
                        }
                    }
                    NetworkMessageC2S::AttackBlock { position } => {
                        server.schedule_block_event(position, BlockEvent::Damage { damage: 1. });
                    }
                    NetworkMessageC2S::InteractBlock { position, face } => {
                        server.place(
                            position + face.get_block_offset(),
                            Key::id("nature.grass").unwrap(),
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

        server.chunks.par_iter().for_each(|(_, chunk)| {
            chunk.tick(&server);
        });

        for entity in server.entity_add_queue.get_mut().drain(..) {
            let add_message = entity.create_add_message();
            let chunk_position = entity.position.to_chunk_pos();
            let entity_index = server.entities.insert(entity);
            let mut chunk = server.chunks.get_mut(&chunk_position).unwrap();
            chunk.entities.push(entity_index);
            server
                .message_queue
                .send_message(chunk.viewers.iter(), add_message, &server.users);
        }

        server.entities.retain(|index, entity| {
            if let Some(teleport) = entity.teleport.lock().take() {
                //todo: check if chunk loaded, maybe should be done on calling site?
                if server.chunks.contains_key(&teleport.to_chunk_pos()) {
                    if entity.position.to_chunk_pos() != teleport.to_chunk_pos() {
                        let [previous_chunk, new_chunk] = server
                            .chunks
                            .get_disjoint_mut([
                                &entity.position.to_chunk_pos(),
                                &teleport.to_chunk_pos(),
                            ])
                            .map(|v| v.unwrap());
                        previous_chunk.entities.retain(|e| *e != index);
                        new_chunk.entities.push(index);
                        entity.position = teleport;
                        server.message_queue.send_message(
                            previous_chunk.viewers.difference(&new_chunk.viewers),
                            entity.create_remove_message(),
                            &server.users,
                        );
                        server.message_queue.send_message(
                            new_chunk.viewers.difference(&previous_chunk.viewers),
                            entity.create_add_message(),
                            &server.users,
                        );
                        server.message_queue.send_message(
                            new_chunk.viewers.intersection(&previous_chunk.viewers),
                            entity.create_move_message(),
                            &server.users,
                        );
                    } else {
                        entity.position = teleport;
                        server.message_queue.send_message(
                            server
                                .chunks
                                .get(&teleport.to_chunk_pos())
                                .unwrap()
                                .viewers
                                .iter(),
                            entity.create_move_message(),
                            &server.users,
                        );
                    }
                } else {
                    println!("invalid teleport");
                }
            }
            if !server.chunks.contains_key(&entity.position.to_chunk_pos()) {
                return true;
            }
            if !entity.removed.load(std::sync::atomic::Ordering::Relaxed) {
                server.message_queue.send_message(
                    server
                        .chunks
                        .get(&entity.position.to_chunk_pos())
                        .unwrap()
                        .viewers
                        .iter(),
                    entity.create_remove_message(),
                    &server.users,
                );
                true
            } else {
                false
            }
            //todo: possible duplication(teleport to another chunk + unload at the same tick)
        });

        chunk_viewing_manager.manage(&mut server, &database);

        for (user, message) in server.message_queue.0.get_mut().drain(..) {
            network_server.send_message(user, DefaultChannel::ReliableOrdered, message);
        }

        network_server.broadcast_message(
            DefaultChannel::ReliableOrdered,
            bincode::serde::encode_to_vec(
                NetworkMessageS2C::GameTick {
                    ticks_passed: server.ticks_passed,
                    dt: 1. / server.tps as f32,
                },
                bincode::config::standard(),
            )
            .unwrap(),
        );

        network_transport.send_packets(&mut network_server);

        let sleep_time = (server.ticks_passed as i64 * (1000 / server.tps as i64))
            - Instant::now().duration_since(start_time).as_millis() as i64;
        if sleep_time > 0 {
            std::thread::sleep(Duration::from_millis(sleep_time as u64));
        } else if sleep_time < 0 {
            println!("server is running {}ms behind", -sleep_time);
        }
        server.ticks_passed += 1;
    }
}
#[derive(Default)]
pub struct MessageQueue(Mutex<Vec<(ClientId, renet::Bytes)>>);
impl MessageQueue {
    pub fn send_message<T: std::borrow::Borrow<UserIndex>>(
        &self,
        users: impl Iterator<Item = T>,
        message: NetworkMessageS2C,
        user_map: &SlotMap<UserIndex, User>,
    ) {
        let message = bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap();
        let message: renet::Bytes = message.into();
        let mut message_queue = self.0.lock();
        for user in users {
            if let Some(user) = user_map.get(*user.borrow()) {
                message_queue.push((user.client_id, message.clone()));
            }
        }
    }
}
pub struct Server {
    pub ticks_passed: u64,
    pub tps: u64,
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub users: SlotMap<UserIndex, User>,
    pub message_queue: MessageQueue,
    pub entities: SlotMap<EntityIndex, Entity>,
    entity_add_queue: Mutex<Vec<Entity>>,
}
impl Server {
    pub fn spawn_entity(&self, entity: Entity) -> Result<(), ()> {
        if !self
            .chunks
            .contains_key(&entity.position.to_block_pos().to_chunk_pos())
        {
            return Err(());
        }
        self.entity_add_queue.lock().push(entity);
        Ok(())
    }
    pub fn place(&self, position: BlockPos, block: BlockKey) -> bool {
        let (chunk, offset) = position.to_chunk_pos_offset();
        let chunk = match self.get_chunk(chunk) {
            Some(chunk) => chunk,
            None => {
                return false;
            }
        };
        let mut blocks = chunk.blocks.write();
        if *blocks.get(offset.index()).unwrap() != air_block() {
            return false;
        }
        blocks.set(offset.index(), &block);
        self.send_message_multiple(
            chunk.viewers.iter(),
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
        self.message_queue
            .send_message(std::iter::once(user), message, &self.users);
    }
    pub fn send_message_multiple<T: std::borrow::Borrow<UserIndex>>(
        &self,
        users: impl Iterator<Item = T>,
        message: NetworkMessageS2C,
    ) {
        self.message_queue.send_message(users, message, &self.users);
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
                components: chunk.components.client(),
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
    pub fn manage(self, server: &mut Server, db: &Mutex<rusqlite::Connection>) {
        use rayon::iter::ParallelIterator;

        let unloads = self
            .unload
            .iter()
            .map(|position| {
                let chunk = server.chunks.remove(&position).unwrap();
                (
                    position,
                    ChunkSaveData {
                        blocks: chunk.blocks.into_inner(),
                        block_events: chunk.block_events.into_inner(),
                        components: chunk.components,
                        entities: chunk
                            .entities
                            .into_iter()
                            .filter_map(|entity| {
                                let entity = server.entities.remove(entity)?;
                                if entity.removed.load(std::sync::atomic::Ordering::Relaxed) {
                                    return None;
                                }
                                Some(entity)
                            })
                            .collect(),
                    },
                )
            })
            .par_bridge()
            .map(|(position, data)| {
                let chunk_data = serde_cbor::to_vec(&data).unwrap();
                (position, chunk_data)
            })
            .collect::<Vec<_>>();
        {
            let mut db = db.lock();
            let transaction = db.transaction().unwrap();
            let mut statement = transaction
                .prepare_cached("REPLACE INTO chunks (x,y,z,data) VALUES (?1,?2,?3,?4)")
                .unwrap();
            for (position, chunk_data) in unloads {
                statement
                    .execute((position.x, position.y, position.z, chunk_data))
                    .unwrap();
            }
            drop(statement);
            transaction.commit().unwrap();
        }
        for (position, users, (mut chunk, entities)) in self
            .load
            .into_par_iter()
            .map(|(position, users)| {
                let db = db.lock();
                let mut stmt = db
                    .prepare_cached("SELECT data FROM chunks WHERE x=?1 and y=?2 and z=?3")
                    .unwrap();
                let data = stmt
                    .query_row((position.x, position.y, position.z), |row| {
                        row.get::<_, Vec<u8>>(0)
                    })
                    .ok();
                let chunk = match data {
                    Some(data) => {
                        let data: ChunkSaveData = serde_cbor::from_slice(data.as_slice()).unwrap();
                        (
                            Chunk {
                                blocks: RwLock::new(data.blocks),
                                block_events: Mutex::new(data.block_events),
                                components: data.components,
                                position,
                                viewers: HashSet::new(),
                                entities: Vec::new(),
                            },
                            data.entities,
                        )
                    }
                    None => (Chunk::generate(position), Vec::new()),
                };
                (position, users, chunk)
            })
            .collect::<Vec<_>>()
        {
            chunk.viewers = users.iter().cloned().collect();
            for entity in entities {
                server.send_message_multiple(users.iter(), entity.create_add_message());
                chunk.entities.push(server.entities.insert(entity));
            }
            server.send_message_multiple(
                users.iter(),
                NetworkMessageS2C::LoadChunk {
                    position,
                    blocks: chunk.blocks.read().clone(),
                    components: chunk.components.client(),
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
    entity: Option<EntityIndex>,
}
impl User {
    pub fn loading_area_for_view_position(view_position: ChunkPos) -> AABB<i16> {
        let distance = 8;
        let world_height = 4;
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
