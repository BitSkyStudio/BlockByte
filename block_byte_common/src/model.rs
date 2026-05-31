use cgmath::{
    Deg, ElementWise, EuclideanSpace, Euler, InnerSpace, Matrix4, Point3, SquareMatrix, Transform,
    Vector2, Vector3, VectorSpace, Zero,
};
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    TexCoords,
    coord::{Face, FaceMap, Pos, Vec3},
    registry::TextureKey,
};

#[derive(Deserialize, Debug)]
struct BBModel {
    elements: Vec<BBElement>,
    outliner: Vec<BBOutliner>,
    groups: Vec<BBGroup>,
    animations: Option<Vec<BBAnimation>>,
    textures: Vec<BBTexture>,
}
#[derive(Deserialize, Debug)]
struct BBTexture {
    uv_width: f32,
    uv_height: f32,
    name: String,
    source: String,
}
fn default_true() -> bool {
    true
}
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "type")]
enum BBElement {
    #[serde(rename = "cube")]
    Cube {
        uuid: String,
        from: [f32; 3],
        to: [f32; 3],
        origin: [f32; 3],
        #[serde(default)]
        rotation: [f32; 3],
        faces: Option<HashMap<String, BBFace>>,
        #[serde(default = "default_true")]
        visibility: bool,
    },
    #[serde(rename = "locator")]
    Locator {
        uuid: String,
        position: [f32; 3],
        rotation: [f32; 3],
        name: String,
        #[serde(default = "default_true")]
        visibility: bool,
    },
    #[serde(rename = "mesh")]
    Mesh {
        uuid: String,
        origin: [f32; 3],
        #[serde(default)]
        rotation: [f32; 3],
        vertices: HashMap<String, [f32; 3]>,
        faces: HashMap<String, BBMeshFace>,
        #[serde(default = "default_true")]
        visibility: bool,
    },
}
#[derive(Deserialize, Clone, Debug)]
struct BBMeshFace {
    uv: HashMap<String, [f32; 2]>,
    vertices: Vec<String>,
    texture: usize,
}
impl BBElement {
    pub fn uuid(&self) -> &str {
        match self {
            BBElement::Cube { uuid, .. } => uuid.as_str(),
            BBElement::Locator { uuid, .. } => uuid.as_str(),
            BBElement::Mesh { uuid, .. } => uuid.as_str(),
        }
    }
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
enum BBOutliner {
    Group {
        uuid: String,
        children: Vec<BBOutliner>,
    },
    Element(String),
}

#[derive(Deserialize, Debug, Clone)]
struct BBGroup {
    uuid: String,
    name: String,
    origin: [f32; 3],
    children: Vec<String>,
}

#[derive(Deserialize, Clone, Debug)]
struct BBFace {
    uv: [f32; 4],
    texture: usize,
    #[serde(default)]
    rotation: u32,
}

#[derive(Deserialize, Debug)]
struct BBAnimation {
    name: String,
    length: f32,
    animators: HashMap<String, BBAnimator>,
}

#[derive(Deserialize, Debug)]
struct BBAnimator {
    #[serde(default)]
    keyframes: Vec<BBKeyframe>,
}

#[derive(Deserialize, Clone, Debug)]
struct BBKeyframe {
    channel: AnimatorChannel,
    time: f32,
    data_points: Vec<BBVec3>,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AnimatorChannel {
    #[serde(rename = "position")]
    Translation,
    #[serde(rename = "rotation")]
    Rotation,
    #[serde(rename = "scale")]
    Scale,
}

#[derive(Deserialize, Clone, Debug)]
struct BBVec3 {
    x: String,
    y: String,
    z: String,
}

impl BBVec3 {
    fn to_vec(&self) -> Vector3<f32> {
        Vector3::new(
            self.x.trim().parse().unwrap(),
            self.y.trim().parse().unwrap(),
            self.z.trim().parse().unwrap(),
        )
    }
}

pub struct Bone {
    pub origin: Vector3<f32>,
    pub elements: Vec<Element>,
    pub children: Vec<Bone>,
    pub animators: Vec<BoneAnimation>,
}
impl Bone {
    fn animation_transform(&self, animations: &[ResolvedAnimation]) -> Matrix4<f32> {
        let mut transform = Matrix4::identity();
        for animation in animations {
            let animator = &self.animators[animation.animation];
            if let Some(value) = animator.sample(AnimatorChannel::Translation, animation.time) {
                transform = transform
                    * Matrix4::from_translation(value / BLOCKBENCH_SIZE * animation.weight);
            }
            if let Some(value) = animator.sample(AnimatorChannel::Rotation, animation.time) {
                let o = Matrix4::from_translation(self.origin);
                let io = Matrix4::from_translation(-self.origin);
                transform = transform
                    * o
                    * Matrix4::from(Euler {
                        x: Deg(value.x * animation.weight),
                        y: Deg(value.y * animation.weight),
                        z: Deg(value.z * animation.weight),
                    })
                    * io;
            }
            if let Some(value) = animator.sample(AnimatorChannel::Scale, animation.time) {
                transform = transform
                    * Matrix4::from_nonuniform_scale(
                        1. + (value.x - 1.) * animation.weight,
                        1. + (value.y - 1.) * animation.weight,
                        1. + (value.z - 1.) * animation.weight,
                    );
            }
        }
        transform
    }
    fn draw(
        &self,
        parent: Matrix4<f32>,
        animations: &[ResolvedAnimation],
        mut geometry_consumer: &mut impl FnMut(ModelGeometry),
        mut binding_consumer: &mut impl FnMut(Matrix4<f32>, &str),
    ) {
        let world = parent * self.animation_transform(animations);

        for v in [world.x, world.y, world.z] {
            if v.truncate().magnitude2() <= 0.1 {
                return;
            }
        }

        for elem in &self.elements {
            match elem {
                Element::Cube {
                    from,
                    to,
                    uvs,
                    origin,
                    rotation,
                } => {
                    let o = Matrix4::from_translation(Vector3::from(origin.into_array()));
                    let io = Matrix4::from_translation(-Vector3::from(origin.into_array()));
                    let world = world * o * Matrix4::from(*rotation) * io;
                    for face in Face::all() {
                        let (uv, rotation, texture) = *uvs.by_face(face);
                        geometry_consumer(ModelGeometry::Quad(
                            face.get_vertices(uv, rotation).map(|(pos, uv)| {
                                let position = Pos {
                                    x: to.x * pos.x + from.x * (1. - pos.x),
                                    y: to.y * pos.y + from.y * (1. - pos.y),
                                    z: to.z * pos.z + from.z * (1. - pos.z),
                                };
                                let normal = face.get_offset();
                                ModelVertex {
                                    position: position.multiply_point(world),
                                    normal: normal.multiply_vector(world).normalize(),
                                    uv,
                                }
                            }),
                            texture,
                        ));
                    }
                }
                Element::Locator {
                    position,
                    rotation,
                    name,
                } => {
                    binding_consumer(
                        world
                            * Matrix4::from_translation(Vector3::from(position.into_array()))
                            * Matrix4::from(*rotation),
                        name.as_str(),
                    );
                }
                Element::Mesh {
                    origin,
                    rotation,
                    faces,
                } => {
                    let o = Matrix4::from_translation(Vector3::from(origin.into_array()));
                    let world = world * o * Matrix4::from(*rotation);
                    for face in faces {
                        geometry_consumer(ModelGeometry::Quad(
                            std::array::from_fn(|i| {
                                let (position, uv) = &face.vertices[i];
                                ModelVertex {
                                    position: position.multiply_point(world),
                                    normal: Pos::Y,
                                    uv: *uv,
                                }
                            }),
                            face.texture,
                        ));
                    }
                }
            }
        }
        for child in &self.children {
            child.draw(world, animations, geometry_consumer, binding_consumer);
        }
    }
    fn anchor(
        &self,
        anchor: &str,
        matrix: Matrix4<f32>,
        animations: &[ResolvedAnimation],
    ) -> Option<Matrix4<f32>> {
        let transform = matrix * self.animation_transform(animations);
        for element in &self.elements {
            match element {
                Element::Locator {
                    name,
                    position,
                    rotation,
                } => {
                    if name == anchor {
                        return Some(
                            transform
                                * Matrix4::from_translation(Vector3::from(position.into_array()))
                                * Matrix4::from(*rotation),
                        );
                    }
                }
                _ => {}
            }
        }
        for child in &self.children {
            if let Some(anchor) = child.anchor(anchor, transform, animations) {
                return Some(anchor);
            }
        }

        None
    }
}

pub enum Element {
    Cube {
        from: Pos,
        to: Pos,
        origin: Pos,
        rotation: Euler<Deg<f32>>,
        uvs: FaceMap<(TexCoords, u8, usize)>,
    },
    Locator {
        name: String,
        position: Pos,
        rotation: Euler<Deg<f32>>,
    },
    Mesh {
        origin: Pos,
        rotation: Euler<Deg<f32>>,
        faces: Vec<MeshFace>,
    },
}
pub struct MeshFace {
    vertices: Vec<(Pos, [f32; 2])>,
    texture: usize,
}
pub struct DrawAnimation<'a> {
    pub animation: &'a str,
    pub time: f32,
    pub weight: f32,
}
struct ResolvedAnimation {
    animation: usize,
    time: f32,
    weight: f32,
}
pub struct Model {
    root_bone: Bone,
    animations: HashMap<String, usize>,
    pub textures: Vec<(ModelTexture, f32, f32)>,
}
pub enum ModelTexture {
    Embed(String, usize),
    Variable(usize),
    Texture(TextureKey),
}
pub struct ModelVertex {
    pub position: Pos,
    pub normal: Pos,
    pub uv: [f32; 2],
}
pub enum ModelGeometry {
    Quad([ModelVertex; 4], usize),
    Triangle([ModelVertex; 3], usize),
}
impl Model {
    pub fn draw(
        &self,
        matrix: Matrix4<f32>,
        animations: &[DrawAnimation],
        mut geometry_consumer: impl FnMut(ModelGeometry),
        mut binding_consumer: impl FnMut(Matrix4<f32>, &str),
    ) {
        let animations = animations
            .iter()
            .map(|animation| ResolvedAnimation {
                animation: *self.animations.get(animation.animation).unwrap(),
                time: animation.time,
                weight: animation.weight,
            })
            .collect::<Vec<_>>();
        self.root_bone.draw(
            matrix,
            &animations[..],
            &mut geometry_consumer,
            &mut binding_consumer,
        );
    }
    pub fn anchor(
        &self,
        name: &str,
        matrix: Matrix4<f32>,
        animations: &[DrawAnimation],
    ) -> Option<Matrix4<f32>> {
        let animations = animations
            .iter()
            .map(|animation| ResolvedAnimation {
                animation: *self.animations.get(animation.animation).unwrap(),
                time: animation.time,
                weight: animation.weight,
            })
            .collect::<Vec<_>>();
        self.root_bone.anchor(name, matrix, &animations[..])
    }
    pub fn from_bbmodel(bbmodel: BBModel) -> Model {
        let element_map: HashMap<_, _> = bbmodel
            .elements
            .iter()
            .map(|e| (e.uuid().to_string(), e.clone()))
            .collect();

        let group_map: HashMap<_, _> = bbmodel
            .groups
            .iter()
            .map(|g| (g.uuid.clone(), g.clone()))
            .collect();

        let mut root_bone =
            Self::build_bone(&bbmodel.outliner, None, &element_map, &group_map, &bbmodel);
        let mut embed_texture_id = 0;
        Model {
            root_bone,
            animations: bbmodel
                .animations
                .unwrap_or(Vec::new())
                .into_iter()
                .enumerate()
                .map(|(i, animation)| (animation.name, i))
                .collect(),
            textures: bbmodel
                .textures
                .into_iter()
                .map(|texture| {
                    let model_texture = if texture.name.starts_with("$") {
                        ModelTexture::Variable(texture.name[1..].parse().unwrap())
                    } else if texture.name.starts_with("@") {
                        ModelTexture::Texture(TextureKey::id(&texture.name[1..]).unwrap())
                    } else {
                        embed_texture_id += 1;
                        ModelTexture::Embed(texture.source, embed_texture_id - 1)
                    };
                    (
                        model_texture,
                        texture.uv_width as f32,
                        texture.uv_height as f32,
                    )
                })
                .collect(),
        }
    }
    fn build_bone(
        outliner: &[BBOutliner],
        uuid: Option<&str>,
        element_map: &HashMap<String, BBElement>,
        group_map: &HashMap<String, BBGroup>,
        model: &BBModel,
    ) -> Bone {
        let group = uuid.and_then(|uuid| group_map.get(uuid));
        let name = group.map(|group| group.name.as_str()).unwrap_or("");
        let mut bone = Bone {
            origin: group
                .map(|group| Vector3::from(group.origin) / BLOCKBENCH_SIZE)
                .unwrap_or(Vector3::zero()),
            elements: Vec::new(),
            children: Vec::new(),
            animators: model
                .animations
                .as_ref()
                .unwrap_or(&Vec::new())
                .iter()
                .map(|animation| {
                    let mut bone_animation = BoneAnimation::default();
                    if let Some(uuid) = uuid {
                        if let Some(animator) = animation.animators.get(uuid) {
                            for bb_keyframe in &animator.keyframes {
                                let keyframe = Keyframe {
                                    data: bb_keyframe.data_points[0].to_vec(),
                                    time: bb_keyframe.time,
                                };
                                match bb_keyframe.channel {
                                    AnimatorChannel::Translation => {
                                        bone_animation.position.push(keyframe)
                                    }
                                    AnimatorChannel::Rotation => {
                                        bone_animation.rotation.push(keyframe)
                                    }
                                    AnimatorChannel::Scale => bone_animation.scale.push(keyframe),
                                }
                            }
                        }
                    }
                    for channel in [
                        &mut bone_animation.position,
                        &mut bone_animation.rotation,
                        &mut bone_animation.scale,
                    ] {
                        channel.sort_by(|a, b| a.time.total_cmp(&b.time));
                    }
                    bone_animation
                })
                .collect(),
        };
        for outline in outliner {
            match outline {
                BBOutliner::Group { uuid, children } => {
                    bone.children.push(Self::build_bone(
                        &children,
                        Some(uuid),
                        element_map,
                        group_map,
                        model,
                    ));
                }
                BBOutliner::Element(element) => {
                    let bbelement = element_map.get(element.as_str()).unwrap();
                    match bbelement {
                        BBElement::Cube { visibility, .. }
                        | BBElement::Locator { visibility, .. }
                        | BBElement::Mesh { visibility, .. } => {
                            if !*visibility {
                                continue;
                            }
                        }
                    }
                    match bbelement {
                        BBElement::Cube {
                            uuid,
                            from,
                            to,
                            faces,
                            origin,
                            rotation,
                            visibility: _,
                        } => {
                            bone.elements.push(Element::Cube {
                                from: Pos::from_array(*from) / BLOCKBENCH_SIZE,
                                to: Pos::from_array(*to) / BLOCKBENCH_SIZE,
                                origin: Pos::from_array(*origin) / BLOCKBENCH_SIZE,
                                rotation: Euler {
                                    x: Deg(rotation[0]),
                                    y: Deg(rotation[1]),
                                    z: Deg(rotation[2]),
                                },
                                uvs: FaceMap::init(|face| {
                                    let face = match face {
                                        Face::Back => "south",
                                        Face::Front => "north",
                                        Face::Up => "up",
                                        Face::Down => "down",
                                        Face::Left => "west",
                                        Face::Right => "east",
                                    };
                                    let face = faces.as_ref().unwrap().get(face).unwrap();
                                    let texture = &model.textures[face.texture];
                                    (
                                        TexCoords {
                                            u1: face.uv[0] / texture.uv_width,
                                            v1: face.uv[1] / texture.uv_height,
                                            u2: face.uv[2] / texture.uv_width,
                                            v2: face.uv[3] / texture.uv_height,
                                        },
                                        (face.rotation / 90) as u8 % 4,
                                        face.texture,
                                    )
                                }),
                            });
                        }
                        BBElement::Locator {
                            uuid,
                            position,
                            rotation,
                            name,
                            visibility: _,
                        } => {
                            bone.elements.push(Element::Locator {
                                position: Pos::from_array(*position) / BLOCKBENCH_SIZE,
                                rotation: Euler {
                                    x: Deg(rotation[0]),
                                    y: Deg(rotation[1]),
                                    z: Deg(rotation[2]),
                                },
                                name: name.clone(),
                            });
                        }
                        BBElement::Mesh {
                            uuid,
                            origin,
                            rotation,
                            vertices,
                            faces,
                            visibility: _,
                        } => {
                            bone.elements.push(Element::Mesh {
                                origin: Pos::from_array(*origin) / BLOCKBENCH_SIZE,
                                rotation: Euler {
                                    x: Deg(rotation[0]),
                                    y: Deg(rotation[1]),
                                    z: Deg(rotation[2]),
                                },
                                faces: faces
                                    .values()
                                    .map(|face| {
                                        let texture = &model.textures[face.texture];
                                        let vertices = face
                                            .vertices
                                            .iter()
                                            .map(|vertex| {
                                                let uv = *face.uv.get(vertex.as_str()).unwrap();
                                                (
                                                    Pos::from_array(
                                                        *vertices.get(vertex.as_str()).unwrap(),
                                                    ) / BLOCKBENCH_SIZE,
                                                    [
                                                        uv[0] / texture.uv_width,
                                                        uv[1] / texture.uv_height,
                                                    ],
                                                )
                                            })
                                            .collect::<Vec<_>>();
                                        assert_eq!(vertices.len(), 4);
                                        MeshFace {
                                            vertices,
                                            texture: face.texture,
                                        }
                                    })
                                    .collect(),
                            });
                        }
                    }
                }
            }
        }
        bone
    }
}
impl<'de> Deserialize<'de> for Model {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let bbmodel = BBModel::deserialize(deserializer)?;
        Ok(Model::from_bbmodel(bbmodel))
    }
}
#[derive(Debug)]
pub struct Keyframe {
    time: f32,
    data: Vector3<f32>,
}
#[derive(Default)]
pub struct BoneAnimation {
    position: Vec<Keyframe>,
    rotation: Vec<Keyframe>,
    scale: Vec<Keyframe>,
}
impl BoneAnimation {
    pub fn sample(&self, channel: AnimatorChannel, time: f32) -> Option<Vector3<f32>> {
        let frames = match channel {
            AnimatorChannel::Translation => &self.position,
            AnimatorChannel::Rotation => &self.rotation,
            AnimatorChannel::Scale => &self.scale,
        };
        if frames.is_empty() {
            return None;
        }
        if time <= frames[0].time {
            return Some(frames[0].data);
        }
        for w in frames.windows(2) {
            let a = &w[0];
            let b = &w[1];
            if time >= a.time && time <= b.time {
                let t = (time - a.time) / (b.time - a.time);
                return Some(a.data.lerp(b.data, t));
            }
        }
        Some(frames.last().unwrap().data)
    }
}

pub const BLOCKBENCH_SIZE: f32 = 16.;
