use cgmath::{
    Deg, ElementWise, EuclideanSpace, Euler, Matrix4, Point3, SquareMatrix, Transform, Vector2,
    Vector3, VectorSpace, Zero,
};
use serde::Deserialize;
use std::collections::HashMap;
use uuid::Uuid;

use crate::{
    TexCoords,
    coord::{Face, FaceMap, Pos},
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
    source: String,
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
    },
    #[serde(rename = "locator")]
    Locator {
        uuid: String,
        position: [f32; 3],
        rotation: [f32; 3],
        name: String,
    },
    #[serde(rename = "mesh")]
    Mesh {
        uuid: String,
        origin: [f32; 3],
        #[serde(default)]
        rotation: [f32; 3],
        vertices: HashMap<String, [f32; 3]>,
        faces: HashMap<String, BBMeshFace>,
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
            self.x.parse().unwrap(),
            self.y.parse().unwrap(),
            self.z.parse().unwrap(),
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
    fn animation_transform(&self, animation: Option<usize>, time: f32) -> Matrix4<f32> {
        let mut transform = Matrix4::identity();
        if let Some(animation) = animation {
            let animator = &self.animators[animation];
            if let Some(value) = animator.sample(AnimatorChannel::Translation, time) {
                transform = transform * Matrix4::from_translation(value / BLOCKBENCH_SIZE);
            }
            if let Some(value) = animator.sample(AnimatorChannel::Rotation, time) {
                let o = Matrix4::from_translation(self.origin);
                let io = Matrix4::from_translation(-self.origin);
                transform = transform
                    * o
                    * Matrix4::from(Euler {
                        x: Deg(value.x),
                        y: Deg(value.y),
                        z: Deg(value.z),
                    })
                    * io;
            }
            if let Some(value) = animator.sample(AnimatorChannel::Scale, time) {
                transform = transform * Matrix4::from_nonuniform_scale(value.x, value.y, value.z);
            }
        }
        transform
    }
    fn draw(
        &self,
        parent: Matrix4<f32>,
        animation: Option<usize>,
        time: f32,
        mut vertex_consumer: &mut impl FnMut(Pos, Pos, (f32, f32), usize),
        mut binding_consumer: &mut impl FnMut(Matrix4<f32>, &str),
    ) {
        let world = parent * self.animation_transform(animation, time);

        for elem in &self.elements {
            match elem {
                Element::Cube {
                    from,
                    to,
                    uvs,
                    origin,
                    rotation,
                } => {
                    let o = Matrix4::from_translation(*origin);
                    let io = Matrix4::from_translation(-*origin);
                    let world = world * o * Matrix4::from(*rotation) * io;
                    for face in Face::all() {
                        let (uv, texture) = *uvs.by_face(*face);
                        face.add_vertices(uv, |pos, uv| {
                            let pos = Vector3::new(pos.x, pos.y, pos.z);
                            let pos = world
                                .transform_point(Point3::from_vec(
                                    *from + ((*to - *from).mul_element_wise(pos)),
                                ))
                                .to_vec();
                            let normal = face.get_offset();
                            let normal =
                                world.transform_vector(Vector3::new(normal.x, normal.y, normal.z));
                            vertex_consumer(
                                Pos {
                                    x: pos.x,
                                    y: pos.y,
                                    z: pos.z,
                                },
                                Pos {
                                    x: normal.x,
                                    y: normal.y,
                                    z: normal.z,
                                },
                                uv,
                                texture,
                            );
                        });
                    }
                }
                Element::Locator {
                    position,
                    rotation,
                    name,
                } => {
                    binding_consumer(
                        world * Matrix4::from_translation(*position) * Matrix4::from(*rotation),
                        name.as_str(),
                    );
                }
                Element::Mesh {
                    origin,
                    rotation,
                    faces,
                } => {
                    let o = Matrix4::from_translation(*origin);
                    let world = world * o * Matrix4::from(*rotation);
                    for face in faces {
                        for (vertex_pos, uv) in &face.vertices {
                            let point = Point3::from_vec(*vertex_pos);
                            let point = world.transform_point(point);
                            vertex_consumer(
                                Pos {
                                    x: point.x,
                                    y: point.y,
                                    z: point.z,
                                },
                                Pos::ZERO,
                                *uv,
                                face.texture,
                            );
                        }
                    }
                }
            }
        }
        for child in &self.children {
            child.draw(world, animation, time, vertex_consumer, binding_consumer);
        }
    }
    fn anchor(
        &self,
        anchor: &str,
        matrix: Matrix4<f32>,
        animation: Option<usize>,
        time: f32,
    ) -> Option<Matrix4<f32>> {
        let transform = matrix * self.animation_transform(animation, time);
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
                                * Matrix4::from_translation(*position)
                                * Matrix4::from(*rotation),
                        );
                    }
                }
                _ => {}
            }
        }
        for child in &self.children {
            if let Some(anchor) = child.anchor(anchor, transform, animation, time) {
                return Some(anchor);
            }
        }

        None
    }
}

