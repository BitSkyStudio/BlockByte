use std::{
    cell::{RefCell, UnsafeCell},
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
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
        ItemKey, KeyGroup, PrefabData, PrefabEntry, ToolData, air_block, load_registries,
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
use renet_netcode::{NetcodeServerTransport, ServerAuthentication};
use ron::ser::PrettyConfig;
use serde::Deserialize;
use slotmap::{SlotMap, new_key_type};
use smallvec::SmallVec;
use uuid::Uuid;

use crate::{
    inventory::{
        Inventory, ItemCount, ItemDurability, ItemQuality, ItemStack, generate_loot_table,
    },
    registry::{Key, REGISTRIES, Registry, RegistryProvider, RegistryStorage},
    world::{
        BlockMachine, BlockPlants, Chunk, ChunkBlockComponents, ChunkSaveData, Entity, WorldAccess,
        WorldAccessCell, WorldAccessRef, WorldEvent, tick_chunk,
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
        let mut config: ServerConfig = ron::from_str(
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
    let world_generator = WorldGenerator::new(world_generator_config, 1);
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
            let mut entity = Entity::new(Key::id("player").unwrap(), spawn_position);
            let full_view = entity.inventory.full_view();
            entity.controlling_user = Some(user);
            let player_uuid = entity.uuid;
            let add_message = entity.create_add_message();
            let chunk_position = entity.position.to_chunk_pos();
            let mut chunk = server.chunks.get_mut(&chunk_position).unwrap();
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
                NetworkMessageS2C::PlayerAbilities {
                    abilities: PlayerAbilities {
                        move_mode: MoveMode::Normal,
                        speed: 1.,
                        max_stamina: 100.,
                    },
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
                    let spawn_position = Pos {
                        x: 0.,
                        y: 90.,
                        z: 0.,
                    };
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
        for (user_id, user) in &server.users {
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
                    //let mut blocks = HashMap::new();
                    /*for position in AABB::new(from, to) {
                        let block = server.get_block(position).unwrap();
                        if block.block != air_block() {
                            blocks.insert(position - center, block);
                        }
                    }*/
                    /*println!(
                        "{}",
                        ron::ser::to_string_pretty(
                            &PrefabData {
                                parts: vec![PrefabEntry { blocks, chance: 1. }],
                                bb: OnceLock::new(),
                            },
                            PrettyConfig::new()
                        )
                        .unwrap()
                    );*/
                }
                _ => {
                    println!("unknown command");
                }
            }
        }

        let chunks: Vec<_> = server.chunks.keys().cloned().collect();
        for center in chunks {
            let mut world_access = WorldAccess::lock(
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
                blocks: chunk.blocks.borrow().clone(),
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
                                events: RefCell::new(data.block_events),
                                components: data.components,
                                position,
                                viewers: HashSet::new(),
                                entities: BTreeMap::new(),
                            },
                            data.entities,
                        ),
                        None => (generate_chunk(position, world_generator), Vec::new()),
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
                    server
                        .message_queue
                        .send_message(users.iter(), entity.create_add_message());
                    chunk
                        .entities
                        .insert(entity.uuid, WorldAccessCell::new(entity));
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
pub struct UserScreen {
    pub screen: UIScreenKey,
    pub inventories: Vec<(InventoryProvider, InventoryView)>,
    pub state: UserScreenState,
    pub previous_state: Vec<Option<ClientItem>>,
    pub previous_properties: PropertyMap,
}
impl UserScreen {
    pub fn get_slot(&self, slot: usize) -> Option<(InventoryProvider, usize)> {
        let mut running_index = 0;
        for (inventory, view) in &self.inventories {
            if slot < running_index + view.size() {
                return Some((*inventory, slot - running_index));
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
                let Some(mut machine) = world.get_block_component::<BlockMachine>(*position) else {
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
            .map(|(provided, view)| provided.get().count_item(view, item))
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
    #[serde(skip_deserializing, default)]
    is_structure_creation_world: bool,
}
static SERVER_CONFIG_INSTANCE: OnceLock<ServerConfig> = OnceLock::new();
impl ServerConfig {
    pub fn config() -> &'static ServerConfig {
        SERVER_CONFIG_INSTANCE.get().unwrap()
    }
}
