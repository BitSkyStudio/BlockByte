use std::{
    collections::{HashMap, HashSet},
    env::args,
    net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket},
    path::Path,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicUsize},
    },
    time::{Duration, Instant, SystemTime},
};

use block_byte_common::{
    ClientItem, Color, DEFAULT_VIEWSLOT, InventoryView, LookDirection, MoveMode, PlayerAbilities,
    SERVER_DT, SERVER_TPS,
    coord::{AABB, BlockPos, CHUNK_SIZE, ChunkOffset, ChunkPos, Face, Orientation, Pos},
    net::{NetworkMessageC2S, NetworkMessageS2C, make_connection_config},
    registry::{
        self, BlockColor, BlockData, BlockEntry, BlockKey, BlockRotation, EntityKey, ItemAction,
        ItemKey, KeyGroup, PrefabData, PrefabPart, ToolData, air_block, load_registries,
    },
    scripts::{ScriptState, ScriptValue},
    ui::{PropertyMap, UIScreenKey},
    world::{
        BlockComponentStorage, ClientBlockComponentUpdate, ClientBlockDamage, ClientBlockPlants,
    },
};
use palettevec::PaletteVec;
use parking_lot::{Mutex, RwLock};
use rand::{Rng, rngs::StdRng};
use rand_seeder::Seeder;
use rayon::iter::{
    IntoParallelIterator, IntoParallelRefIterator, ParallelBridge, ParallelIterator,
};
use renet::{
    Bytes, ChannelConfig, ClientId, ConnectionConfig, DefaultChannel, RenetServer, ServerEvent,
};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};
use ron::ser::PrettyConfig;
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};
use smallvec::SmallVec;
use uuid::Uuid;

