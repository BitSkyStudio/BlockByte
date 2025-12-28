use palettevec::PaletteVec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    coord::{BlockPos, ChunkOffset, ChunkPos, Face, Pos},
    registry::{BlockKey, BlockPalette, EntityKey},
    world::{ClientBlockComponentUpdate, ClientChunkBlockComponents},
};

#[derive(Serialize, Deserialize)]
pub enum NetworkMessageC2S {
    PlayerPosition { position: Pos },
    AttackBlock { position: BlockPos },
    InteractBlock { position: BlockPos, face: Face },
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
}
