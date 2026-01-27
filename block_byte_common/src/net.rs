use std::time::Duration;

use palettevec::PaletteVec;
use renet::{ChannelConfig, ConnectionConfig, SendType};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ClientItem, ItemMoveMode, LookDirection, PlayerAbilities,
    coord::{BlockPos, ChunkOffset, ChunkPos, Face, Pos},
    registry::{BlockEntry, BlockKey, BlockPalette, EntityKey, ToolData},
    ui::{PropertyMap, UIScreenKey},
    world::{ClientBlockComponentUpdate, ClientChunkBlockComponents},
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition {
        position: Pos,
        direction: LookDirection,
        teleport_id: u32,
    },
    AttackBlock {
        position: BlockPos,
    },
    PlaceBlock {
        position: BlockPos,
        face: Face,
        variant: usize,
    },
    CloseUI,
    HotbarSelect {
        slot: usize,
    },
    InteractBlock {
        position: BlockPos,
    },
    InteractEntity {
        entity: Uuid,
    },
    AttackEntity {
        entity: Uuid,
    },
    DropItem {
        stack: bool,
    },
    MoveItem {
        from: usize,
        to: usize,
        mode: ItemMoveMode,
    },
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
    GameTick {
        ticks_passed: u64,
        dt: f32,
        mspt: f32,
    },
    LoadChunk {
        position: ChunkPos,
        blocks: BlockPalette,
        components: ClientChunkBlockComponents,
    },
    UnloadChunk {
        position: ChunkPos,
    },
    SetBlock {
        position: BlockPos,
        block: BlockEntry,
    },
    UpdateBlockComponents {
        chunk: ChunkPos,
        offset: ChunkOffset,
        data: ClientBlockComponentUpdate,
    },
    AddEntity {
        uuid: Uuid,
        key: EntityKey,
        position: Pos,
        direction: LookDirection,
        hand_item: Option<ClientItem>,
    },
    MoveEntity {
        uuid: Uuid,
        position: Pos,
        direction: LookDirection,
    },
    RemoveEntity {
        uuid: Uuid,
    },
    EntityHandItem {
        uuid: Uuid,
        item: Option<ClientItem>,
    },
    SetPlayerEntity {
        uuid: Option<Uuid>,
    },
    TeleportPlayer {
        position: Pos,
        teleport_id: u32,
    },
    PlayerAbilities {
        abilities: PlayerAbilities,
    },
    UIOpen {
        screen: UIScreenKey,
        slots: Vec<Option<ClientItem>>,
    },
    UISetSlot {
        slot: usize,
        item: Option<ClientItem>,
    },
    UIClose,
    HUDSlot {
        slot: usize,
        item: Option<ClientItem>,
    },
}

pub fn make_connection_config() -> ConnectionConfig {
    ConnectionConfig {
        available_bytes_per_tick: 60000,
        server_channels_config: vec![
            ChannelConfig {
                channel_id: 0,
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::Unreliable,
            },
            ChannelConfig {
                channel_id: 1,
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::ReliableUnordered {
                    resend_time: Duration::from_millis(300),
                },
            },
            ChannelConfig {
                channel_id: 2,
                max_memory_usage_bytes: 10 * 1024 * 1024,
                send_type: SendType::ReliableOrdered {
                    resend_time: Duration::from_millis(300),
                },
            },
        ],
        client_channels_config: vec![
            ChannelConfig {
                channel_id: 0,
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::Unreliable,
            },
            ChannelConfig {
                channel_id: 1,
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::ReliableUnordered {
                    resend_time: Duration::from_millis(300),
                },
            },
            ChannelConfig {
                channel_id: 2,
                max_memory_usage_bytes: 5 * 1024 * 1024,
                send_type: SendType::ReliableOrdered {
                    resend_time: Duration::from_millis(300),
                },
            },
        ],
    }
}
