use cgmath::{
    Deg, EuclideanSpace, Euler, Matrix4, Point3, SquareMatrix, Transform, Vector2, Vector3,
    VectorSpace,
};
use serde::Deserialize;
use std::collections::HashMap;

/* =========================
Blockbench JSON structs
========================= */

#[derive(Deserialize, Debug)]
pub struct BBModel {
    pub elements: Vec<BBElement>,
    pub outliner: Vec<BBOutliner>,
    pub groups: Vec<BBGroup>,
    pub animations: Option<Vec<BBAnimation>>,
    pub textures: Vec<BBTexture>,
}
#[derive(Deserialize, Debug)]
pub struct BBTexture {
    pub uv_width: f32,
    pub uv_height: f32,
    pub source: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BBElement {
    pub uuid: String,
    pub from: [f32; 3],
    pub to: [f32; 3],
    pub faces: Option<HashMap<String, BBFace>>,
}

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum BBOutliner {
    Group {
        uuid: String,
        children: Vec<BBOutliner>,
    },
    Element(String), // element UUID
}

#[derive(Deserialize, Debug, Clone)]
pub struct BBGroup {
    pub uuid: String,
    pub name: String,
    pub origin: [f32; 3],
    pub children: Vec<String>, // UUIDs of elements
}

#[derive(Deserialize, Clone, Debug)]
pub struct BBFace {
    pub uv: [f32; 4],
    pub texture: usize,
}

/* =========================
Animations
========================= */

#[derive(Deserialize, Debug)]
pub struct BBAnimation {
    pub name: String,
    pub length: f32,
    pub animators: HashMap<String, BBAnimator>, // UUID keys
}

#[derive(Deserialize, Debug)]
pub struct BBAnimator {
    pub name: String, // UUID of bone/group
    #[serde(rename = "type")]
    pub animator_type: String,
    pub keyframes: Option<Vec<BBKeyframe>>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BBKeyframe {
    pub channel: String, // "rotation", "position", "scale"
    pub time: f32,
    pub data_points: Vec<BBVec3>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct BBVec3 {
    pub x: String,
    pub y: String,
    pub z: String,
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

/* =========================
Runtime structures
========================= */

#[derive(Debug, Clone)]
pub struct Vertex {
    pub position: Vector3<f32>,
    pub uv: Vector2<f32>,
}

#[derive(Debug, Clone)]
pub struct Triangle {
    pub a: Vertex,
    pub b: Vertex,
    pub c: Vertex,
    pub texture: usize,
}

#[derive(Debug)]
pub struct Bone {
    pub uuid: String,
    pub origin: Vector3<f32>,
    pub elements: Vec<BBElement>,
    pub children: Vec<Bone>,
}

/* =========================
Bone hierarchy builder (UUID-based)
========================= */

fn build_bones(model: &BBModel) -> HashMap<String, Bone> {
    // Map elements and groups by UUID
    let element_map: HashMap<_, _> = model
        .elements
        .iter()
        .map(|e| (e.uuid.clone(), e.clone()))
        .collect();

    let group_map: HashMap<_, _> = model
        .groups
        .iter()
        .map(|g| (g.uuid.clone(), g.clone()))
        .collect();

    // Build top-level bones recursively
    let mut bones = HashMap::new();
    for node in &model.outliner {
        let bone = build_bone(node, &element_map, &group_map);
        bones.insert(bone.uuid.clone(), bone);
    }

    bones
}

fn build_bone(
    node: &BBOutliner,
    elements: &HashMap<String, BBElement>,
    groups: &HashMap<String, BBGroup>,
) -> Bone {
    match node {
        BBOutliner::Element(uuid) => {
            let elem = elements.get(uuid).expect("Element UUID not found");
            Bone {
                uuid: elem.uuid.clone(),
                origin: Vector3::new(0.0, 0.0, 0.0),
                elements: vec![elem.clone()],
                children: vec![],
            }
        }
        BBOutliner::Group { uuid, children } => {
            let group = groups.get(uuid).expect("Group UUID not found");
            let bone_origin = Vector3::from(group.origin);

            // Build child bones recursively
            let mut bone_children = Vec::new();
            for child in children {
                bone_children.push(build_bone(child, elements, groups));
            }

            // Attach direct child elements
            let mut elems = Vec::new();
            for elem_uuid in &group.children {
                if let Some(elem) = elements.get(elem_uuid) {
                    elems.push(elem.clone());
                }
            }

            Bone {
                uuid: group.uuid.clone(),
                origin: bone_origin,
                elements: elems,
                children: bone_children,
            }
        }
    }
}

/* =========================
Animation helpers
========================= */

fn find_animator<'a>(anim: &'a BBAnimation, bone_uuid: &str) -> Option<&'a BBAnimator> {
    anim.animators.get(bone_uuid)
}

fn sample_keyframes(frames: &[BBKeyframe], time: f32) -> Vector3<f32> {
    if frames.len() == 1 {
        return frames[0].data_points[0].to_vec();
    }

    for w in frames.windows(2) {
        let a = &w[0];
        let b = &w[1];
        if time >= a.time && time <= b.time {
            let t = (time - a.time) / (b.time - a.time);
            return a.data_points[0].to_vec().lerp(b.data_points[0].to_vec(), t);
        }
    }

    frames.last().unwrap().data_points[0].to_vec()
}

/* =========================
Geometry helpers
========================= */

const CUBE_VERTS: [Vector3<f32>; 8] = [
    Vector3::new(0.0, 0.0, 0.0),
    Vector3::new(1.0, 0.0, 0.0),
    Vector3::new(1.0, 1.0, 0.0),
    Vector3::new(0.0, 1.0, 0.0),
    Vector3::new(0.0, 0.0, 1.0),
    Vector3::new(1.0, 0.0, 1.0),
    Vector3::new(1.0, 1.0, 1.0),
    Vector3::new(0.0, 1.0, 1.0),
];

fn face_indices(face: &str) -> [usize; 4] {
    match face {
        "north" => [0, 1, 2, 3],
        "south" => [5, 4, 7, 6],
        "west" => [4, 0, 3, 7],
        "east" => [1, 5, 6, 2],
        "up" => [3, 2, 6, 7],
        "down" => [4, 5, 1, 0],
        _ => panic!("Unknown face {}", face),
    }
}

fn face_uvs(uv: [f32; 4]) -> [Vector2<f32>; 4] {
    let (x1, y1, x2, y2) = (uv[0], uv[1], uv[2], uv[3]);
    [
        Vector2::new(x1, y1),
        Vector2::new(x2, y1),
        Vector2::new(x2, y2),
        Vector2::new(x1, y2),
    ]
}

/* =========================
Triangle collector (UUID-based)
========================= */

fn collect_triangles(
    bone: &Bone,
    parent: Matrix4<f32>,
    anim: Option<&BBAnimation>,
    time: f32,
    out: &mut Vec<Triangle>,
) {
    // Get rotation from animator (UUID)
    let mut rot = Matrix4::identity();
    let mut transl = Matrix4::identity();
    if let Some(anim) = anim {
        if let Some(animator) = anim.animators.get(&bone.uuid) {
            if let Some(frames) = &animator.keyframes {
                let rotation_frames: Vec<_> = frames
                    .iter()
                    .filter(|k| k.channel == "rotation")
                    .cloned()
                    .collect();
                if !rotation_frames.is_empty() {
                    let r = sample_keyframes(&rotation_frames, time);
                    rot = Matrix4::from(Euler {
                        x: Deg(r.x),
                        y: Deg(r.y),
                        z: Deg(r.z),
                    });
                }
                let translation_frames: Vec<_> = frames
                    .iter()
                    .filter(|k| k.channel == "position")
                    .cloned()
                    .collect();
                if !translation_frames.is_empty() {
                    let r = sample_keyframes(&translation_frames, time);
                    rot = Matrix4::from_translation(r);
                }
            }
        }
    }

    // Apply rotation around origin pivot
    let o = Matrix4::from_translation(bone.origin);
    let io = Matrix4::from_translation(-bone.origin);
    let world = parent * transl * o * rot * io;

    // Transform elements
    for elem in &bone.elements {
        let min = Vector3::from(elem.from);
        let max = Vector3::from(elem.to);
        let size = max - min;

        let verts: Vec<Vector3<f32>> = CUBE_VERTS
            .iter()
            .map(|v| min + Vector3::new(v.x * size.x, v.y * size.y, v.z * size.z))
            .collect();

        if let Some(faces) = &elem.faces {
            for (face_name, face) in faces {
                let idx = face_indices(face_name);
                let uvs = face_uvs(face.uv);

                let v = |i: usize, uv: Vector2<f32>| Vertex {
                    position: world.transform_point(Point3::from_vec(verts[i])).to_vec(),
                    uv,
                };

                out.push(Triangle {
                    a: v(idx[0], uvs[0]),
                    b: v(idx[1], uvs[1]),
                    c: v(idx[2], uvs[2]),
                    texture: face.texture,
                });
                out.push(Triangle {
                    a: v(idx[2], uvs[2]),
                    b: v(idx[3], uvs[3]),
                    c: v(idx[0], uvs[0]),
                    texture: face.texture,
                });
            }
        }
    }

    // Recurse into children
    for child in &bone.children {
        collect_triangles(child, world, anim, time, out);
    }
}

/* =========================
Public draw entry point
========================= */

pub fn draw(model: &BBModel, animation_name: Option<&str>, time: f32) -> Vec<Triangle> {
    let bones_map = build_bones(model);

    let anim =
        animation_name.and_then(|name| model.animations.as_ref()?.iter().find(|a| a.name == name));

    let mut triangles = Vec::new();
    for bone in bones_map.values() {
        collect_triangles(bone, Matrix4::identity(), anim, time, &mut triangles);
    }
    triangles
}
