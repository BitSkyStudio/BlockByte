use std::{
    cell::RefCell,
    collections::{HashMap, HashSet, VecDeque},
    env::args,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    ClientItem, EntityAction, InventoryView, LookDirection, SERVER_DT, SERVER_TPS, ViewSlot,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, HorizontalFace, Pos},
    net::{ItemInteractTarget, NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    registry::{
        self, BlockColor, BlockEntry, BlockInteractAction, BlockKey, EntityInteractAction,
        EntityKey, ItemAction, ItemKey, KeyGroup, PrefabBlockEntry, PrefabData, PrefabKey,
        air_block, load_registries,
    },
    rotation::BlockRotation,
    time_to_ticks,
    ui::{PropertyMap, UIScreenKey},
    world::BlockTickList,
};
use parking_lot::Mutex;
use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};
use renet::{Bytes, ClientId, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication};
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};
use uuid::Uuid;

use crate::{
    inventory::{Inventory, ItemCount, ItemStack, LootGenerationContext, generate_loot_table},
    registry::Key,
    world::{
        ActiveEffect, BlockMachine, Chunk, ChunkBlocks, ChunkSaveData, Entity, WorldAccess,
        WorldAccessCell, WorldAccessRef, WorldEvent, compute_tool_damage_and_knockback, tick_chunk,
    },
    worldgen::{WorldGenerator, generate_chunk},
};

mod debug;
mod inventory;
mod world;
mod worldgen;

