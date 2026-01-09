use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    sync::{
        OnceLock,
        atomic::{AtomicU32, AtomicUsize},
    },
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    MoveMode, PlayerAbilities,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Pos},
    net::{NetworkMessageC2S, NetworkMessageS2C},
    registry::{self, BlockData, BlockKey, ItemKey, load_registries},
    ui::UIScreenKey,
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
    inventory::{Inventory, InventoryView, ItemDurability, ItemStack},
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::{
        BlockEvent, BlockMachine, Chunk, ChunkSaveData, Entity, EntityEvent, EntityIndex, air_block,
    },
};

mod inventory;
mod world;

fn main() {
    /*rayon::ThreadPoolBuilder::new()
    .num_threads(1)
    .build_global()
    .unwrap();*/
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
    let mut player_spawns = Vec::new();
    loop {
        {
            let delta_time = Duration::from_millis(1000 / server.tps as u64); //probably make it smarter
            network_server.update(delta_time);
            network_transport
                .update(delta_time, &mut network_server)
                .unwrap();
        }
        for (user, spawn_position) in player_spawns.drain(..) {
            let mut entity = Entity::new(Key::id("player").unwrap(), spawn_position);
            InventoryView::from_range(0..10).add_item(
                &mut *entity.inventory.write(),
                ItemStack::new(ItemKey::id("grass_item").unwrap(), 20),
            );
            InventoryView::from_range(0..10).add_item(
                &mut *entity.inventory.write(),
                ItemStack::new(ItemKey::id("barrel").unwrap(), 20),
            );
            let player_uuid = entity.uuid;
            let add_message = entity.create_add_message();
            let chunk_position = entity.position.to_chunk_pos();
            let entity_index = server.entities.insert(entity);
            let mut chunk = server.chunks.get_mut(&chunk_position).unwrap();
            chunk.entities.push(entity_index);
            server
                .message_queue
                .send_message(chunk.viewers.iter(), add_message);
            server.users.get_mut(user).unwrap().entity = Some(entity_index);
            server.send_message(
                user,
                NetworkMessageS2C::SetPlayerEntity {
                    uuid: Some(player_uuid),
                },
            );
            server.send_message(
                user,
                NetworkMessageS2C::PlayerAbilities {
                    abilities: PlayerAbilities {
                        move_mode: MoveMode::Normal,
                        speed: 1.,
                    },
                },
            );
        }
        let mut chunk_viewing_manager = ChunkViewingManager::new();
        while let Some(event) = network_server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    println!("Client {client_id} connected");
                    let spawn_position = Pos {
                        x: 0.,
                        y: 85.,
                        z: 0.,
                    };
                    let user = server.users.insert(User {
                        client_id,
                        view_position: Mutex::new(spawn_position.to_chunk_pos()),
                        entity: None,
                        teleport_id: AtomicU32::new(1),
                        screen: Mutex::new(None),
                        hotbar_slot: AtomicUsize::new(0),
                    });
                    server.send_message(
                        user,
                        NetworkMessageS2C::TeleportPlayer {
                            position: spawn_position,
                            teleport_id: 1,
                        },
                    );
                    player_spawns.push((user, spawn_position));
                    net_users.insert(client_id, user);
                    for chunk_position in
                        User::loading_area_for_view_position(spawn_position.to_chunk_pos())
                    {
                        chunk_viewing_manager.add_viewer(chunk_position, user, &mut server);
                    }
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    println!("Client {client_id} disconnected: {reason}");
                    let user_index = net_users.remove(&client_id).unwrap();
                    let user = server.users.remove(user_index).unwrap();
                    let view_position = *user.view_position.lock();
                    for chunk_position in User::loading_area_for_view_position(view_position) {
                        chunk_viewing_manager.remove_viewer(
                            chunk_position,
                            user_index,
                            &mut server,
                        );
                    }
                    if let Some(entity) = user.entity {
                        server
                            .entities
                            .get(entity)
                            .unwrap()
                            .removed
                            .store(true, std::sync::atomic::Ordering::Relaxed);
                    }
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
                    NetworkMessageC2S::PlayerPosition {
                        position,
                        teleport_id,
                    } => {
                        if teleport_id
                            != user.teleport_id.load(std::sync::atomic::Ordering::Relaxed)
                        {
                            continue;
                        }
                        if let Some(entity) = user.entity {
                            let mut blocked = false;
                            let entity = server.entities.get(entity).unwrap();
                            for block in entity.key.data().hitbox().offset(position).to_block() {
                                if match server.get_block(block) {
                                    Some(block) => !block.data().selection.is_empty(), //todo: proper collisions
                                    None => true,
                                } {
                                    blocked = true;
                                    break;
                                }
                            }
                            if blocked {
                                let teleport_id = user
                                    .teleport_id
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                    + 1;
                                server.message_queue.send_message(
                                    std::iter::once(user_id),
                                    NetworkMessageS2C::TeleportPlayer {
                                        position: entity.position,
                                        teleport_id,
                                    },
                                );
                                continue;
                            }
                        }
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
                        if let Some(entity) = user.entity {
                            *server.entities.get(entity).unwrap().teleport.lock() = Some(position);
                        }
                    }
                    NetworkMessageC2S::AttackBlock { position } => {
                        server.schedule_block_event(position, BlockEvent::Damage { damage: 1. });
                    }
                    NetworkMessageC2S::PlaceBlock { position, face } => {
                        if let Some(entity) = user.entity {
                            let entity = server.entities.get(entity).unwrap();
                            let hotbar_slot =
                                user.hotbar_slot.load(std::sync::atomic::Ordering::Relaxed);
                            let mut inventory = entity.inventory.write();
                            if let Some(item) = &mut inventory.items[hotbar_slot] {
                                if let Some(place) = item.item.data().place {
                                    if let Ok(_) =
                                        server.place(position + face.get_block_offset(), place)
                                    {
                                        item.count -= 1;
                                        if item.count == 0 {
                                            inventory.items[hotbar_slot] = None;
                                        }
                                    }
                                }
                            }
                        }
                    }
                    NetworkMessageC2S::CloseUI => {
                        if let Some(screen) = &mut *user.screen.lock() {
                            screen.state = UserScreenState::Close;
                        }
                    }
                    NetworkMessageC2S::HotbarSelect { slot, relative } => {
                        let new_slot = if relative {
                            user.hotbar_slot.load(std::sync::atomic::Ordering::Relaxed) as isize
                                + slot
                        } else {
                            slot
                        };
                        let hotbar_size = 10;
                        let new_slot =
                            ((new_slot % hotbar_size + hotbar_size) % hotbar_size) as usize;
                        user.hotbar_slot
                            .store(new_slot, std::sync::atomic::Ordering::Relaxed);
                    }
                    NetworkMessageC2S::InteractBlock { position } => {
                        server.schedule_block_event(
                            position,
                            BlockEvent::PlayerInteract {
                                user: user_id.into(),
                            },
                        );
                    }
                    NetworkMessageC2S::InteractEntity { entity: entity_id } => {
                        for chunk in (AABB::<i16> {
                            min: ChunkPos {
                                x: -1,
                                y: -1,
                                z: -1,
                            },
                            max: ChunkPos { x: 1, y: 1, z: 1 },
                        }
                        .offset(*user.view_position.lock()))
                        {
                            if let Some(chunk) = server.get_chunk(chunk) {
                                for entity in &chunk.entities {
                                    let entity = server.get_entity(*entity).unwrap();
                                    if entity.uuid == entity_id {
                                        entity.events.lock().push(EntityEvent::PlayerInteract {
                                            user: user_id.into(),
                                        });
                                    }
                                }
                            }
                        }
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

        for (user_index, user) in &server.users {
            server.message_queue.send_message(
                std::iter::once(user_index),
                NetworkMessageS2C::HUDUpdate {
                    items: match user
                        .entity
                        .as_ref()
                        .and_then(|entity| server.entities.get(*entity))
                    {
                        Some(entity) => entity
                            .inventory
                            .read()
                            .items
                            .iter()
                            .map(|item| item.as_ref().map(|item| item.client()))
                            .collect(),
                        None => vec![],
                    },
                },
            );
            let mut screen_lock = user.screen.lock();
            if let Some(screen) = &mut *screen_lock {
                match screen.state {
                    UserScreenState::Open => {
                        println!("open inventory");
                        let inventory = match screen.inventory.get_inventory(&server) {
                            Some(inventory) => inventory,
                            None => {
                                *screen_lock = None;
                                continue;
                            }
                        };
                        server.message_queue.send_message(
                            std::iter::once(user_index),
                            NetworkMessageS2C::UIOpen {
                                screen: screen.screen,
                                slots: inventory
                                    .items
                                    .iter()
                                    .map(|item| item.as_ref().map(|item| item.client()))
                                    .collect(),
                            },
                        );
                        screen.previous_inventory = inventory;
                        screen.state = UserScreenState::Normal;
                    }
                    UserScreenState::Normal => {
                        let new_inventory = match screen.inventory.get_inventory(&server) {
                            Some(inventory) => inventory,
                            None => {
                                screen.state = UserScreenState::Close;
                                continue;
                            }
                        };
                        for (slot, (previous, new)) in screen
                            .previous_inventory
                            .items
                            .iter()
                            .zip(new_inventory.items.iter())
                            .enumerate()
                        {
                            if previous != new {
                                server.message_queue.send_message(
                                    std::iter::once(user_index),
                                    NetworkMessageS2C::UISetSlot {
                                        slot,
                                        item: new.as_ref().map(|item| item.client()),
                                    },
                                );
                            }
                        }
                        screen.previous_inventory = new_inventory;
                    }
                    UserScreenState::Close => {
                        server
                            .message_queue
                            .send_message(std::iter::once(user_index), NetworkMessageS2C::UIClose);
                        *screen_lock = None;
                    }
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
                .send_message(chunk.viewers.iter(), add_message);
        }

        server.entities.retain(|index, entity| {
            if let Some(teleport) = entity.teleport.lock().take() {
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
                        );
                        server.message_queue.send_message(
                            new_chunk.viewers.difference(&previous_chunk.viewers),
                            entity.create_add_message(),
                        );
                        server.message_queue.send_message(
                            new_chunk.viewers.intersection(&previous_chunk.viewers),
                            entity.create_move_message(),
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
                        );
                    }
                }
            }
            let removed = entity.removed.load(std::sync::atomic::Ordering::Relaxed);
            if removed {
                server.message_queue.send_message(
                    server
                        .chunks
                        .get(&entity.position.to_chunk_pos())
                        .unwrap()
                        .viewers
                        .iter(),
                    entity.create_remove_message(),
                );
            }
            !removed
        });

        chunk_viewing_manager.manage(&mut server, &database);

        {
            let mut skipping_users = HashSet::new();
            server.message_queue.0.get_mut().retain(|(user, message)| {
                if skipping_users.contains(user) {
                    return true;
                }
                let client_id = match server.users.get(*user) {
                    Some(user) => user.client_id,
                    None => return false,
                };
                //idk why it is broken and needs + 1000
                if network_server
                    .channel_available_memory(client_id, DefaultChannel::ReliableOrdered)
                    < message.len() + 1000
                {
                    skipping_users.insert(*user);
                    return true;
                }
                network_server.send_message(
                    client_id,
                    DefaultChannel::ReliableOrdered,
                    message.clone(),
                );
                false
            });
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
pub struct MessageQueue(Mutex<Vec<(UserIndex, renet::Bytes)>>);
impl MessageQueue {
    pub fn send_message<T: std::borrow::Borrow<UserIndex>>(
        &self,
        users: impl Iterator<Item = T>,
        message: NetworkMessageS2C,
    ) {
        let message = bincode::serde::encode_to_vec(message, bincode::config::standard()).unwrap();
        let message: renet::Bytes = message.into();
        let mut message_queue = self.0.lock();
        for user in users {
            message_queue.push((*user.borrow(), message.clone()));
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
    pub fn place(&self, position: BlockPos, block: BlockKey) -> Result<(), ()> {
        let block_data = block.data();
        let (chunk, offset) = position.to_chunk_pos_offset();
        let chunk = match self.get_chunk(chunk) {
            Some(chunk) => chunk,
            None => {
                return Err(());
            }
        };
        for chunk in (AABB {
            min: ChunkPos {
                x: -1,
                y: -1,
                z: -1,
            },
            max: ChunkPos { x: 1, y: 0, z: 1 },
        }
        .offset(position.to_chunk_pos()))
        {
            if let Some(chunk) = self.get_chunk(chunk) {
                for entity in &chunk.entities {
                    let entity = self.entities.get(*entity).unwrap();
                    if entity.get_hitbox().to_block().contains(position) {
                        return Err(());
                    }
                }
            }
        }
        {
            let mut blocks = chunk.blocks.write();
            if *blocks.get(offset.index()).unwrap() != air_block() {
                return Err(());
            }
            blocks.set(offset.index(), &block);
        }
        self.send_message_multiple(
            chunk.viewers.iter(),
            NetworkMessageS2C::SetBlock { position, block },
        );
        if let Some(machine_data) = &block_data.machine {
            chunk.components.machine.write().set(
                offset,
                BlockMachine {
                    inventory: RwLock::new(Inventory::new(machine_data.inventory_size)),
                },
            );
        }
        Ok(())
    }
    pub fn schedule_block_event(&self, position: BlockPos, event: BlockEvent) {
        let (chunk, offset) = position.to_chunk_pos_offset();
        if let Some(chunk) = self.get_chunk(chunk) {
            chunk.block_events.lock().push((offset, event));
        }
    }
    pub fn get_block(&self, position: BlockPos) -> Option<BlockKey> {
        let (chunk, offset) = position.to_chunk_pos_offset();
        let chunk = match self.get_chunk(chunk) {
            Some(chunk) => chunk,
            None => {
                return None;
            }
        };
        Some(*chunk.blocks.read().get(offset.index()).unwrap())
    }
    pub fn get_chunk(&self, position: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&position)
    }
    pub fn get_user(&self, user: UserIndex) -> Option<&User> {
        self.users.get(user)
    }
    pub fn get_entity(&self, entity: EntityIndex) -> Option<&Entity> {
        self.entities.get(entity)
    }
    pub fn send_message(&self, user: UserIndex, message: NetworkMessageS2C) {
        self.message_queue
            .send_message(std::iter::once(user), message);
    }
    pub fn send_message_multiple<T: std::borrow::Borrow<UserIndex>>(
        &self,
        users: impl Iterator<Item = T>,
        message: NetworkMessageS2C,
    ) {
        self.message_queue.send_message(users, message);
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
            for entity in &chunk.entities {
                let add_message = server.entities.get(*entity).unwrap().create_add_message();
                server
                    .message_queue
                    .send_message(std::iter::once(user), add_message);
            }
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
                                if entity.key == Key::id("player").unwrap() {
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
            .into_iter()
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
                (position, users, data)
            })
            .par_bridge()
            .map(|(position, users, data)| {
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
    teleport_id: AtomicU32,
    screen: Mutex<Option<UserScreen>>,
    hotbar_slot: AtomicUsize,
}
pub struct UserScreen {
    pub screen: UIScreenKey,
    pub inventory: InventoryProvider,
    pub state: UserScreenState,
    pub previous_inventory: Inventory,
}
pub enum UserScreenState {
    Open,
    Normal,
    Close,
}
#[derive(Clone, Copy)]
pub enum InventoryProvider {
    Entity(EntityIndex),
    Block(BlockPos),
}
impl InventoryProvider {
    //todo: prevent copies?
    pub fn get_inventory(self, server: &Server) -> Option<Inventory> {
        match self {
            InventoryProvider::Entity(entity) => {
                Some(server.entities.get(entity)?.inventory.read().clone())
            }
            InventoryProvider::Block(block) => {
                let (chunk, offset) = block.to_chunk_pos_offset();
                Some(
                    server
                        .get_chunk(chunk)?
                        .components
                        .machine
                        .read()
                        .get(offset)?
                        .inventory
                        .read()
                        .clone(),
                )
            }
        }
    }
}
impl User {
    pub fn set_screen(&self, screen: UIScreenKey, inventory: InventoryProvider) {
        *self.screen.lock() = Some(UserScreen {
            inventory,
            screen,
            state: UserScreenState::Open,
            previous_inventory: Inventory::new(0),
        });
    }
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
                y: world_height - 1,
                z: view_position.z + distance,
            },
        }
    }
}
