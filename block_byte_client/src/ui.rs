use std::{cell::RefCell, sync::OnceLock};

use block_byte_common::{
    ClientItem, Color, TexCoords,
    coord::Pos,
    registry::{BlockRenderData, ItemModel, Key, TextureKey},
    ui::{PropertyMap, StyleLength, UIElement, UIElementType, UIScreen, UIScreenKey, UIStyleRule},
};
use cgmath::{Matrix4, SquareMatrix, Transform, Vector3};
use taffy::{
    AlignItems, AvailableSpace, Dimension, FlexDirection, JustifyContent, Layout, LengthPercentage,
    LengthPercentageAuto, NodeId, Rect, Style, TaffyTree, prelude::TaffyZero,
};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::MouseButton,
};

use crate::{
    ClientGame, ClientPlayer, GUIMesh, TexCoordsExt, TexCoordsIndexExt,
    render::{CameraUniform, GUIVertex, item_model_icon_view},
    translate,
};

pub struct ScreenData {
    pub screen: UIScreenKey,
    pub slots: Vec<Option<ClientItem>>,
    pub properties: PropertyMap,
}

pub fn render_screen(
    screen_data: &ScreenData,
    size: PhysicalSize<u32>,
    game: &ClientGame,
    mesh: &mut GUIMesh,
    enable_hovering: bool,
) -> Option<usize> {
    let screen = screen_data.screen.data();
    let mut taffy: TaffyTree<&UIElement> = TaffyTree::new();
    let root = add_element_to_taffy(&screen.root, &mut taffy, &screen_data.properties);
    let body = taffy
        .new_with_children(
            Style {
                size: taffy::Size {
                    width: Dimension::percent(1.),
                    height: Dimension::percent(1.),
                },
                ..Style::DEFAULT
            },
            &[root],
        )
        .unwrap();

    taffy
        .compute_layout_with_measure(
            body,
            taffy::Size {
                width: AvailableSpace::Definite(size.width as f32),
                height: AvailableSpace::Definite(size.height as f32),
            },
            |_known_dimensions, _available_space, _node_id, node_context, _style| {
                measure_element(node_context.unwrap(), &screen_data.properties)
            },
        )
        .unwrap();
    render_element(root, &taffy, size, Pos::ZERO, &screen_data, game, mesh);
    if let Some(hovering) = resolve_hovering(
        root,
        Pos::ZERO,
        &taffy,
        Pos {
            x: game.cursor_position.x as f32,
            y: game.cursor_position.y as f32,
            z: 0.,
        },
    ) && enable_hovering
    {
        match &hovering.element_type {
            UIElementType::ItemSlot { slot } => {
                if let Some(item) = screen_data.slots.get(*slot).cloned().flatten() {
                    let aspect_ratio = size.width as f32 / size.height as f32;
                    let mut shift = 0.;
                    shift += text_renderer()
                        .draw(
                            UIPos {
                                x: game.cursor_position.x as f32 / size.height as f32 * 2.
                                    - aspect_ratio,
                                y: -(game.cursor_position.y as f32 / size.height as f32 * 2. - 1.),
                            },
                            translate(format!("item.{}", item.item.text_id()).as_str()),
                            40. / size.height as f32 * 2.,
                            Color::WHITE,
                            mesh,
                        )
                        .y;
                    for line in item.description.lines() {
                        shift += text_renderer()
                            .draw(
                                UIPos {
                                    x: game.cursor_position.x as f32 / size.height as f32 * 2.
                                        - aspect_ratio,
                                    y: -((game.cursor_position.y) as f32 / size.height as f32 * 2.
                                        - 1.
                                        + shift),
                                },
                                line,
                                40. / size.height as f32 * 2.,
                                Color::WHITE,
                                mesh,
                            )
                            .y;
                    }
                }
                return Some(*slot);
            }
            _ => {}
        }
    }
    None
}
pub fn measure_element(element: &UIElement, properties: &PropertyMap) -> taffy::Size<f32> {
    let style = get_element_style(element, properties);
    match &element.element_type {
        UIElementType::Box(uielements) => taffy::Size::ZERO,
        UIElementType::Label(text) => {
            let size = text_renderer().get_size(&text, style.font_size);
            taffy::Size {
                width: size.x,
                height: size.y,
            }
        }
        UIElementType::Image(_, width, height) => taffy::Size {
            width: *width,
            height: *height,
        },
        UIElementType::ItemSlot { .. } => taffy::Size {
            width: 50.,
            height: 50.,
        },
        UIElementType::CraftArea { .. } | UIElementType::ResearchTree { .. } => taffy::Size {
            width: 100.,
            height: 100.,
        },
    }
}
fn resolve_hovering<'a>(
    node: NodeId,
    parent_offset: Pos,
    taffy: &TaffyTree<&'a UIElement>,
    mouse: Pos,
) -> Option<&'a UIElement> {
    let layout = taffy.layout(node).unwrap();
    for child in taffy.children(node).unwrap() {
        if let Some(element) = resolve_hovering(
            child,
            parent_offset
                + Pos {
                    x: layout.location.x + layout.border.left,
                    y: layout.location.y + layout.border.top,
                    z: 0.,
                },
            taffy,
            mouse,
        ) {
            return Some(element);
        }
    }
    let x = layout.location.x + layout.border.left + parent_offset.x;
    let y = layout.location.y + layout.border.top + parent_offset.y;
    let width = layout.size.width - layout.border.left - layout.border.right;
    let height = layout.size.height - layout.border.top - layout.border.bottom;
    if mouse.x >= x && mouse.x <= x + width && mouse.y >= y && mouse.y <= y + height {
        Some(taffy.get_node_context(node).unwrap())
    } else {
        None
    }
}
fn render_element(
    node: NodeId,
    taffy: &TaffyTree<&UIElement>,
    size: PhysicalSize<u32>,
    parent_offset: Pos,
    data: &ScreenData,
    game: &ClientGame,
    mesh: &mut GUIMesh,
) {
    let layout = taffy.layout(node).unwrap();
    let element = *taffy.get_node_context(node).unwrap();
    let aspect_ratio = size.width as f32 / size.height as f32;
    let style = get_element_style(element, &data.properties);
    if let Some(background) = style.background {
        let width = layout.size.width - layout.border.left - layout.border.right;
        let height = layout.size.height - layout.border.top - layout.border.bottom;

        mesh.add_quad(
            UIRect {
                pos: UIPos {
                    x: (layout.location.x + layout.border.left + parent_offset.x)
                        / size.height as f32
                        * 2.
                        - aspect_ratio,
                    y: -((layout.location.y + layout.border.top + parent_offset.y + height)
                        / size.height as f32
                        * 2.
                        - 1.),
                },
                size: UIPos {
                    x: width / size.height as f32 * 2.,
                    y: height / size.height as f32 * 2.,
                },
            },
            background.tex_coords(),
            Color::WHITE,
        );
    }
    match &element.element_type {
        UIElementType::Box(uielements) => {
            for child in taffy.children(node).unwrap() {
                render_element(
                    child,
                    taffy,
                    size,
                    parent_offset
                        + Pos {
                            x: layout.location.x + layout.border.left,
                            y: layout.location.y + layout.border.top,
                            z: 0.,
                        },
                    data,
                    game,
                    mesh,
                );
            }
        }
        UIElementType::Label(text) => {
            text_renderer().draw(
                UIPos {
                    x: (layout.content_box_x() + parent_offset.x) / size.height as f32 * 2.
                        - aspect_ratio,
                    y: -((layout.content_box_y() + parent_offset.y + layout.content_box_height())
                        / size.height as f32
                        * 2.
                        - 1.),
                },
                &text,
                style.font_size / size.height as f32 * 2.,
                Color::WHITE,
                mesh,
            );
        }
        UIElementType::Image(key, width, height) => {
            mesh.add_quad(
                UIRect {
                    pos: UIPos {
                        x: (layout.content_box_x() + parent_offset.x) / size.height as f32 * 2.
                            - aspect_ratio,
                        y: -((layout.content_box_y()
                            + parent_offset.y
                            + layout.content_box_height())
                            / size.height as f32
                            * 2.
                            - 1.),
                    },
                    size: UIPos {
                        x: width / size.height as f32 * 2.,
                        y: height / size.height as f32 * 2.,
                    },
                },
                key.tex_coords(),
                Color::WHITE,
            );
        }
        UIElementType::ItemSlot { slot } => {
            mesh.add_quad(
                UIRect {
                    pos: UIPos {
                        x: (layout.content_box_x() + parent_offset.x) / size.height as f32 * 2.
                            - aspect_ratio,
                        y: -((layout.content_box_y()
                            + parent_offset.y
                            + layout.content_box_height())
                            / size.height as f32
                            * 2.
                            - 1.),
                    },
                    size: UIPos {
                        x: 50. / size.height as f32 * 2.,
                        y: 50. / size.height as f32 * 2.,
                    },
                },
                TextureKey::id("slot").unwrap().tex_coords(),
                Color::WHITE,
            );
            if let Some(background) = style.background {
                mesh.add_quad(
                    UIRect {
                        pos: UIPos {
                            x: (layout.content_box_x() + parent_offset.x) / size.height as f32 * 2.
                                - aspect_ratio,
                            y: -((layout.content_box_y()
                                + parent_offset.y
                                + layout.content_box_height())
                                / size.height as f32
                                * 2.
                                - 1.),
                        },
                        size: UIPos {
                            x: 50. / size.height as f32 * 2.,
                            y: 50. / size.height as f32 * 2.,
                        },
                    },
                    background.tex_coords(),
                    Color::WHITE,
                );
            }
            if let Some(item) = data.slots.get(*slot).cloned().flatten() {
                let border = 3.;
                let mut vertex_consumer = || {};
                let item_data = item.item.data();
                let matrix = cgmath::perspective(cgmath::Deg(20.), 1., 0.05, 5.)
                    * item_model_icon_view(&item_data.model);
                crate::render::draw_item_model(
                    &item_data.model,
                    Matrix4::identity(),
                    &mut |pos, texture, normal| {
                        let x = (layout.content_box_x() + border + parent_offset.x)
                            / size.height as f32
                            * 2.
                            - aspect_ratio;
                        let y = -((layout.content_box_y() - border
                            + parent_offset.y
                            + layout.content_box_height())
                            / size.height as f32
                            * 2.
                            - 1.);
                        let s = (50. - border * 2.) / size.height as f32 * 2.;
                        let pos = matrix.transform_point(cgmath::Point3 {
                            x: pos[0],
                            y: pos[1],
                            z: pos[2],
                        });
                        let normal = Pos {
                            x: normal[0],
                            y: normal[1],
                            z: normal[2],
                        };
                        let light = Pos {
                            x: 1.,
                            y: 1.,
                            z: 1.,
                        }
                        .normalize();
                        let dot = normal.dot(light);
                        if dot > 0. {
                            let shade_color = 1. - normal.x.abs() * 0.5 - normal.z.abs() * 0.2;
                            mesh.vertices.push(GUIVertex {
                                color: Color::grayscale((shade_color * 255.) as u8).into(),
                                tex_coords: texture,
                                position: [x + (pos.x + 1.) / 2. * s, y + (pos.y + 1.) / 2. * s],
                            });
                        }
                    },
                );
                text_renderer().draw(
                    UIPos {
                        x: (layout.content_box_x() + border + parent_offset.x) / size.height as f32
                            * 2.
                            - aspect_ratio,
                        y: -((layout.content_box_y() - border
                            + parent_offset.y
                            + layout.content_box_height())
                            / size.height as f32
                            * 2.
                            - 1.),
                    },
                    &format!("{}", item.count),
                    20. / size.height as f32 * 2.,
                    Color::WHITE,
                    mesh,
                );
            }
        }
        UIElementType::CraftArea { recipes } => todo!(),
        UIElementType::ResearchTree { research } => todo!(),
    }
}
fn add_element_to_taffy<'a>(
    element: &'a UIElement,
    taffy: &mut TaffyTree<&'a UIElement>,
    properties: &PropertyMap,
) -> NodeId {
    let style = get_element_style(element, properties);
    match &element.element_type {
        UIElementType::Box(children) => {
            let children = children
                .iter()
                .map(|child| add_element_to_taffy(child, taffy, properties))
                .collect::<Vec<_>>();
            let node = taffy.new_with_children(style.taffy, &children[..]).unwrap();
            taffy.set_node_context(node, Some(element));
            node
        }
        _ => {
            let node = taffy.new_leaf_with_context(style.taffy, element).unwrap();
            node
        }
    }
}
fn get_element_style(element: &UIElement, properties: &PropertyMap) -> BBStyle {
    //todo: this should be precomputed
    let mut style = BBStyle::default();
    for style_list in &element.style_classes {
        for (rule, condition) in &style_list.data().0 {
            if let Some(condition) = &condition {
                if !condition.satisfies(properties) {
                    continue;
                }
            }
            style.patch(rule);
        }
    }
    for (rule, condition) in &element.style.0 {
        if let Some(condition) = &condition {
            if !condition.satisfies(properties) {
                continue;
            }
        }
        style.patch(rule);
    }
    style
}
struct BBStyle {
    taffy: taffy::Style,
    background: Option<TextureKey>,
    font_size: f32,
}
impl Default for BBStyle {
    fn default() -> Self {
        Self {
            taffy: Style::DEFAULT,
            background: None,
            font_size: 60.,
        }
    }
}
impl BBStyle {
    pub fn patch(&mut self, rule: &UIStyleRule) {
        match rule {
            UIStyleRule::FlexDirection(flex_direction) => {
                self.taffy.flex_direction = *flex_direction;
            }
            UIStyleRule::Width(width) => {
                self.taffy.size.width = (*width).into();
            }
            UIStyleRule::Height(height) => {
                self.taffy.size.height = (*height).into();
            }
            UIStyleRule::PaddingLeft(style_length) => {
                self.taffy.padding.left = (*style_length).into();
            }
            UIStyleRule::PaddingRight(style_length) => {
                self.taffy.padding.right = (*style_length).into();
            }
            UIStyleRule::PaddingTop(style_length) => {
                self.taffy.padding.top = (*style_length).into();
            }
            UIStyleRule::PaddingBottom(style_length) => {
                self.taffy.padding.bottom = (*style_length).into();
            }
            UIStyleRule::MarginLeft(style_length) => {
                self.taffy.margin.left = (*style_length).into();
            }
            UIStyleRule::MarginRight(style_length) => {
                self.taffy.margin.right = (*style_length).into();
            }
            UIStyleRule::MarginTop(style_length) => {
                self.taffy.margin.top = (*style_length).into();
            }
            UIStyleRule::MarginBottom(style_length) => {
                self.taffy.margin.bottom = (*style_length).into();
            }
            UIStyleRule::FontSize(font_size) => {
                self.font_size = *font_size;
            }
            UIStyleRule::Background(key) => {
                self.background = Some(*key);
            }
            UIStyleRule::AlignItems(align_items) => {
                self.taffy.align_items = Some(*align_items);
            }
            UIStyleRule::AlignContent(align_content) => {
                self.taffy.align_content = Some(*align_content);
            }
            UIStyleRule::AlignSelf(align_self) => {
                self.taffy.align_self = Some(*align_self);
            }
            UIStyleRule::JustifyItems(justify_items) => {
                self.taffy.justify_items = Some(*justify_items);
            }
            UIStyleRule::JustifyContent(justify_content) => {
                self.taffy.justify_content = Some(*justify_content);
            }
            UIStyleRule::JustifySelf(justify_self) => {
                self.taffy.justify_self = Some(*justify_self);
            }
            UIStyleRule::Position(position) => {
                self.taffy.position = *position;
            }
            UIStyleRule::InsetLeft(style_length) => {
                self.taffy.inset.left = (*style_length).into();
            }
            UIStyleRule::InsetRight(style_length) => {
                self.taffy.inset.right = (*style_length).into();
            }
            UIStyleRule::InsetTop(style_length) => {
                self.taffy.inset.top = (*style_length).into();
            }
            UIStyleRule::InsetBottom(style_length) => {
                self.taffy.inset.bottom = (*style_length).into();
            }
            UIStyleRule::FlexWrap(flex_wrap) => {
                self.taffy.flex_wrap = *flex_wrap;
            }
            UIStyleRule::GapColumn(style_length) => {
                self.taffy.gap.width = (*style_length).into();
            }
            UIStyleRule::GapRow(style_length) => {
                self.taffy.gap.height = (*style_length).into();
            }
            UIStyleRule::Display(display) => {
                self.taffy.display = *display;
            }
        }
    }
}
pub struct TextRenderer {
    pub font: rusttype::Font<'static>,
    pub glyphs: Vec<TexCoords>,
}
impl TextRenderer {
    pub fn get_size(&self, text: &str, size: f32) -> Pos {
        let layout = self.font.layout(
            text,
            rusttype::Scale::uniform(size),
            rusttype::Point { x: 0., y: 0. },
        );
        let glyphs: Vec<_> = layout.collect();
        let width: f32 = glyphs
            .iter()
            .map(|glyph| glyph.unpositioned().h_metrics().advance_width)
            .sum();
        let height = glyphs
            .iter()
            .map(|glyph| {
                glyph
                    .unpositioned()
                    .exact_bounding_box()
                    .map(|bb| -bb.min.y + bb.max.y)
                    .unwrap_or(0.)
            })
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.);
        Pos {
            x: width,
            y: height,
            z: 0.,
        }
    }
    pub fn draw(
        &self,
        position: UIPos,
        text: &str,
        size: f32,
        color: Color,
        mesh: &mut GUIMesh,
    ) -> UIPos {
        let layout = self.font.layout(
            text,
            rusttype::Scale::uniform(size),
            rusttype::Point { x: 0., y: 0. },
        );
        let glyphs: Vec<_> = layout.collect();
        let width: f32 = glyphs
            .iter()
            .map(|glyph| glyph.unpositioned().h_metrics().advance_width)
            .sum();
        let height = glyphs
            .iter()
            .map(|glyph| {
                glyph
                    .unpositioned()
                    .exact_bounding_box()
                    .map(|bb| -bb.min.y + bb.max.y)
                    .unwrap_or(0.)
            })
            .max_by(|a, b| a.partial_cmp(b).unwrap())
            .unwrap_or(0.);
        for glyph in glyphs {
            if let Some(bb) = glyph.unpositioned().exact_bounding_box() {
                let bb = rusttype::Rect {
                    min: rusttype::point(bb.min.x, -bb.max.y),
                    max: rusttype::point(bb.max.x, -bb.min.y),
                };
                let texture = self.glyphs[glyph.id().0 as usize];
                let size_x = -bb.min.x + bb.max.x;
                let size_y = -bb.min.y + bb.max.y;
                let x = glyph.position().x + bb.min.x + position.x;
                let y = glyph.position().y + bb.min.y + position.y;
                mesh.add_quad(
                    UIRect {
                        pos: UIPos { x, y },
                        size: UIPos {
                            x: size_x,
                            y: size_y,
                        },
                    },
                    texture,
                    color,
                );
            }
        }
        UIPos {
            x: width,
            y: height,
        }
    }
}
pub static TEXT_RENDERER: OnceLock<TextRenderer> = OnceLock::new();
pub fn text_renderer() -> &'static TextRenderer {
    TEXT_RENDERER.get().unwrap()
}
#[derive(Copy, Clone)]
pub struct UIPos {
    pub x: f32,
    pub y: f32,
}
#[derive(Copy, Clone)]
pub struct UIRect {
    pub pos: UIPos,
    pub size: UIPos,
}
