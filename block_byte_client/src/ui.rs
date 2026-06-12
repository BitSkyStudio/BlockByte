use std::{cell::RefCell, collections::HashMap, sync::OnceLock, u32};

use block_byte_common::{
    ClientItem, Color, ItemMoveMode, TexCoords,
    coord::Pos,
    net::{NetworkMessageC2S, PropertyModifyMode},
    registry::{BlockRenderData, ItemKey, ItemModel, Key, RecipeKey, ResearchKey, TextureKey},
    scripts::ScriptValue,
    ui::{
        CraftAreaRecipes, PropertyMap, SlotId, StretchTexture, StyleLength, UIElement,
        UIElementType, UIScreen, UIScreenKey, UIStyleRule,
    },
};
use cgmath::{Matrix4, SquareMatrix, Transform, Vector3};
use taffy::{
    AlignItems, AvailableSpace, Dimension, FlexDirection, JustifyContent, Layout, LengthPercentage,
    LengthPercentageAuto, NodeId, Rect, Style, TaffyTree, prelude::TaffyZero,
};
use winit::{
    dpi::{PhysicalPosition, PhysicalSize},
    event::MouseButton,
    keyboard::KeyCode,
};

use crate::{
    ClientGame, ClientPlayer, GUIMesh, InputContainer, TEXTURE_ATLAS, TexCoordsExt,
    TexCoordsIndexExt,
    render::{CameraUniform, GUIVertex, MeshVertexConsumer, item_model_icon_view},
    translate,
};

