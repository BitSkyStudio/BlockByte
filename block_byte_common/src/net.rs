use std::{collections::HashSet, time::Duration};

use palettevec::PaletteVec;
use renet::{ChannelConfig, ConnectionConfig, SendType};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ClientItem, ItemMoveMode, LookDirection, PlayerAbilities,
    coord::{BlockPos, ChunkOffset, ChunkPos, Face, Pos},
    registry::{BlockEntry, BlockKey, BlockPalette, EntityKey, RecipeKey, ResearchKey, ToolData},
    scripts::ScriptValue,
    ui::{PropertyMap, UIScreenKey},
    world::{ClientBlockComponentUpdate, ClientChunkBlockComponents},
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition {
        position: Pos,
        direction: LookDirection,
        teleport_id: u32,
        crouching: bool,
    },
    AttackBlock {
        position: BlockPos,
        face: Face,
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
    Research {
        research: ResearchKey,
    },
    Craft {
        recipe: RecipeKey,
        count: u32,
    },
    OpenPlayerInventory,
    HarvestPlant {
        position: BlockPos,
        index: usize,
    },
    UIButtonPress {
        property: String,
        value: ScriptValue,
    },
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
    GameTick {
        ticks_passed: u64,
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
        crouching: bool,
    },
    MoveEntity {
        uuid: Uuid,
        position: Pos,
        direction: LookDirection,
        crouching: bool,
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
    HudBarUpdate {
        health: f32,
    },
    Knockback {
        velocity: Pos,
    },
    UpdateResearch {
        research: HashSet<ResearchKey>,
    },
}
impl NetworkMessageS2C {
    pub fn is_block_related(&self) -> bool {
        match self {
            NetworkMessageS2C::LoadChunk { .. }
            | NetworkMessageS2C::UnloadChunk { .. }
            | NetworkMessageS2C::SetBlock { .. }
            | NetworkMessageS2C::UpdateBlockComponents { .. } => true,
            _ => false,
        }
    }
}

pub fn make_connection_config() -> ConnectionConfig {
    ConnectionConfig {
        available_bytes_per_tick: 30000,
        server_channels_config: vec![
            ChannelConfig {
                channel_id: 0,
                max_memory_usage_bytes: 1 * 1024 * 1024,
                send_type: SendType::ReliableOrdered {
                    resend_time: Duration::from_millis(300),
                },
            },
            ChannelConfig {
                channel_id: 1,
                max_memory_usage_bytes: 10 * 1024 * 1024,
                send_type: SendType::ReliableOrdered {
                    resend_time: Duration::from_millis(300),
                },
            },
        ],
        client_channels_config: vec![ChannelConfig {
            channel_id: 0,
            max_memory_usage_bytes: 1 * 1024 * 1024,
            send_type: SendType::ReliableOrdered {
                resend_time: Duration::from_millis(300),
            },
        }],
    }
}