use crate::{
    inventory::{Inventory, ItemDurability, ItemQuality, ItemStack, generate_loot_table},
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::{
        BlockMachine, BlockPlants, Chunk, ChunkBlockComponents, ChunkSaveData, Entity, EntityEvent,
        EntityIndex, WorldEvent, WorldGenerator,
    },
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
    if let Some(arg) = args().nth(1) {
        let mut start = false;
        match arg.as_str() {
            "worldgen_vis" => {
                debug::visualise();
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

    let world_generator_config = ron::from_str(
        std::fs::read_to_string("assets/world_generator.ron")
            .unwrap()
            .as_str(),
    )
    .unwrap();
    let world_generator = WorldGenerator::new(world_generator_config, 1);
    let mut server = Server {
        ticks_passed: 0,
        chunks: HashMap::new(),
        users: SlotMap::with_key(),
        message_queue: Default::default(),
        entities: SlotMap::with_key(),
        entity_add_queue: Mutex::new(Vec::new()),
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
            let mut entity = Entity::new(Key::id("player").unwrap(), spawn_position);
            let full_view = entity.inventory.get_mut().full_view();
            entity.inventory.get_mut().add_item(
                &full_view,
                ItemStack::new(ItemKey::id("arrow").unwrap(), 20),
            );
            entity.inventory.get_mut().add_item(
                &full_view,
                ItemStack::new(ItemKey::id("rock.limestone.stone").unwrap(), 80),
            );
            entity.controlling_user = Some(user);
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
                        max_stamina: 100.,
                    },
                },
            );
            server.send_message(
                user,
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
                    let spawn_position = Pos {
                        x: 0.,
                        y: 90.,
                        z: 0.,
                    };
                    let user = server.users.insert(User {
                        client_id,
                        view_position: Mutex::new(spawn_position.to_chunk_pos()),
                        entity: None,
                        teleport_id: AtomicU32::new(1),
                        screen: Mutex::new(None),
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
                            .schedule_event(EntityEvent::Remove);
                    }
                }
            }
        }
        let mut view_chunk_chunk_changed_users: HashMap<UserIndex, (ChunkPos, ChunkPos)> =
            HashMap::new();
        for (user_id, user) in &server.users {
            while let Some(message) = network_server.receive_message(user.client_id, 0) {
                let message: NetworkMessageC2S = serde_cbor::from_slice(&message).unwrap();
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
                "structure_export" => {
                    let args = command[1..]
                        .iter()
                        .map(|n| n.parse().unwrap())
                        .collect::<Vec<i32>>();
                    let from = BlockPos {
                        x: args[0],
                        y: args[1],
                        z: args[2],
                    };
                    let to = BlockPos {
                        x: args[3],
                        y: args[4],
                        z: args[5],
                    };
                    let center = BlockPos {
                        x: args[6],
                        y: args[7],
                        z: args[8],
                    };
                    let mut blocks = HashMap::new();
                    for position in AABB::new(from, to) {
                        let block = server.get_block(position).unwrap();
                        if block.block != air_block() {
                            blocks.insert(position - center, block);
                        }
                    }
                    println!(
                        "{}",
                        ron::ser::to_string_pretty(
                            &PrefabData {
                                parts: vec![PrefabPart { blocks, chance: 1. }],
                                bb: OnceLock::new(),
                            },
                            PrettyConfig::new()
                        )
                        .unwrap()
                    );
                }
                _ => {
                    println!("unknown command");
                }
            }
        }

        for (user_index, user) in &server.users {
            {
                if let Some(entity) = user
                    .entity
                    .as_ref()
                    .and_then(|entity| server.entities.get_mut(*entity))
                {
                    let inventory: Vec<_> = entity
                        .inventory
                        .get_mut()
                        .items
                        .iter()
                        .map(|item| item.as_ref().map(|item| item.client()))
                        .collect();
                    for (i, item) in inventory.into_iter().enumerate() {
                        server.message_queue.send_message(
                            std::iter::once(user_index),
                            NetworkMessageS2C::HUDSlot { slot: i, item },
                        );
                    }
                    server.message_queue.send_message(
                        std::iter::once(user_index),
                        NetworkMessageS2C::HudBarUpdate {
                            health: entity.state.get_mut().health,
                        },
                    );
                }
            }
            let mut screen_lock = user.screen.lock();
            if let Some(screen) = &mut *screen_lock {
                match screen.state {
                    UserScreenState::Open => {
                        let Ok(items) = screen.get_items(&mut server.entities, &mut server.chunks)
                        else {
                            *screen_lock = None;
                            continue;
                        };
                        server.message_queue.send_message(
                            std::iter::once(user_index),
                            NetworkMessageS2C::UIOpen {
                                screen: screen.screen,
                                slots: items
                                    .iter()
                                    .map(|item| item.as_ref().map(|item| item.client()))
                                    .collect(),
                            },
                        );
                        screen.previous_state = items;
                        screen.state = UserScreenState::Normal;
                    }
                    UserScreenState::Normal => {
                        let Ok(items) = screen.get_items(&mut server.entities, &mut server.chunks)
                        else {
                            *screen_lock = None;
                            continue;
                        };
                        for (slot, (previous, new)) in
                            screen.previous_state.iter().zip(items.iter()).enumerate()
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
                        screen.previous_state = items;
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
            if entity.state.get_mut().removed {
                {
                    let mut chunk_entities = &mut server
                        .chunks
                        .get_mut(&entity.position.to_chunk_pos())
                        .unwrap()
                        .entities;
                    chunk_entities
                        .remove(chunk_entities.iter().position(|ce| *ce == index).unwrap());
                }
                server.message_queue.send_message(
                    server
                        .chunks
                        .get(&entity.position.to_chunk_pos())
                        .unwrap()
                        .viewers
                        .iter(),
                    entity.create_remove_message(),
                );
                return false;
            }
            if let Some(teleport) = entity.state.get_mut().teleport.take() {
                if server.chunks.contains_key(&teleport.to_chunk_pos()) {
                    if entity.position.to_chunk_pos() != teleport.to_chunk_pos() {
                        let [previous_chunk, new_chunk] = server
                            .chunks
                            .get_disjoint_mut([
                                &entity.position.to_chunk_pos(),
                                &teleport.to_chunk_pos(),
                            ])
                            .map(|v| v.unwrap());
                        previous_chunk.entities.remove(
                            previous_chunk
                                .entities
                                .iter()
                                .position(|ce| *ce == index)
                                .unwrap(),
                        );
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
            true
        });

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
        let mut message = serde_cbor::to_writer(&mut wtr, &message).unwrap();
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
    pub chunks: HashMap<ChunkPos, Chunk>,
    pub users: SlotMap<UserIndex, User>,
    pub message_queue: MessageQueue,
    pub entities: SlotMap<EntityIndex, Entity>,
    entity_add_queue: Mutex<Vec<Entity>>,
}
impl Server {
    pub fn hitbox_block_collides(&self, hitbox: AABB<f32>) -> bool {
        for block in hitbox.to_block() {
            match self.get_block(block) {
                Some(block_entry) => {
                    let block_collider = &block_entry.block.data().collision;
                    for block_collider in block_collider {
                        if hitbox.intersects(
                            block_entry
                                .rotation
                                .rotate_aabb(*block_collider)
                                .offset(block.to_pos()),
                        ) {
                            return true;
                        }
                    }
                }
                None => return true,
            }
        }
        false
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
                        block_events: chunk.block_events.into_inner(),
                        components: chunk.components,
                        entities: chunk
                            .entities
                            .into_iter()
                            .filter_map(|entity| {
                                let mut entity = server.entities.remove(entity)?;
                                if entity.state.get_mut().removed {
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
            for (position, users, load_message, (mut chunk, entities)) in self
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
                        let mut rdr = lz4_flex::frame::FrameDecoder::new(&mut data);
                        match serde_cbor::from_reader::<ChunkSaveData, _>(rdr) {
                            Ok(data) => Some(data),
                            Err(_) => {
                                println!("loading {position:?} failed, regenerating");
                                None
                            }
                        }
                    }) {
                        Some(data) => (
                            Chunk {
                                blocks: RefCell::new(data.blocks),
                                block_events: RefCell::new(data.block_events),
                                components: data.components,
                                position,
                                viewers: HashSet::new(),
                                entities: RefCell::new(Vec::new()),
                            },
                            data.entities,
                        ),
                        None => (Chunk::generate(position, world_generator), Vec::new()),
                    };
                    (
                        position,
                        users,
                        MessageQueue::encode_message(NetworkMessageS2C::LoadChunk {
                            position,
                            blocks: chunk.0.blocks.get_mut().clone(),
                            components: chunk.0.components.client(),
                        }),
                        chunk,
                    )
                })
                .collect::<Vec<_>>()
            {
                chunk.viewers = users.iter().cloned().collect();
                for entity in entities {
                    server.send_message_multiple(users.iter(), entity.create_add_message());
                    chunk.entities.push(server.entities.insert(entity));
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
    entity: Option<EntityIndex>,
    teleport_id: AtomicU32,
    screen: Mutex<Option<UserScreen>>,
}
pub struct UserScreen {
    pub screen: UIScreenKey,
    pub inventories: Vec<(InventoryProvider, InventoryView)>,
    pub state: UserScreenState,
    pub previous_state: Vec<Option<ItemStack>>,
}
impl UserScreen {
    pub fn get_items(
        &self,
        entities: &mut SlotMap<EntityIndex, Entity>,
        chunks: &mut HashMap<ChunkPos, Chunk>,
    ) -> Result<Vec<Option<ItemStack>>, ()> {
        let mut items = Vec::new();
        for (inventory, view) in &self.inventories {
            let inventory = inventory.get_inventory(entities, chunks).ok_or(())?;
            for i in 0..view.size() {
                items.push(inventory.get_slot(view, i).cloned());
            }
        }
        Ok(items)
    }
}
pub enum UserScreenState {
    Open,
    Normal,
    Close,
}
#[derive(PartialEq, Clone, Copy)]
pub enum InventoryProvider {
    Entity(EntityIndex),
    Block(BlockPos),
}
impl InventoryProvider {
    pub fn modify_property_button(
        self,
        property: &str,
        value: ScriptValue,
        chunks: &mut HashMap<ChunkPos, Chunk>,
    ) {
        match self {
            InventoryProvider::Entity(entity) => {}
            InventoryProvider::Block(block) => {
                let (chunk, offset) = block.to_chunk_pos_offset();
                if let Some(chunk) = chunks.get_mut(&chunk) {
                    let block_data = chunk
                        .blocks
                        .get_mut()
                        .get(offset.index())
                        .unwrap()
                        .block
                        .data();
                    if let Some(machine) = chunk.components.machine.get_mut().get_mut(offset) {
                        if let Some(property) = block_data
                            .machine
                            .as_ref()
                            .unwrap()
                            .script
                            .named_registers
                            .iter()
                            .position(|r| r == property)
                        {
                            let mut register =
                                &mut machine.script_state.get_mut().registers[property];
                            *register = register.wrapping_add(value);
                        }
                    }
                }
            }
        }
    }
    pub fn get_inventory<'a>(
        self,
        entities: &'a mut SlotMap<EntityIndex, Entity>,
        chunks: &'a mut HashMap<ChunkPos, Chunk>,
    ) -> Option<&'a mut Inventory> {
        match self {
            InventoryProvider::Entity(entity) => {
                Some(entities.get_mut(entity)?.inventory.get_mut())
            }
            InventoryProvider::Block(block) => {
                let (chunk, offset) = block.to_chunk_pos_offset();
                Some(
                    chunks
                        .get_mut(&chunk)?
                        .components
                        .machine
                        .get_mut()
                        .get_mut(offset)?
                        .inventory
                        .get_mut(),
                )
            }
        }
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
        });
    }
    pub fn loading_area_for_view_position(view_position: ChunkPos) -> AABB<i16> {
        let distance = 8;
        let world_height = 12;
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