pub struct ScreenData {
    pub screen: UIScreenKey,
    pub slots: Vec<Option<ClientItem>>,
    pub properties: PropertyMap,
    pub selected_slot: Option<(usize, MouseButton)>,
    pub slot_action_prediction: HashMap<usize, (MouseButton, f32)>,
}
pub struct UIInput<'a> {
    pub mouse_position: UIPos,
    pub last_mouse_position: UIPos,
    pub last_scroll: UIPos,
    pub buttons: &'a InputContainer<MouseButton>,
    pub keys: &'a InputContainer<KeyCode>,
}
impl UIInput<'_> {
    pub fn mouse_delta(&self) -> UIPos {
        UIPos {
            x: self.mouse_position.x - self.last_mouse_position.x,
            y: self.mouse_position.y - self.last_mouse_position.y,
        }
    }
}
pub fn render_screen(
    screen_data: &mut ScreenData,
    input: Option<&UIInput>,
    size: PhysicalSize<u32>,
    mesh: &mut GUIMesh,
    dt: f32,
    message_queue: &mut Vec<NetworkMessageC2S>,
) {
    if let Some(selected_slot) = screen_data.selected_slot {
        screen_data
            .slot_action_prediction
            .insert(selected_slot.0, (selected_slot.1, 0.1));
    }
    screen_data.slot_action_prediction.retain(|_, (_, time)| {
        *time -= dt;
        *time > 0.
    });
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
    let mut overlay_mesh = GUIMesh::default();
    render_element(
        root,
        &taffy,
        size,
        UIPos::all(0.),
        screen_data,
        mesh,
        &mut overlay_mesh,
        input,
        message_queue,
    );
    mesh.append_mesh(overlay_mesh);
    if let Some(input) = input {
        if let Some((_, button)) = screen_data.selected_slot.as_ref() {
            if !input.buttons.is_down(*button) {
                screen_data.selected_slot = None;
            }
        }
    }
}
pub fn measure_element(element: &UIElement, properties: &PropertyMap) -> taffy::Size<f32> {
    let style = get_element_style(element, properties);
    match &element.element_type {
        UIElementType::Box(uielements) => taffy::Size::ZERO,
        UIElementType::Label(text) | UIElementType::Button { text, .. } => {
            let size = text_renderer().get_size(
                properties.patch_text(text.as_str()).as_str(),
                style.font_size,
            );
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
        UIElementType::CraftArea {
            recipes,
            craft_width,
        } => {
            let craft_size = 50.;
            let craft_width = *craft_width as f32;
            taffy::Size {
                width: craft_width * craft_size,
                height: (match recipes {
                    CraftAreaRecipes::Recipes(recipes) => recipes.list().len(),
                    CraftAreaRecipes::CheatMenu => ItemKey::entries().count(), //todo: just cache this number
                } as f32
                    / craft_width)
                    .ceil()
                    * craft_size,
            }
        }
        UIElementType::ResearchTree { .. } => taffy::Size {
            width: 100.,
            height: 100.,
        },
    }
}
fn render_element(
    node: NodeId,
    taffy: &TaffyTree<&UIElement>,
    size: PhysicalSize<u32>,
    parent_offset: UIPos,
    data: &mut ScreenData,
    mesh: &mut GUIMesh,
    overlay_mesh: &mut GUIMesh,
    input: Option<&UIInput>,
    message_queue: &mut Vec<NetworkMessageC2S>,
) {
    let layout = taffy.layout(node).unwrap();
    let element = *taffy.get_node_context(node).unwrap();
    let aspect_ratio = size.width as f32 / size.height as f32;
    let style = get_element_style(element, &data.properties);
    if let Some(background) = style.background {
        let mut bg_context = UIElementRenderContext {
            aspect_ratio,
            buffer: mesh,
            gui_size: size.height as f32,
            content: UIRect {
                pos: UIPos {
                    x: layout.location.x + parent_offset.x,
                    y: layout.location.y + parent_offset.y,
                },
                size: UIPos {
                    x: layout.size.width,
                    y: layout.size.height,
                },
            },
        };
        let texture = background.texture.tex_coords();
        let bg_texture_data = &background.texture.data().texture;
        let (border_u, border_v) = (
            background.border as f32 / bg_texture_data.width() as f32,
            background.border as f32 / bg_texture_data.height() as f32,
        );
        let grid_pos_x = [
            0.,
            layout.border.left,
            layout.size.width - layout.border.right,
            layout.size.width,
        ];
        let grid_pos_y = [
            0.,
            layout.border.top,
            layout.size.height - layout.border.bottom,
            layout.size.height,
        ];
        let grid_uv_x = [0., border_u, 1. - border_u, 1.];
        let grid_uv_y = [0., border_v, 1. - border_v, 1.];
        for x in 0..3 {
            for y in 0..3 {
                let x1 = grid_pos_x[x];
                let x2 = grid_pos_x[x + 1];
                let y1 = grid_pos_y[y];
                let y2 = grid_pos_y[y + 1];
                bg_context.draw_quad(
                    UIRect {
                        pos: UIPos { x: x1, y: y1 },
                        size: UIPos {
                            x: x2 - x1,
                            y: y2 - y1,
                        },
                    },
                    texture.map_sub(TexCoords {
                        u1: grid_uv_x[x],
                        v1: grid_uv_y[y],
                        u2: grid_uv_x[x + 1],
                        v2: grid_uv_y[y + 1],
                    }),
                    Color::WHITE,
                );
            }
        }
    }
    let mut context = UIElementRenderContext {
        aspect_ratio,
        buffer: mesh,
        gui_size: size.height as f32,
        content: UIRect {
            pos: UIPos {
                x: layout.content_box_x() + parent_offset.x,
                y: layout.content_box_y() + parent_offset.y,
            },
            size: UIPos {
                x: layout.content_box_width(),
                y: layout.content_box_height(),
            },
        },
    };
    let mut overlay_context = UIElementRenderContext {
        aspect_ratio,
        buffer: overlay_mesh,
        gui_size: size.height as f32,
        content: UIRect {
            pos: UIPos { x: 0., y: 0. },
            size: UIPos {
                x: size.width as f32,
                y: size.height as f32,
            },
        },
    };
    match &element.element_type {
        UIElementType::Box(uielements) => {
            let parent_offset = UIPos {
                x: context.content.pos.x - layout.border.left - layout.padding.left,
                y: context.content.pos.y - layout.border.top - layout.padding.top,
            };
            for child in taffy.children(node).unwrap() {
                render_element(
                    child,
                    taffy,
                    size,
                    parent_offset,
                    data,
                    mesh,
                    overlay_mesh,
                    input,
                    message_queue,
                );
            }
        }
        UIElementType::Label(text) => {
            context.draw_text(
                UIPos::all(0.),
                data.properties.patch_text(text.as_str()).as_str(),
                20.,
                Color::WHITE,
            );
        }
        UIElementType::Image(key, width, height) => {
            context.draw_quad(
                UIRect {
                    pos: UIPos::all(0.),
                    size: UIPos {
                        x: *width,
                        y: *height,
                    },
                },
                key.tex_coords(),
                Color::WHITE,
            );
        }
        UIElementType::ItemSlot { slot } => {
            if let Some(input) = input {
                if context.content.contains(input.mouse_position) {
                    let target_slot = *slot;
                    match data.selected_slot {
                        Some((slot, button)) => {
                            let move_mode = match button {
                                MouseButton::Left => {
                                    if input.buttons.is_just_up(MouseButton::Left) {
                                        Some(ItemMoveMode::Stack)
                                    } else if input.buttons.is_just_down(MouseButton::Right) {
                                        Some(ItemMoveMode::Single)
                                    } else {
                                        None
                                    }
                                }
                                MouseButton::Right => {
                                    if input.buttons.is_just_up(MouseButton::Right) {
                                        Some(ItemMoveMode::Half)
                                    } else {
                                        None
                                    }
                                }
                                _ => unreachable!(),
                            };
                            if let Some(mode) = move_mode {
                                match target_slot {
                                    SlotId::Id(target_slot) => {
                                        message_queue.push(NetworkMessageC2S::MoveItem {
                                            from: slot,
                                            to: target_slot,
                                            mode,
                                        });
                                    }
                                    SlotId::Trash => {
                                        message_queue
                                            .push(NetworkMessageC2S::TrashItem { slot, mode });
                                    }
                                }
                            }
                        }
                        None => match target_slot {
                            SlotId::Id(target_slot) => {
                                for button in [MouseButton::Left, MouseButton::Right] {
                                    if input.buttons.is_just_down(button) {
                                        data.selected_slot = Some((target_slot, button));
                                        break;
                                    }
                                }
                            }
                            SlotId::Trash => {}
                        },
                    }
                }
            }
            match slot {
                SlotId::Id(slot) => {
                    if let Some(item) = data.slots.get(*slot).cloned().flatten() {
                        if let Some(input) = input
                            && context.content.contains(input.mouse_position)
                            && data.selected_slot.is_none()
                        {
                            let mut shift = overlay_context
                                .draw_text(
                                    input.mouse_position,
                                    translate(format!("item.{}", item.item.text_id()).as_str()),
                                    40.,
                                    Color::WHITE,
                                )
                                .y;
                            for line in item.description.lines() {
                                shift += overlay_context
                                    .draw_text(
                                        UIPos {
                                            x: input.mouse_position.x,
                                            y: input.mouse_position.y + shift,
                                        },
                                        line,
                                        40.,
                                        Color::WHITE,
                                    )
                                    .y;
                            }
                        }
                        let border = 3.;
                        let item_data = item.item.data();

                        let mut draw_slot =
                            |context: &mut UIElementRenderContext, position: UIPos, count: u16| {
                                context.draw_icon(
                                    UIRect {
                                        pos: position,
                                        size: UIPos::all(50. - border * 2.),
                                    },
                                    &item_data.model,
                                );
                                let text = format!("{}", count);
                                let size = text_renderer()
                                    .get_size(&text, 20. / context.gui_size as f32 * 2.);
                                let count_text_offset = 2.;
                                context.draw_text(
                                    UIPos {
                                        x: position.x + 50.
                                            - size.x * context.gui_size / 2.
                                            - count_text_offset,
                                        y: position.y + 50. - count_text_offset,
                                    },
                                    &text,
                                    20.,
                                    Color::WHITE,
                                );
                            };

                        if let Some((selected_button, _)) = data.slot_action_prediction.get(slot) {
                            if let Some(input) = input {
                                match selected_button {
                                    MouseButton::Left => {
                                        draw_slot(
                                            &mut overlay_context,
                                            UIPos {
                                                x: input.mouse_position.x + border - 25.,
                                                y: input.mouse_position.y + border - 25.,
                                            },
                                            item.count,
                                        );
                                    }
                                    MouseButton::Right => {
                                        let move_count = item.count.div_ceil(2);
                                        draw_slot(
                                            &mut context,
                                            UIPos::all(border),
                                            item.count - move_count,
                                        );
                                        draw_slot(
                                            &mut overlay_context,
                                            UIPos {
                                                x: input.mouse_position.x + border - 25.,
                                                y: input.mouse_position.y + border - 25.,
                                            },
                                            move_count,
                                        );
                                    }
                                    _ => unreachable!(),
                                }
                            }
                        } else {
                            draw_slot(&mut context, UIPos::all(border), item.count);
                        }
                    }
                }
                SlotId::Trash => {}
            }
        }
        UIElementType::CraftArea {
            recipes,
            craft_width,
        } => {
            let craft_size = 50.;
            match recipes {
                CraftAreaRecipes::CheatMenu => {
                    for (i, item) in ItemKey::entries().enumerate() {
                        let item_data = item.data();
                        let area = UIRect {
                            pos: UIPos {
                                x: (i % (*craft_width as usize)) as f32 * craft_size,
                                y: (i / (*craft_width as usize)) as f32 * craft_size,
                            },
                            size: UIPos::all(craft_size),
                        };
                        context.draw_icon(area, &item_data.model);
                        if let Some(input) = input {
                            if (UIRect {
                                pos: UIPos {
                                    x: area.pos.x + context.content.pos.x,
                                    y: area.pos.y + context.content.pos.y,
                                },
                                size: UIPos::all(craft_size),
                            })
                            .contains(input.mouse_position)
                                && data.selected_slot.is_none()
                            {
                                overlay_context.draw_text(
                                    input.mouse_position,
                                    translate(format!("item.{}", item.text_id()).as_str()),
                                    40.,
                                    Color::WHITE,
                                );
                                for (button, stack) in
                                    [(MouseButton::Left, false), (MouseButton::Right, true)]
                                {
                                    if input.buttons.is_just_down(button) {
                                        message_queue
                                            .push(NetworkMessageC2S::GiveItem { item, stack });
                                    }
                                }
                            }
                        }
                    }
                }
                CraftAreaRecipes::Recipes(recipes) => {
                    for (i, recipe) in recipes.list().iter().enumerate() {
                        let recipe_data = recipe.data();
                        let area = UIRect {
                            pos: UIPos {
                                x: (i % (*craft_width as usize)) as f32 * craft_size,
                                y: (i / (*craft_width as usize)) as f32 * craft_size,
                            },
                            size: UIPos::all(craft_size),
                        };
                        context.draw_icon(
                            area,
                            match &recipe_data.icon_override {
                                Some(icon) => icon,
                                None => &recipe_data.outputs.data().entries[0].item.data().model,
                            },
                        );
                        if let Some(input) = input {
                            if (UIRect {
                                pos: UIPos {
                                    x: area.pos.x + context.content.pos.x,
                                    y: area.pos.y + context.content.pos.y,
                                },
                                size: UIPos::all(craft_size),
                            })
                            .contains(input.mouse_position)
                            {
                                let mut text =
                                    translate(format!("recipe.{}", recipe.text_id()).as_str())
                                        .to_string();
                                for (input_item, input_count) in &recipe_data.inputs {
                                    text += format!(
                                        "\n-{}x{}",
                                        *input_count,
                                        translate(
                                            format!("item.{}", input_item.text_id()).as_str()
                                        )
                                    )
                                    .as_str();
                                }
                                for loot_entry in &recipe_data.outputs.data().entries {
                                    //todo: somehow do modifiers
                                    text += format!(
                                        "\n+{}",
                                        translate(
                                            format!("item.{}", loot_entry.item.text_id()).as_str()
                                        )
                                    )
                                    .as_str();
                                }
                                overlay_context.draw_multiline_text(
                                    input.mouse_position,
                                    text.as_str(),
                                    40.,
                                    Color::WHITE,
                                );
                                for (button, count) in
                                    [(MouseButton::Left, 1), (MouseButton::Right, 5)]
                                {
                                    if input.buttons.is_just_down(button) {
                                        message_queue.push(NetworkMessageC2S::Craft {
                                            recipe: *recipe,
                                            count,
                                        });
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        UIElementType::ResearchTree { research } => {
            let research_size = 50.;
            for research in research.list() {
                let research_data = research.data();
                let area = UIRect {
                    pos: UIPos {
                        x: research_data.x * research_size + context.content.size.x / 2.,
                        y: research_data.y * research_size + context.content.size.y / 2.,
                    },
                    size: UIPos::all(research_size),
                };
                context.draw_icon(area, &research_data.icon);
                if let Some(input) = input {
                    if (UIRect {
                        pos: UIPos {
                            x: area.pos.x + context.content.pos.x,
                            y: area.pos.y + context.content.pos.y,
                        },
                        size: UIPos::all(research_size),
                    })
                    .contains(input.mouse_position)
                    {
                        overlay_context.draw_text(
                            input.mouse_position,
                            translate(format!("research.{}", research.text_id()).as_str()),
                            40.,
                            Color::WHITE,
                        );
                        if input.buttons.is_just_down(MouseButton::Left) {
                            message_queue.push(NetworkMessageC2S::Research {
                                research: *research,
                            });
                        }
                    }
                }
            }
        }
        UIElementType::Button {
            text,
            property,
            value,
            modify_mode,
        } => {
            context.draw_text(
                UIPos::all(0.),
                data.properties.patch_text(text.as_str()).as_str(),
                20.,
                Color::WHITE,
            );
            if let Some(input) = input
                && context.content.contains(input.mouse_position)
            {
                if input.buttons.is_just_down(MouseButton::Left) {
                    message_queue.push(NetworkMessageC2S::UIButtonPress {
                        property: property.clone(),
                        value: *value,
                        modify_mode: *modify_mode,
                    });
                }
            }
        }
    }
}
struct UIElementRenderContext<'a> {
    buffer: &'a mut GUIMesh,
    content: UIRect,
    gui_size: f32,
    aspect_ratio: f32,
}
impl UIElementRenderContext<'_> {
    pub fn draw_quad(&mut self, quad: UIRect, texture: TexCoords, color: Color) {
        self.buffer.add_quad_clip(
            UIRect {
                pos: UIPos {
                    x: (self.content.pos.x + quad.pos.x) / self.gui_size as f32 * 2.
                        - self.aspect_ratio,
                    y: -((self.content.pos.y + quad.pos.y + quad.size.y) / self.gui_size as f32
                        * 2.
                        - 1.),
                },
                size: UIPos {
                    x: quad.size.x / self.gui_size as f32 * 2.,
                    y: quad.size.y / self.gui_size as f32 * 2.,
                },
            },
            texture,
            color,
            UIRect {
                pos: UIPos {
                    x: self.content.pos.x / self.gui_size as f32 * 2. - self.aspect_ratio,
                    y: -((self.content.pos.y + self.content.size.y) / self.gui_size as f32 * 2.
                        - 1.),
                },
                size: UIPos {
                    x: self.content.size.x / self.gui_size as f32 * 2.,
                    y: self.content.size.y / self.gui_size as f32 * 2.,
                },
            },
        );
    }
    pub fn draw_text(&mut self, position: UIPos, text: &str, size: f32, color: Color) -> UIPos {
        //todo: clip
        let size = text_renderer().draw(
            UIPos {
                x: (self.content.pos.x + position.x) / self.gui_size as f32 * 2.
                    - self.aspect_ratio,
                y: -((self.content.pos.y + position.y) / self.gui_size as f32 * 2. - 1.),
            },
            text,
            size / self.gui_size as f32 * 2.,
            color,
            self.buffer,
        );
        UIPos {
            x: size.x * self.gui_size / 2.,
            y: size.y * self.gui_size / 2.,
        }
    }
    pub fn draw_multiline_text(&mut self, position: UIPos, text: &str, size: f32, color: Color) {
        let row_gap = 10.;
        let mut y_shift = 0.;
        for line in text.lines() {
            let size = self.draw_text(
                UIPos {
                    x: position.x,
                    y: position.y + y_shift,
                },
                line,
                size,
                color,
            );
            y_shift += size.y + row_gap;
        }
    }
    pub fn draw_icon(&mut self, quad: UIRect, icon: &ItemModel) {
        //todo: clip
        let matrix =
            cgmath::perspective(cgmath::Deg(20.), 1., 0.05, 5.) * item_model_icon_view(icon);
        let light = Pos {
            x: 1.,
            y: 3.,
            z: 2.,
        }
        .normalize();
        struct IconVertexConsumer<'a> {
            pub mesh: &'a mut GUIMesh,
            pub projection: Matrix4<f32>,
            pub light: Pos,
            pub rect: UIRect,
        }
        impl MeshVertexConsumer for IconVertexConsumer<'_> {
            fn add_vertex(&mut self, vertex: crate::render::MeshVertex) -> u32 {
                let position = vertex.position.multiply_point(self.projection);
                if vertex.normal.x + vertex.normal.y + vertex.normal.z <= 0. {
                    return u32::MAX;
                }
                let light_dot = vertex.normal.dot(self.light);
                self.mesh.add_vertex(GUIVertex {
                    position: [
                        self.rect.pos.x + (position.x + 1.) / 2. * self.rect.size.x,
                        self.rect.pos.y + (position.y + 1.) / 2. * self.rect.size.y,
                    ],
                    tex_coords: vertex.uv,
                    color: Color::grayscale(((0.5 + light_dot / 2.) * 255.) as u8).into(),
                })
            }
            fn add_index(&mut self, index: u32) {
                if index == u32::MAX {
                    return;
                }
                self.mesh.add_index(index);
            }
        }
        let x = (self.content.pos.x + quad.pos.x) / self.gui_size as f32 * 2. - self.aspect_ratio;
        let y = -((self.content.pos.y + quad.pos.y + quad.size.y) / self.gui_size as f32 * 2. - 1.);
        let w = quad.size.x / self.gui_size as f32 * 2.;
        let h = quad.size.y / self.gui_size as f32 * 2.;
        let mut icon_vertex_consumer = IconVertexConsumer {
            light,
            mesh: self.buffer,
            projection: matrix,
            rect: UIRect {
                pos: UIPos { x, y },
                size: UIPos { x: w, y: h },
            },
        };
        crate::render::draw_item_model(icon, Matrix4::identity(), &mut icon_vertex_consumer);
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
        for (rule, condition) in &style_list.data().rules {
            if let Some(condition) = &condition {
                if !condition.satisfies(properties) {
                    continue;
                }
            }
            style.patch(rule, properties);
        }
    }
    for (rule, condition) in &element.style.rules {
        if let Some(condition) = &condition {
            if !condition.satisfies(properties) {
                continue;
            }
        }
        style.patch(rule, properties);
    }
    style
}
struct BBStyle {
    taffy: taffy::Style,
    background: Option<StretchTexture>,
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
    pub fn patch(&mut self, rule: &UIStyleRule, properties: &PropertyMap) {
        match rule {
            UIStyleRule::FlexDirection(flex_direction) => {
                self.taffy.flex_direction = *flex_direction;
            }
            UIStyleRule::Width(width) => {
                self.taffy.size.width = width.as_dimension(properties);
            }
            UIStyleRule::Height(height) => {
                self.taffy.size.height = height.as_dimension(properties);
            }
            UIStyleRule::PaddingLeft(style_length) => {
                self.taffy.padding.left = style_length.as_length_percentage(properties);
            }
            UIStyleRule::PaddingRight(style_length) => {
                self.taffy.padding.right = style_length.as_length_percentage(properties);
            }
            UIStyleRule::PaddingTop(style_length) => {
                self.taffy.padding.top = style_length.as_length_percentage(properties);
            }
            UIStyleRule::PaddingBottom(style_length) => {
                self.taffy.padding.bottom = style_length.as_length_percentage(properties);
            }
            UIStyleRule::MarginLeft(style_length) => {
                self.taffy.margin.left = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::MarginRight(style_length) => {
                self.taffy.margin.right = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::MarginTop(style_length) => {
                self.taffy.margin.top = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::MarginBottom(style_length) => {
                self.taffy.margin.bottom = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::BorderLeft(style_length) => {
                self.taffy.border.left = style_length.as_length_percentage(properties);
            }
            UIStyleRule::BorderRight(style_length) => {
                self.taffy.border.right = style_length.as_length_percentage(properties);
            }
            UIStyleRule::BorderTop(style_length) => {
                self.taffy.border.top = style_length.as_length_percentage(properties);
            }
            UIStyleRule::BorderBottom(style_length) => {
                self.taffy.border.bottom = style_length.as_length_percentage(properties);
            }
            UIStyleRule::FontSize(font_size) => {
                self.font_size = font_size.calc(properties);
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
                self.taffy.inset.left = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::InsetRight(style_length) => {
                self.taffy.inset.right = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::InsetTop(style_length) => {
                self.taffy.inset.top = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::InsetBottom(style_length) => {
                self.taffy.inset.bottom = style_length.as_length_percentage_auto(properties);
            }
            UIStyleRule::FlexWrap(flex_wrap) => {
                self.taffy.flex_wrap = *flex_wrap;
            }
            UIStyleRule::GapColumn(style_length) => {
                self.taffy.gap.width = style_length.as_length_percentage(properties);
            }
            UIStyleRule::GapRow(style_length) => {
                self.taffy.gap.height = style_length.as_length_percentage(properties);
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
pub fn text_renderer() -> &'static TextRenderer {
    &TEXTURE_ATLAS.get().unwrap().text_renderer
}
#[derive(Copy, Clone)]
pub struct UIPos {
    pub x: f32,
    pub y: f32,
}
impl UIPos {
    pub fn all(v: f32) -> UIPos {
        UIPos { x: v, y: v }
    }
}
#[derive(Copy, Clone)]
pub struct UIRect {
    pub pos: UIPos,
    pub size: UIPos,
}
impl UIRect {
    pub fn contains(self, pos: UIPos) -> bool {
        pos.x >= self.pos.x
            && pos.x <= self.pos.x + self.size.x
            && pos.y >= self.pos.y
            && pos.y <= self.pos.y + self.size.y
    }
}