fn main() {
    load_registries(&[&Path::new("assets"), &Path::new("assets_generated")]);
    /*rayon::ThreadPoolBuilder::new()
    .num_threads(8)
    .build_global()
    .unwrap();*/
    rayon::ThreadPoolBuilder::new()
        .num_threads(4)
        .build_global();
    let mut memorydb = false;
    let mut is_design_server = false;
    if let Some(arg) = args().nth(1) {
        let mut start = false;
        match arg.as_str() {
            "worldgen_vis" => {
                debug::visualise();
            }
            "test_ticklist" => {
                let mut tl = BlockTickList::default();
                tl.set_ticking(3, true);
                tl.set_ticking(5, true);
                tl.set_ticking(34, true);
                tl.set_ticking(64, true);
                tl.set_ticking(65, true);
                tl.set_ticking(101, true);
                tl.set_ticking(128, true);
                tl.set_ticking(537, true);
                let mut index = tl.start_index();
                while let Some(i) = tl.next_index(&mut index) {
                    println!("{}", i);
                }
                return;
            }
            "tree" => {
                let base = "wood.oak";
                let load_block =
                    |btype: &str| BlockKey::id(format!("{}.{}", base, btype).as_str()).unwrap();
                let structure = debug::generate_tree(
                    load_block("log"),
                    load_block("slab"),
                    load_block("branch"),
                    load_block("leaves"),
                );
                println!("{}", ron::to_string(&structure).unwrap());
            }
            "memorydb" => {
                start = true;
                memorydb = true;
            }
            "design" => {
                start = true;
                memorydb = true;
                is_design_server = true;
            }
            _ => {
                start = true;
            }
        }
        if !start {
            return;
        }
    }
    let mut network_server = RenetServer::new(make_connection_config());
    const SERVER_ADDR: SocketAddr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), 5000);
    let network_socket: UdpSocket = UdpSocket::bind(SERVER_ADDR).unwrap();
    let network_server_config = renet_netcode::ServerConfig {
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
    let database = if memorydb {
        rusqlite::Connection::open_in_memory().unwrap()
    } else {
        rusqlite::Connection::open(database_path).unwrap()
    };
    {
        database.execute(
            "CREATE TABLE `chunks` (
            `x` INTEGER, `y` INTEGER, `z` INTEGER,
            `data` BLOB NOT NULL,
            PRIMARY KEY (`x`, `z`, `y`)
        )",
            (),
        );
    }
    let database = Mutex::new(database);

    let console = {
        let (tx, rx) = std::sync::mpsc::channel::<String>();
        std::thread::spawn(move || {
            loop {
                let mut buffer = String::new();
                std::io::stdin().read_line(&mut buffer).unwrap();
                tx.send(buffer).unwrap();
            }
        });
        rx
    };
    {
        let config: ServerConfig = ron::from_str(
            std::fs::read_to_string("server_config.ron")
                .unwrap_or("()".to_string())
                .as_str(),
        )
        .unwrap();
        assert!(SERVER_CONFIG_INSTANCE.set(config).is_ok());
    }

    let world_generator_config = ron::from_str(
        std::fs::read_to_string("assets/world_generator.ron")
            .unwrap()
            .as_str(),
    )
    .unwrap();
    let mut world_generator = WorldGenerator::new(world_generator_config, 1);
    world_generator.design_world = is_design_server;
    let mut server = Server {
        ticks_passed: 0,
        chunks: ahash::HashMap::default(),
        users: SlotMap::with_key(),
        message_queue: Default::default(),
    };
    let start_time = Instant::now();
    let mut net_users = HashMap::new();
    let mut player_spawns = Vec::new();
    let stopped = Arc::new(AtomicBool::new(false));
    {
        let stopped = stopped.clone();
        ctrlc::set_handler(move || {
            match stopped.compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            ) {
                Ok(_) => {}
                Err(_) => {
                    println!("server killed");
                    std::process::exit(0);
                }
            }
        })
        .unwrap();
    }
    while !stopped.load(std::sync::atomic::Ordering::SeqCst) || !server.chunks.is_empty() {
        let tick_start_time = Instant::now();
        if stopped.load(std::sync::atomic::Ordering::SeqCst) {
            network_server.disconnect_all();
        }
        {
            let delta_time = Duration::from_secs_f32(SERVER_DT); //probably make it smarter
            network_server.update(delta_time);
            network_transport
                .update(delta_time, &mut network_server)
                .unwrap();
        }
        for (user, spawn_position) in player_spawns.drain(..) {
            let mut entity = Entity::new(
                Key::id(if is_design_server {
                    "designer"
                } else {
                    "player"
                })
                .unwrap(),
                spawn_position,
            );
            let _full_view = entity.inventory.full_view();
            entity.controlling_user = Some(user);
            let player_uuid = entity.uuid;
            let add_message = entity.create_add_message();
            let chunk_position = entity.position.to_chunk_pos();
            let chunk = server.chunks.get_mut(&chunk_position).unwrap();
            chunk
                .entities
                .insert(entity.uuid, WorldAccessCell::new(entity));
            server
                .message_queue
                .send_message(chunk.viewers.iter(), add_message);
            server.users.get_mut(user).unwrap().entity = Some(player_uuid);
            server.message_queue.send_message(
                std::iter::once(user),
                NetworkMessageS2C::SetPlayerEntity {
                    uuid: Some(player_uuid),
                },
            );
            server.message_queue.send_message(
                std::iter::once(user),
                NetworkMessageS2C::UpdateResearch {
                    research: HashSet::new(), //todo: load
                },
            );
        }
        let mut chunk_viewing_manager = ChunkViewingManager::new();
        while let Some(event) = network_server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    println!("Client {client_id} connected");
                    let spawn_position = world_generator.find_valid_spawn();
                    let user = server.users.insert(User {
                        client_id,
                        view_position: Mutex::new(spawn_position.to_chunk_pos()),
                        last_view_position: spawn_position.to_chunk_pos(),
                        entity: None,
                        teleport_id: AtomicU32::new(1),
                        screen: Mutex::new(None),
                        message_queue: Mutex::new(VecDeque::new()),
                        hud_sync_items: Mutex::new(Vec::new()),
                    });
                    server.message_queue.send_message(
                        std::iter::once(user),
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
                    for chunk_position in
                        User::loading_area_for_view_position(user.last_view_position)
                    {
                        chunk_viewing_manager.remove_viewer(
                            chunk_position,
                            user_index,
                            &mut server,
                        );
                    }
                }
            }
        }
        for (_user_id, user) in &server.users {
            while let Some(message) = network_server.receive_message(user.client_id, 0) {
                let message: NetworkMessageC2S = serde_cbor::from_slice(&message).unwrap();
                user.message_queue.lock().push_front(message);
            }
        }
        let mut view_chunk_chunk_changed_users: HashMap<UserIndex, (ChunkPos, ChunkPos)> =
            HashMap::new();
        for (id, user) in &mut server.users {
            let view_position = *user.view_position.lock();
            if view_position != user.last_view_position {
                view_chunk_chunk_changed_users.insert(id, (user.last_view_position, view_position));
                user.last_view_position = view_position;
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

        while let Ok(command) = console.try_recv() {
            if command.is_empty() {
                continue;
            }
            let command = command.trim().split(" ").collect::<Vec<_>>();
            match command[0] {
                "design_export" => {
                    let origin = BlockPos { x: 0, y: 80, z: 0 };
                    let origin_chunk = origin.to_chunk_pos();
                    let mut prefab = PrefabData::default();
                    for x in -1..=1 {
                        for y in -1..=1 {
                            for z in -1..=1 {
                                let chunk_position = origin_chunk + ChunkPos { x, y, z };
                                let Some(chunk) = server.chunks.get_mut(&chunk_position) else {
                                    continue;
                                };
                                for bx in 0..CHUNK_SIZE {
                                    for by in 0..CHUNK_SIZE {
                                        for bz in 0..CHUNK_SIZE {
                                            let offset =
                                                ChunkOffset::new(bx as u8, by as u8, bz as u8);
                                            let Some(block) =
                                                chunk.blocks.get_mut().get(offset.index())
                                            else {
                                                continue;
                                            };
                                            if block.block != air_block() {
                                                let block_position =
                                                    chunk_position.to_block_pos() + offset.xyz();
                                                let local_position = block_position - origin;
                                                prefab.blocks.push(PrefabBlockEntry {
                                                    x: local_position.x,
                                                    y: local_position.y,
                                                    z: local_position.z,
                                                    chance: 1.,
                                                    replace: KeyGroup::Empty,
                                                    replace_inverted: false,
                                                    block: block.block,
                                                    rotation: block.rotation,
                                                    color: block.color,
                                                    loot_table: None,
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    println!("{}", ron::to_string(&prefab).unwrap());
                }
                "design_load" => {
                    let Some(prefab) = command.get(1).and_then(|c| PrefabKey::id(c)) else {
                        println!("unknown prefab");
                        continue;
                    };
                    prefab.data().build(
                        BlockPos { x: 0, y: 80, z: 0 },
                        HorizontalFace::Front,
                        0,
                        |place_position, block, _entry, _| {
                            let (place_chunk, place_chunk_offset) =
                                place_position.to_chunk_pos_offset();
                            let Some(chunk) = server.chunks.get_mut(&place_chunk) else {
                                return;
                            };
                            chunk
                                .blocks
                                .get_mut()
                                .set(place_chunk_offset.index(), &block);
                            for viewer in &chunk.viewers {
                                server.message_queue.send_message(
                                    std::iter::once(*viewer),
                                    NetworkMessageS2C::SetBlock {
                                        position: place_position,
                                        block,
                                    },
                                );
                            }
                        },
                        |_, _, _| {
                            //todo
                        },
                    );
                }
                _ => {
                    println!("unknown command");
                }
            }
        }

        let chunks: Vec<_> = server.chunks.keys().cloned().collect();
        for center in chunks {
            let world_access = WorldAccess::lock(
                center,
                &mut server.chunks,
                server.ticks_passed,
                &server.users,
                &server.message_queue,
            );
            tick_chunk(&world_access);
        }

        chunk_viewing_manager.manage(&mut server, &database, &world_generator);

        network_server.broadcast_message(
            0,
            MessageQueue::encode_message(NetworkMessageS2C::GameTick {
                ticks_passed: server.ticks_passed,
                mspt: tick_start_time.elapsed().as_secs_f32() * 1000.,
            }),
        );

        {
            let mut skipping_users = HashSet::new();
            server
                .message_queue
                .0
                .get_mut()
                .retain(|(user, message, block_related)| {
                    let client_id = match server.users.get(*user) {
                        Some(user) => user.client_id,
                        None => return false,
                    };
                    if *block_related {
                        if skipping_users.contains(user) {
                            return true;
                        }
                        //1000 buffer for broadcast message
                        if network_server.channel_available_memory(client_id, 1)
                            < message.len() + 1000
                        {
                            skipping_users.insert(*user);
                            return true;
                        }
                        network_server.send_message(client_id, 1, message.clone());
                    } else {
                        network_server.send_message(client_id, 0, message.clone());
                    }
                    false
                });
        }

        network_transport.send_packets(&mut network_server);

        let sleep_time = (server.ticks_passed as i64 * 1000 / SERVER_TPS as i64)
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
pub struct MessageQueue(Mutex<Vec<(UserIndex, renet::Bytes, bool)>>);
impl MessageQueue {
    pub fn encode_message(message: NetworkMessageS2C) -> Bytes {
        let mut wtr = lz4_flex::frame::FrameEncoder::new(Vec::new());
        let _message = serde_cbor::to_writer(&mut wtr, &message).unwrap();
        wtr.finish().unwrap().into()
    }
    pub fn send_message<T: std::borrow::Borrow<UserIndex>>(
        &self,
        users: impl Iterator<Item = T>,
        message: NetworkMessageS2C,
    ) {
        let block_related = message.is_block_related();
        let message = Self::encode_message(message);
        let mut message_queue = self.0.lock();
        for user in users {
            message_queue.push((*user.borrow(), message.clone(), block_related));
        }
    }
}
pub struct Server {
    pub ticks_passed: u64,
    pub chunks: ahash::HashMap<ChunkPos, Chunk>,
    pub users: SlotMap<UserIndex, User>,
    pub message_queue: MessageQueue,
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
                blocks: chunk.blocks.get_mut().clone(),
                components: chunk.components.client(),
            };
            for (_, entity) in &mut chunk.entities {
                let add_message = entity.get_mut().create_add_message();
                server
                    .message_queue
                    .send_message(std::iter::once(user), add_message);
            }
            server
                .message_queue
                .send_message(std::iter::once(user), message);
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
        server.message_queue.send_message(
            std::iter::once(user),
            NetworkMessageS2C::UnloadChunk { position },
        );
    }
    pub fn manage(
        self,
        server: &mut Server,
        db: &Mutex<rusqlite::Connection>,
        world_generator: &WorldGenerator,
    ) {
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
                        block_events: chunk.events.into_inner(),
                        components: chunk.components,
                        entities: chunk
                            .entities
                            .into_iter()
                            .filter_map(|(_, entity)| {
                                let entity = entity.into_inner();
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
                let mut wtr = lz4_flex::frame::FrameEncoder::new(Vec::new());
                serde_cbor::to_writer(&mut wtr, &data).unwrap();
                (position, wtr.finish().unwrap())
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
        {
            let db = db.lock();
            let mut stmt = db
                .prepare_cached("SELECT data FROM chunks WHERE x=?1 and y=?2 and z=?3")
                .unwrap();
            for (position, users, load_message, mut chunk) in self
                .load
                .into_iter()
                .map(move |(position, users)| {
                    let data = stmt
                        .query_row((position.x, position.y, position.z), |row| {
                            row.get::<_, Vec<u8>>(0)
                        })
                        .ok();
                    (position, users, data)
                })
                .collect::<Vec<_>>()
                .into_par_iter()
                .map(|(position, users, data)| {
                    let mut chunk = match data.and_then(|data| {
                        let mut data = &data[..];
                        let rdr = lz4_flex::frame::FrameDecoder::new(&mut data);
                        match serde_cbor::from_reader::<ChunkSaveData, _>(rdr) {
                            Ok(data) => Some(data),
                            Err(_) => {
                                println!("loading {position:?} failed, regenerating");
                                None
                            }
                        }
                    }) {
                        Some(data) => Chunk {
                            blocks: ChunkBlocks::new(data.blocks),
                            events: RefCell::new(data.block_events),
                            components: data.components,
                            position,
                            viewers: HashSet::new(),
                            entities: data
                                .entities
                                .into_iter()
                                .map(|entity| (entity.uuid, WorldAccessCell::new(entity)))
                                .collect(),
                        },
                        None => generate_chunk(position, world_generator),
                    };
                    (
                        position,
                        users,
                        MessageQueue::encode_message(NetworkMessageS2C::LoadChunk {
                            position,
                            blocks: chunk.blocks.get_mut().clone(), //todo: this should be cow and not cloned
                            components: chunk.components.client(),
                        }),
                        chunk,
                    )
                })
                .collect::<Vec<_>>()
            {
                chunk.viewers = users.iter().cloned().collect();
                for entity in chunk.entities.values_mut() {
                    server
                        .message_queue
                        .send_message(users.iter(), entity.get_mut().create_add_message());
                }
                for user in &users {
                    server
                        .message_queue
                        .0
                        .get_mut()
                        .push((*user, load_message.clone(), true));
                }
                server.chunks.insert(position, chunk);
            }
        }
    }
}
new_key_type! {pub struct UserIndex;}
pub struct User {
    client_id: ClientId,
    view_position: Mutex<ChunkPos>,
    last_view_position: ChunkPos,
    entity: Option<Uuid>,
    teleport_id: AtomicU32,
    hud_sync_items: Mutex<Vec<Option<ClientItem>>>,
    screen: Mutex<Option<UserScreen>>,
    message_queue: Mutex<VecDeque<NetworkMessageC2S>>,
}
impl User {
    pub fn tick_controlling_entity(
        &self,
        entity: &mut Entity,
        controlling_user: UserIndex,
        world: &WorldAccess,
    ) {
        let entity_data = entity.key.data();
        let mut message_queue = self.message_queue.lock();
        while let Some(message) = message_queue.pop_back() {
            match message {
                NetworkMessageC2S::PlayerPosition {
                    position,
                    direction,
                    teleport_id,
                    pose,
                } => {
                    if self.teleport_id.load(Ordering::SeqCst) != teleport_id {
                        continue;
                    }
                    entity.direction = direction;
                    entity.pose = pose;
                    match world.teleport_entity(entity, position) {
                        Ok(_) => {
                            *self.view_position.lock() = position.to_chunk_pos();
                        }
                        Err(_) => {
                            let teleport_id = self
                                .teleport_id
                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
                                + 1;
                            world.send(
                                controlling_user,
                                NetworkMessageS2C::TeleportPlayer {
                                    position: entity.position,
                                    teleport_id,
                                },
                            );
                        }
                    }
                }
                NetworkMessageC2S::AttackBlock { position, face: _ } => {
                    let (damage_table, _) = compute_tool_damage_and_knockback(
                        entity.inventory.get_raw(entity.hand_slot),
                        &entity.current_stats,
                    );
                    let (chunk, offset) = position.to_chunk_pos_offset();
                    let _ = world.schedule_event(
                        chunk,
                        WorldEvent::BlockDamage {
                            block: offset,
                            damage: damage_table,
                            source_entity: Some(entity.uuid),
                        },
                    );
                }
                NetworkMessageC2S::ItemInteraction { target, variant } => {
                    let mut is_place = false;
                    let mut item_stack = &mut entity.inventory.items[entity.hand_slot];
                    if let Some(item) = &mut item_stack {
                        match &item.item.data().action {
                            ItemAction::Ignore => {}
                            ItemAction::Place(placements) => {
                                is_place = true;
                                let ItemInteractTarget::Block { position, face } = target else {
                                    continue;
                                };
                                let Some(place) = placements.get(variant) else {
                                    continue;
                                };
                                if item.count < place.use_count {
                                    continue;
                                }
                                if let Some(research) = place.research {
                                    if !entity.research.contains(&research) {
                                        continue;
                                    }
                                }
                                let position = position + face.get_block_offset();
                                let block = BlockEntry {
                                    block: place.block,
                                    color: BlockColor::default(),
                                    rotation: place
                                        .block
                                        .data()
                                        .rotation
                                        .from_look_direction(entity.direction, face),
                                };
                                let entity_collider =
                                    entity_data.hitbox(entity.pose).offset(entity.position);
                                if block.colliders(position).any(|block_collider| {
                                    block_collider.intersects(entity_collider)
                                }) {
                                    continue;
                                }
                                let mut blocked = false;
                                for other_entity in world.iter_entities(&[entity.uuid], false) {
                                    let entity_collider = other_entity.get_hitbox();
                                    if block.colliders(position).any(|block_collider| {
                                        block_collider.intersects(entity_collider)
                                    }) {
                                        blocked = true;
                                        break;
                                    }
                                }
                                if blocked {
                                    continue;
                                }
                                if let Ok(_) = world.place_block(position, block) {
                                    item.count -= place.use_count;
                                }
                            }
                            ItemAction::SpawnEntity(key) => {
                                let ItemInteractTarget::Block { position, face: _ } = target else {
                                    continue;
                                };
                                world
                                    .spawn_entity(Entity::new(
                                        *key,
                                        position.to_pos() + Pos::Y * 0.5,
                                    ))
                                    .unwrap();
                                item.count -= 1;
                            }
                            ItemAction::Plant(_key) => todo!(),
                            ItemAction::RotateBlock => {
                                let ItemInteractTarget::Block { position, face: _ } = target else {
                                    continue;
                                };
                                let Some(mut block) = world.get_block(position) else {
                                    continue;
                                };
                                let block_data = block.block.data();
                                let mut i = block.rotation as usize;
                                while i < 48 {
                                    i += 1;
                                    let new_rotation: BlockRotation =
                                        unsafe { std::mem::transmute((i % 24) as u8) };
                                    if block_data.rotation.get_nearest_valid(new_rotation)
                                        == new_rotation
                                    {
                                        if let Some(hanging) = block_data.hanging {
                                            let hanging_face = new_rotation.rotate_face(hanging);
                                            let Some(support_block) = world.get_block(
                                                hanging_face.get_block_offset() + position,
                                            ) else {
                                                continue;
                                            };
                                            if !support_block.supports(hanging.opposite()) {
                                                continue;
                                            }
                                        }
                                        //todo: check collisions?
                                        block.rotation = new_rotation;
                                        world.replace_block(position, block).unwrap();
                                        break;
                                    }
                                }
                            }
                            ItemAction::Consume {
                                effects,
                                effect_duration,
                            } => {
                                entity.effects.push(ActiveEffect {
                                    stats: effects.clone(),
                                    timer: time_to_ticks(*effect_duration),
                                });
                                item.count -= 1;
                            }
                        }
                        if item.count == 0 {
                            *item_stack = None;
                        }
                    }
                    world.send_viewers(
                        entity.position.to_chunk_pos(),
                        NetworkMessageS2C::EntityAction {
                            entity: entity.uuid,
                            action: if is_place {
                                EntityAction::Place
                            } else {
                                EntityAction::Interact
                            },
                        },
                    );
                }
                NetworkMessageC2S::CloseUI => {
                    if let Some(screen) = self.screen.lock().as_mut() {
                        screen.state = UserScreenState::Close;
                    }
                }
                NetworkMessageC2S::HotbarSelect { slot } => {
                    entity.hand_slot = slot;
                    entity.current_stats_dirty = true;
                }
                NetworkMessageC2S::InteractBlock { position } => {
                    let Some(block) = world.get_block(position) else {
                        continue;
                    };
                    let block_data = block.block.data();
                    match &block_data.interact_action {
                        BlockInteractAction::Ignore => {}
                        BlockInteractAction::OpenInventory {
                            screen: screen_key,
                            view,
                        } => {
                            self.set_screen(
                                *screen_key,
                                vec![
                                    (
                                        InventoryProvider::Entity(entity.uuid),
                                        InventoryView::from_range(0..10),
                                    ),
                                    (InventoryProvider::Block(position), view.clone()),
                                ],
                            );
                        }
                        BlockInteractAction::Pickup => {
                            let Ok(drops) = world.break_block(position) else {
                                continue;
                            };
                            for item in drops {
                                if let Some(rest) = entity
                                    .inventory
                                    .add_item(entity.key.data().pickup_view(), item)
                                {
                                    world.drop_items(
                                        std::iter::once(rest),
                                        position.to_pos() + Pos::all(0.5),
                                    );
                                }
                            }
                            entity.current_stats_dirty = true;
                        }
                        BlockInteractAction::ModifyProperty {
                            property,
                            value,
                            mode,
                        } => {
                            let Some(mut machine) =
                                world.get_block_component::<BlockMachine>(position)
                            else {
                                continue;
                            };
                            let machine_data = world
                                .get_block(position)
                                .unwrap()
                                .block
                                .data()
                                .machine
                                .as_ref()
                                .unwrap();
                            machine.modify_property(machine_data, &property, *value, *mode);
                            world.wakeup_component::<BlockMachine>(position);
                        }
                    }
                }
                NetworkMessageC2S::InteractEntity {
                    entity: other_entity_uuid,
                } => {
                    let Some(mut other_entity) = world.get_entity(other_entity_uuid) else {
                        continue;
                    };
                    let other_entity_data = other_entity.key.data();
                    match other_entity_data.interact_action {
                        EntityInteractAction::Ignore => {}
                        EntityInteractAction::Pickup => {
                            let mut items_present = false;
                            let mut player_view = entity.inventory.full_view();
                            player_view.slots.retain(|slot| {
                                !entity.key.data().equipment_slots.contains(&slot.slot)
                            });
                            for slot in &mut other_entity.inventory.items {
                                if let Some(item) = &slot {
                                    *slot = entity.inventory.add_item(&player_view, item.clone());
                                    if slot.is_some() {
                                        items_present = true;
                                    }
                                }
                            }
                            entity.current_stats_dirty = true;
                            if !items_present {
                                world.remove_entity(other_entity);
                            }
                        }
                    }
                }
                NetworkMessageC2S::AttackEntity {
                    entity: other_entity_id,
                } => {
                    let (damage_table, knockback) = compute_tool_damage_and_knockback(
                        entity.inventory.get_raw(entity.hand_slot),
                        &entity.current_stats,
                    );
                    world
                        .schedule_event(
                            world.center_chunk,
                            WorldEvent::EntityDamage {
                                entity: other_entity_id,
                                damage: damage_table,
                                source_entity: Some(entity.uuid),
                            },
                        )
                        .unwrap();
                    let knockback_direction = entity.direction.make_front();
                    world
                        .schedule_event(
                            world.center_chunk,
                            WorldEvent::EntityKnockback {
                                entity: other_entity_id,
                                knockback: (knockback_direction + Pos::Y * 0.5) * knockback,
                            },
                        )
                        .unwrap();
                }
                NetworkMessageC2S::DropItem { stack } => {
                    let slot = entity.inventory.get_slot_mut_raw(entity.hand_slot);
                    let drop_item = if let Some(item) = slot {
                        if item.count == 1 || stack {
                            slot.take().unwrap()
                        } else {
                            item.count -= 1;
                            item.copy(1)
                        }
                    } else {
                        continue;
                    };
                    entity.current_stats_dirty = true;
                    let mut item_entity =
                        Entity::new(EntityKey::id("item").unwrap(), entity.get_eye());
                    item_entity.direction = LookDirection {
                        pitch: 0.,
                        yaw: entity.direction.yaw,
                    };
                    let throw_force = 10.;
                    let mut throw_velocity = entity.direction.make_front() * throw_force;
                    throw_velocity.y = 0.1;
                    item_entity.character_controller.velocity = throw_velocity;
                    item_entity.inventory.items[0] = Some(drop_item);
                    world.spawn_entity(item_entity);
                }
                NetworkMessageC2S::MoveItem { from, to, mode } => {
                    if from == to {
                        continue;
                    }
                    let screen = self.screen.lock();
                    let Some(screen) = &*screen else {
                        continue;
                    };
                    let Some((from_provider, from_index, from_slot)) = screen.get_slot(from) else {
                        continue;
                    };
                    let Some((to_provider, to_index, to_slot)) = screen.get_slot(to) else {
                        continue;
                    };
                    let move_item = |src: &mut Option<ItemStack>,
                                     dst: &mut Option<ItemStack>,
                                     src_viewslot: &ViewSlot,
                                     dst_viewslot: &ViewSlot| {
                        if !src_viewslot.output || !dst_viewslot.input {
                            return;
                        }
                        if let Some(source) = src.as_mut() {
                            let count = mode.get_count(source.count);
                            if let Some(destination) = dst.as_mut() {
                                if let Some((a, b)) =
                                    destination.merge(&source.copy(count), dst_viewslot)
                                {
                                    *destination = a;
                                    source.count += b.map(|item| item.count).unwrap_or(0);
                                    source.count -= count;
                                    if source.count == 0 {
                                        *src = None;
                                    }
                                } else if mode.can_swap() {
                                    if !(src_viewslot.input
                                        && src_viewslot.output
                                        && dst_viewslot.input
                                        && dst_viewslot.output)
                                    {
                                        return;
                                    }
                                    match dst_viewslot.filter {
                                        Some(filter) => {
                                            if !filter.contains(source.item) {
                                                return;
                                            }
                                        }
                                        None => {}
                                    }
                                    match src_viewslot.filter {
                                        Some(filter) => {
                                            if !filter.contains(destination.item) {
                                                return;
                                            }
                                        }
                                        None => {}
                                    }
                                    match dst_viewslot.stack_size_override {
                                        Some(stack_size_override) => {
                                            if source.count > stack_size_override {
                                                return;
                                            }
                                        }
                                        None => {}
                                    }
                                    match src_viewslot.stack_size_override {
                                        Some(stack_size_override) => {
                                            if destination.count > stack_size_override {
                                                return;
                                            }
                                        }
                                        None => {}
                                    }
                                    std::mem::swap(source, destination);
                                }
                            } else {
                                match dst_viewslot.filter {
                                    Some(filter) => {
                                        if !filter.contains(source.item) {
                                            return;
                                        }
                                    }
                                    None => {}
                                }
                                let count =
                                    count.min(dst_viewslot.stack_size_override.unwrap_or(u16::MAX));
                                if count >= source.count {
                                    *dst = src.take();
                                } else {
                                    let (a, b) = source.split(count);
                                    *dst = Some(a);
                                    *source = b;
                                }
                            }
                        }
                    };
                    let user_inventory = RefCell::new(&mut entity.inventory);
                    let mut from_provided = ProvidedInventory::lock(
                        &from_provider,
                        world,
                        entity.uuid,
                        &user_inventory,
                    )
                    .unwrap();
                    if from_provider == to_provider {
                        let [src, dst] = from_provided
                            .get_mut()
                            .items
                            .get_disjoint_mut([from_index, to_index])
                            .unwrap();
                        move_item(src, dst, from_slot, to_slot);
                    } else {
                        let mut to_provided = ProvidedInventory::lock(
                            &to_provider,
                            world,
                            entity.uuid,
                            &user_inventory,
                        )
                        .unwrap();
                        move_item(
                            &mut from_provided.get_mut().items[from_index],
                            &mut to_provided.get_mut().items[to_index],
                            from_slot,
                            to_slot,
                        );
                    }
                    entity.current_stats_dirty = true;
                }
                NetworkMessageC2S::Research { research } => {
                    let screen = self.screen.lock();
                    let Some(screen) = &*screen else {
                        continue;
                    };
                    let user_inventory = RefCell::new(&mut entity.inventory);
                    let Ok(mut list) = ProvidedInventoryList::lock_screen(
                        screen,
                        world,
                        entity.uuid,
                        &user_inventory,
                    ) else {
                        continue;
                    };
                    if entity.research.contains(&research) {
                        continue;
                    }
                    let research_data = research.data();
                    let mut failed = false;
                    for dependency in &research_data.dependencies {
                        if !entity.research.contains(dependency) {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        continue;
                    }
                    for (req_item, req_count) in &research_data.requirements {
                        if list.count_item(*req_item) < *req_count {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        continue;
                    }
                    for (req_item, req_count) in &research_data.requirements {
                        assert_eq!(list.remove_item(*req_item, *req_count), 0);
                    }
                    entity.research.insert(research);
                    world.send(
                        controlling_user,
                        NetworkMessageS2C::UpdateResearch {
                            research: entity.research.clone(),
                        },
                    );
                    entity.current_stats_dirty = true;
                }
                NetworkMessageC2S::Craft { recipe, mut count } => {
                    let screen = self.screen.lock();
                    let Some(screen) = &*screen else {
                        continue;
                    };
                    let user_inventory = RefCell::new(&mut entity.inventory);
                    let Ok(mut list) = ProvidedInventoryList::lock_screen(
                        screen,
                        world,
                        entity.uuid,
                        &user_inventory,
                    ) else {
                        continue;
                    };
                    let recipe = recipe.data();
                    for (input_item, input_count) in &recipe.inputs {
                        count = count.min(list.count_item(*input_item) / *input_count);
                        if count == 0 {
                            break;
                        }
                    }
                    if count == 0 {
                        continue;
                    }
                    for (input_item, input_count) in &recipe.inputs {
                        assert_eq!(list.remove_item(*input_item, *input_count * count), 0);
                    }
                    for _ in 0..count {
                        for item in generate_loot_table(
                            recipe.outputs.data(),
                            &mut LootGenerationContext::new(rand::random()),
                        ) {
                            if let Some(overflow_item) = list.add_item(item) {
                                world.drop_items(
                                    std::iter::once(overflow_item),
                                    entity.position + Pos::Y * entity_data.hitbox_height / 2.,
                                );
                            }
                        }
                    }
                    entity.current_stats_dirty = true;
                }
                NetworkMessageC2S::OpenPlayerInventory => {
                    self.set_screen(
                        Key::id("player_creative").unwrap(),
                        vec![(
                            InventoryProvider::Entity(entity.uuid),
                            InventoryView::from_range(0..10),
                        )],
                    );
                }
                NetworkMessageC2S::HarvestPlant {
                    position: _,
                    index: _,
                } => todo!(),
                NetworkMessageC2S::UIButtonPress {
                    property,
                    value,
                    modify_mode,
                } => {
                    let screen_lock = self.screen.lock();
                    if let Some(screen_lock) = screen_lock.as_ref() {
                        if !screen_lock
                            .screen
                            .data()
                            .button_properties
                            .contains(&property)
                        {
                            continue;
                        }
                        for (provider, _) in &screen_lock.inventories {
                            match provider {
                                InventoryProvider::Entity(_uuid) => {}
                                InventoryProvider::Block(position) => {
                                    let Some(mut machine) =
                                        world.get_block_component::<BlockMachine>(*position)
                                    else {
                                        continue;
                                    };
                                    let machine_data = world
                                        .get_block(*position)
                                        .unwrap()
                                        .block
                                        .data()
                                        .machine
                                        .as_ref()
                                        .unwrap();
                                    machine.modify_property(
                                        machine_data,
                                        &property,
                                        value,
                                        modify_mode,
                                    );
                                    world.wakeup_component::<BlockMachine>(*position);
                                }
                            }
                        }
                    }
                }
                NetworkMessageC2S::TrashItem { slot, mode } => {
                    let screen = self.screen.lock();
                    let Some(screen) = &*screen else {
                        continue;
                    };
                    let user_inventory = RefCell::new(&mut entity.inventory);
                    let Some((provider, index, slot)) = screen.get_slot(slot) else {
                        continue;
                    };
                    if !slot.output {
                        continue;
                    }
                    let mut provided =
                        ProvidedInventory::lock(&provider, world, entity.uuid, &user_inventory)
                            .unwrap();
                    let item_slot = &mut provided.get_mut().items[index];
                    if let Some(item) = item_slot {
                        item.count -= mode.get_count(item.count);
                        if item.count <= 0 {
                            *item_slot = None;
                        }
                    }
                    entity.current_stats_dirty = true;
                }
                NetworkMessageC2S::GiveItem { item, stack } => {
                    let screen = self.screen.lock();
                    let Some(screen) = &*screen else {
                        continue;
                    };
                    let user_inventory = RefCell::new(&mut entity.inventory);
                    let Ok(mut list) = ProvidedInventoryList::lock_screen(
                        screen,
                        world,
                        entity.uuid,
                        &user_inventory,
                    ) else {
                        continue;
                    };
                    list.add_item(ItemStack::new(
                        item,
                        if stack { item.data().stack_size } else { 1 },
                    ));
                    entity.current_stats_dirty = true;
                }
            }
        }
    }
}
pub struct UserScreen {
    pub screen: UIScreenKey,
    pub inventories: Vec<(InventoryProvider, InventoryView)>,
    pub state: UserScreenState,
    pub previous_state: Vec<Option<ClientItem>>,
    pub previous_properties: PropertyMap,
}
impl UserScreen {
    pub fn get_slot<'a>(&'a self, slot: usize) -> Option<(InventoryProvider, usize, &'a ViewSlot)> {
        let mut running_index = 0;
        for (inventory, view) in &self.inventories {
            if slot < running_index + view.size() {
                let i = slot - running_index;
                return Some((*inventory, i, &view.slots[i]));
            }
            running_index += view.size();
        }
        None
    }
}
pub enum UserScreenState {
    Open,
    Normal,
    Close,
}
#[derive(PartialEq, Clone, Copy)]
pub enum InventoryProvider {
    Entity(Uuid),
    Block(BlockPos),
}
enum ProvidedInventory<'a> {
    Block(WorldAccessRef<'a, BlockMachine, BlockPos>),
    Entity(WorldAccessRef<'a, Entity, Uuid>),
    RefMut(std::cell::RefMut<'a, &'a mut Inventory>),
}
impl ProvidedInventory<'_> {
    pub fn lock<'a>(
        provider: &InventoryProvider,
        world: &'a WorldAccess,
        user_id: Uuid,
        user_inventory: &'a RefCell<&'a mut Inventory>,
    ) -> Result<ProvidedInventory<'a>, ()> {
        match provider {
            InventoryProvider::Entity(uuid) => {
                if *uuid == user_id {
                    Ok(ProvidedInventory::RefMut(user_inventory.borrow_mut()))
                } else {
                    let Some(entity) = world.get_entity(*uuid) else {
                        return Err(());
                    };
                    Ok(ProvidedInventory::Entity(entity))
                }
            }
            InventoryProvider::Block(position) => {
                let Some(machine) = world.get_block_component::<BlockMachine>(*position) else {
                    return Err(());
                };
                Ok(ProvidedInventory::Block(machine))
            }
        }
    }
    pub fn get(&self) -> &Inventory {
        match self {
            ProvidedInventory::Block(block) => &block.inventory,
            ProvidedInventory::Entity(entity) => &entity.inventory,
            ProvidedInventory::RefMut(ref_mut) => ref_mut,
        }
    }
    pub fn get_mut(&mut self) -> &mut Inventory {
        match self {
            ProvidedInventory::Block(block) => &mut block.inventory,
            ProvidedInventory::Entity(entity) => &mut entity.inventory,
            ProvidedInventory::RefMut(ref_mut) => ref_mut,
        }
    }
}
pub struct ProvidedInventoryList<'a>(Vec<(ProvidedInventory<'a>, &'a InventoryView)>);
impl ProvidedInventoryList<'_> {
    pub fn lock_screen<'a>(
        screen: &'a UserScreen,
        world: &'a WorldAccess,
        user_id: Uuid,
        user_inventory: &'a RefCell<&'a mut Inventory>,
    ) -> Result<ProvidedInventoryList<'a>, ()> {
        Ok(ProvidedInventoryList(
            screen
                .inventories
                .iter()
                .map(|(provider, view)| {
                    Ok((
                        ProvidedInventory::lock(provider, world, user_id, user_inventory)?,
                        view,
                    ))
                })
                .collect::<Result<Vec<(ProvidedInventory, &InventoryView)>, ()>>()?,
        ))
    }
    pub fn add_item(&mut self, mut item: ItemStack) -> Option<ItemStack> {
        for (inv, view) in &mut self.0 {
            match inv.get_mut().add_item(view, item) {
                Some(overflow) => item = overflow,
                None => return None,
            }
        }
        Some(item)
    }
    pub fn count_item(&self, item: ItemKey) -> ItemCount {
        self.0
            .iter()
            .map(|(provided, view)| provided.get().count_removeable_items(view, item))
            .sum()
    }
    pub fn remove_item(&mut self, item: ItemKey, mut count: ItemCount) -> ItemCount {
        for (inv, view) in &mut self.0 {
            count = inv.get_mut().remove_item(view, item, count);
            if count == 0 {
                return 0;
            }
        }
        count
    }
}
impl User {
    pub fn set_screen(
        &self,
        screen: UIScreenKey,
        inventories: Vec<(InventoryProvider, InventoryView)>,
    ) {
        *self.screen.lock() = Some(UserScreen {
            previous_state: (0..(inventories.iter().map(|(_, view)| view.size()).sum()))
                .map(|_| None)
                .collect(),
            inventories,
            screen,
            state: UserScreenState::Open,
            previous_properties: PropertyMap(HashMap::new()),
        });
    }
    pub fn loading_area_for_view_position(view_position: ChunkPos) -> AABB<i16> {
        let config = ServerConfig::config();
        AABB {
            min: ChunkPos {
                x: view_position.x - config.view_distance,
                y: 0,
                z: view_position.z - config.view_distance,
            },
            max: ChunkPos {
                x: view_position.x + config.view_distance,
                y: config.world_chunk_height - 1,
                z: view_position.z + config.view_distance,
            },
        }
    }
}
use serde_default_utils::*;
#[derive(Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_i16::<8>")]
    view_distance: i16,
    #[serde(default = "default_i16::<12>")]
    world_chunk_height: i16,
}
static SERVER_CONFIG_INSTANCE: OnceLock<ServerConfig> = OnceLock::new();
impl ServerConfig {
    pub fn config() -> &'static ServerConfig {
        SERVER_CONFIG_INSTANCE.get().unwrap()
    }
}