pub enum Element {
    Cube {
        from: Vector3<f32>,
        to: Vector3<f32>,
        origin: Vector3<f32>,
        rotation: Euler<Deg<f32>>,
        uvs: FaceMap<(TexCoords, usize)>,
    },
    Locator {
        name: String,
        position: Vector3<f32>,
        rotation: Euler<Deg<f32>>,
    },
    Mesh {
        origin: Vector3<f32>,
        rotation: Euler<Deg<f32>>,
        faces: Vec<MeshFace>,
    },
}
pub struct MeshFace {
    vertices: Vec<(Vector3<f32>, (f32, f32))>,
    texture: usize,
}

pub struct Model {
    root_bone: Bone,
    animations: HashMap<String, usize>,
    pub textures: Vec<String>,
}
impl Model {
    pub fn draw(
        &self,
        matrix: Matrix4<f32>,
        animation_name: Option<&str>,
        time: f32,
        mut vertex_consumer: impl FnMut(Pos, Pos, (f32, f32), usize),
        mut binding_consumer: impl FnMut(Matrix4<f32>, &str),
    ) {
        let animation = animation_name.map(|animation| *self.animations.get(animation).unwrap());
        self.root_bone.draw(
            matrix,
            animation,
            time,
            &mut vertex_consumer,
            &mut binding_consumer,
        );
    }
    pub fn anchor(
        &self,
        name: &str,
        matrix: Matrix4<f32>,
        animation_name: Option<&str>,
        time: f32,
    ) -> Option<Matrix4<f32>> {
        let animation = animation_name.map(|animation| *self.animations.get(animation).unwrap());
        self.root_bone.anchor(name, matrix, animation, time)
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
                .map(|texture| texture.source)
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
                        BBElement::Cube {
                            uuid,
                            from,
                            to,
                            faces,
                            origin,
                            rotation,
                        } => {
                            bone.elements.push(Element::Cube {
                                from: Vector3::from(*from) / BLOCKBENCH_SIZE,
                                to: Vector3::from(*to) / BLOCKBENCH_SIZE,
                                origin: Vector3::from(*origin) / BLOCKBENCH_SIZE,
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
                        } => {
                            bone.elements.push(Element::Locator {
                                position: Vector3::from(*position) / BLOCKBENCH_SIZE,
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
                        } => {
                            bone.elements.push(Element::Mesh {
                                origin: Vector3::from(*origin) / BLOCKBENCH_SIZE,
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
                                                    Vector3::<f32>::from(
                                                        *vertices.get(vertex.as_str()).unwrap(),
                                                    ) / BLOCKBENCH_SIZE,
                                                    (
                                                        uv[0] / texture.uv_width,
                                                        uv[1] / texture.uv_height,
                                                    ),
                                                )
                                            })
                                            .collect::<Vec<_>>();
                                        MeshFace {
                                            vertices: [
                                                0, 1, 2, 0, 2, 3, /* back*/ 2, 1, 0, 3, 2, 0,
                                            ]
                                            .into_iter()
                                            .map(|i| vertices[i])
                                            .collect(),
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
