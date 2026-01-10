use palettevec::PaletteVec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ClientItem, PlayerAbilities,
    coord::{BlockPos, ChunkOffset, ChunkPos, Face, Pos},
    registry::{BlockKey, BlockPalette, EntityKey},
    ui::{PropertyMap, UIScreenKey},
    world::{ClientBlockComponentUpdate, ClientChunkBlockComponents},
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition { position: Pos, teleport_id: u32 },
    AttackBlock { position: BlockPos },
    PlaceBlock { position: BlockPos, face: Face },
    CloseUI,
    HotbarSelect { slot: isize, relative: bool },
    InteractBlock { position: BlockPos },
    InteractEntity { entity: Uuid },
}
#[derive(Serialize, Deserialize)]
pub enum NetworkMessageS2C {
    GameTick {
        ticks_passed: u64,
        dt: f32,
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
        block: BlockKey,
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
    },
    MoveEntity {
        uuid: Uuid,
        position: Pos,
    },
    RemoveEntity {
        uuid: Uuid,
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
    HUDUpdate {
        items: Vec<Option<ClientItem>>,
        properties: PropertyMap,
    },
}
