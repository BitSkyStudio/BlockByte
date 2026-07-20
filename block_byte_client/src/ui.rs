use std::{collections::HashMap, u32};

use block_byte_common::{
    ClientItem, Color, ItemMoveMode, TexCoords,
    coord::{Pos, Vec3},
    net::NetworkMessageC2S,
    registry::{ItemKey, ItemModel, TextureKey},
    ui::{
        CraftAreaRecipes, PropertyMap, SlotId, StretchTexture, UIElement, UIElementType,
        UIScreenKey, UIStyleRule,
    },
};
use cgmath::{Matrix4, SquareMatrix};
use smallvec::SmallVec;
use taffy::{AvailableSpace, Dimension, NodeId, Style, TaffyTree};
use uuid::Uuid;
use winit::{dpi::PhysicalSize, event::MouseButton, keyboard::NamedKey};

use crate::{
    GUIMesh, InputManager,
    atlas::TEXTURE_ATLAS,
    render::{GUIVertex, MeshVertex, MeshVertexConsumer, item_model_icon_view},
};
use crate::{atlas::TexCoordsExt, game::translate};

pub struct ScreenData {
    pub screen: UIScreenKey,
    pub slots: Vec<Option<ClientItem>>,
    pub properties: PropertyMap,
    pub selected_slot: Option<(usize, MouseButton)>,
    pub slot_action_prediction: HashMap<usize, (MouseButton, f32)>,
    pub element_data: HashMap<Uuid, InstantiatedElementData>,
    pub time: f32,
}
pub struct InstantiatedElementData {
    pub scroll: UIPos,
    pub text: String,
    pub text_cursor: usize,
    pub selected: bool,
}
impl Default for InstantiatedElementData {
    fn default() -> Self {
        InstantiatedElementData {
            scroll: UIPos { x: 0., y: 0. },
            text: String::new(),
            text_cursor: 0,
            selected: false,
        }
    }
}
pub fn render_screen(
    screen_data: &mut ScreenData,
    input: Option<&InputManager>,
    size: PhysicalSize<u32>,
    mesh: &mut GUIMesh,
    dt: f32,
    mut event_consumer: impl FnMut(UIMessage),
) {
    screen_data.time += dt;
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
        &mut event_consumer,
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
        UIElementType::Box(_)
        | UIElementType::CraftArea { .. }
        | UIElementType::ResearchTree { .. }
        | UIElementType::TextField { .. } => taffy::Size::ZERO,
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
        UIElementType::ItemSlot { .. } => taffy::Size {
            width: 50.,
            height: 50.,
        },
    }
}
pub enum UIMessage {
    ServerMessage(NetworkMessageC2S),
}
fn render_element(
    node: NodeId,
    taffy: &TaffyTree<&UIElement>,
    size: PhysicalSize<u32>,
    parent_offset: UIPos,
    data: &mut ScreenData,
    mesh: &mut GUIMesh,
    overlay_mesh: &mut GUIMesh,
    input: Option<&InputManager>,
    event_consumer: &mut impl FnMut(UIMessage),
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
        UIElementType::Box(_) => {
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
                    event_consumer,
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
        UIElementType::ItemSlot { slot } => {
            if let Some(input) = input {
                if context.content.contains(input.cursor_position) {
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
                                        event_consumer(UIMessage::ServerMessage(
                                            NetworkMessageC2S::MoveItem {
                                                from: slot,
                                                to: target_slot,
                                                mode,
                                            },
                                        ));
                                    }
                                    SlotId::Trash => {
                                        event_consumer(UIMessage::ServerMessage(
                                            NetworkMessageC2S::TrashItem { slot, mode },
                                        ));
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
                            && context.content.contains(input.cursor_position)
                            && data.selected_slot.is_none()
                        {
                            let mut shift = overlay_context
                                .draw_text(
                                    input.cursor_position,
                                    translate(format!("item.{}", item.item.text_id()).as_str()),
                                    40.,
                                    Color::WHITE,
                                )
                                .y;
                            for line in item.description.lines() {
                                shift += overlay_context
                                    .draw_text(
                                        UIPos {
                                            x: input.cursor_position.x,
                                            y: input.cursor_position.y + shift,
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

                        let draw_slot =
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
                                                x: input.cursor_position.x + border - 25.,
                                                y: input.cursor_position.y + border - 25.,
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
                                                x: input.cursor_position.x + border - 25.,
                                                y: input.cursor_position.y + border - 25.,
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
        UIElementType::CraftArea { recipes, filter } => {
            let craft_size = 50.;
            if context.content.size.x <= 0. {
                return;
            }
            let (element_data, filter) = match filter {
                Some(filter) => {
                    data.element_data.entry(element.uuid).or_default();
                    data.element_data.entry(*filter).or_default();
                    let [element_data, filter] =
                        data.element_data.get_disjoint_mut([&element.uuid, filter]);
                    (element_data.unwrap(), Some(filter.unwrap().text.as_str()))
                }
                None => (data.element_data.entry(element.uuid).or_default(), None),
            };
            let filter_passes = |text: &str| {
                if let Some(filter) = filter {
                    for filter_part in filter.split(" ") {
                        if !text.contains(filter_part) {
                            return false;
                        }
                    }
                }
                true
            };
            let top_offset = -element_data.scroll.y;
            let craft_width = (context.content.size.x / craft_size).floor() as usize;
            let left_offset = (context.content.size.x - craft_width as f32 * craft_size) / 2.;
            let mouse_inside = if let Some(input) = input {
                if context.content.contains(input.cursor_position) {
                    let crafts_count = match recipes {
                        CraftAreaRecipes::Recipes(key_group) => key_group.list().len(),
                        CraftAreaRecipes::CheatMenu => ItemKey::count(),
                    };
                    let max_scroll_height = (crafts_count.div_ceil(craft_width) as f32 * 50.
                        - context.content.size.y)
                        .max(0.);
                    element_data.scroll.y = (element_data.scroll.y
                        - input.wheel_scroll_delta.y * 20.)
                        .clamp(0., max_scroll_height);
                    true
                } else {
                    false
                }
            } else {
                false
            };
            match recipes {
                CraftAreaRecipes::CheatMenu => {
                    let mut i = 0;
                    for item in ItemKey::entries() {
                        let item_data = item.data();
                        let item_name = format!("item.{}", item.text_id());
                        let item_name = translate(item_name.as_str());
                        if !filter_passes(item_name) {
                            continue;
                        }
                        let area = UIRect {
                            pos: UIPos {
                                x: (i % craft_width) as f32 * craft_size + left_offset,
                                y: (i / craft_width) as f32 * craft_size + top_offset,
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
                            .contains(input.cursor_position)
                                && data.selected_slot.is_none()
                                && mouse_inside
                            {
                                overlay_context.draw_text(
                                    input.cursor_position,
                                    item_name,
                                    40.,
                                    Color::WHITE,
                                );
                                for (button, stack) in
                                    [(MouseButton::Left, false), (MouseButton::Right, true)]
                                {
                                    if input.buttons.is_just_down(button) {
                                        event_consumer(UIMessage::ServerMessage(
                                            NetworkMessageC2S::GiveItem { item, stack },
                                        ));
                                    }
                                }
                            }
                        }
                        i += 1;
                    }
                }
                CraftAreaRecipes::Recipes(recipes) => {
                    let mut i = 0;
                    for recipe in recipes.list() {
                        let recipe_data = recipe.data();
                        let mut text =
                            translate(format!("recipe.{}", recipe.text_id()).as_str()).to_string();
                        if !filter_passes(&text) {
                            continue;
                        }
                        let area = UIRect {
                            pos: UIPos {
                                x: (i % craft_width) as f32 * craft_size + left_offset,
                                y: (i / craft_width) as f32 * craft_size + top_offset,
                            },
                            size: UIPos::all(craft_size),
                        };
                        context.draw_icon(
                            area,
                            match &recipe_data.icon_override {
                                Some(icon) => icon,
                                None => {
                                    &recipe_data.outputs.data().pools[0].entries[0]
                                        .item
                                        .data()
                                        .model
                                }
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
                            .contains(input.cursor_position)
                                && mouse_inside
                            {
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
                                for loot_entry in &recipe_data.outputs.data().pools {
                                    //todo: somehow do modifiers
                                    text += format!(
                                        "\n+{}",
                                        translate(
                                            format!(
                                                "item.{}",
                                                loot_entry.entries[0].item.text_id()
                                            )
                                            .as_str()
                                        )
                                    )
                                    .as_str();
                                }
                                overlay_context.draw_multiline_text(
                                    input.cursor_position,
                                    text.as_str(),
                                    40.,
                                    Color::WHITE,
                                );
                                for (button, count) in
                                    [(MouseButton::Left, 1), (MouseButton::Right, 5)]
                                {
                                    if input.buttons.is_just_down(button) {
                                        event_consumer(UIMessage::ServerMessage(
                                            NetworkMessageC2S::Craft {
                                                recipe: *recipe,
                                                count,
                                            },
                                        ));
                                    }
                                }
                            }
                        }
                        i += 1;
                    }
                }
            }
        }
        UIElementType::ResearchTree { research } => {
            let element_data = data.element_data.entry(element.uuid).or_default();
            let mouse_inside = if let Some(input) = input {
                if context.content.contains(input.cursor_position) {
                    if data.selected_slot.is_none() && input.buttons.is_down(MouseButton::Left) {
                        element_data.scroll.x += input.mouse_delta.x as f32;
                        element_data.scroll.y += input.mouse_delta.y as f32;
                    }
                    true
                } else {
                    false
                }
            } else {
                false
            };
            let research_size = 50.;
            let research_scroll = UIPos {
                x: context.content.size.x / 2. + element_data.scroll.x,
                y: context.content.size.y / 2. + element_data.scroll.y,
            };
            for research in research.list() {
                let research_data = research.data();
                for dependency in &research_data.dependencies {
                    let dependency_data = dependency.data();
                    context.draw_line(
                        UIPos {
                            x: (research_data.x + 0.5) * research_size + research_scroll.x,
                            y: (research_data.y + 0.5) * research_size + research_scroll.y,
                        },
                        UIPos {
                            x: (dependency_data.x + 0.5) * research_size + research_scroll.x,
                            y: (dependency_data.y + 0.5) * research_size + research_scroll.y,
                        },
                        TextureKey::id("crosshair").unwrap().tex_coords(), //todo
                        Color::WHITE,
                        3.,
                    );
                }
            }
            for research in research.list() {
                let research_data = research.data();
                let area = UIRect {
                    pos: UIPos {
                        x: research_data.x * research_size + research_scroll.x,
                        y: research_data.y * research_size + research_scroll.y,
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
                    .contains(input.cursor_position)
                        && mouse_inside
                    {
                        overlay_context.draw_text(
                            input.cursor_position,
                            translate(format!("research.{}", research.text_id()).as_str()),
                            40.,
                            Color::WHITE,
                        );
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
                                    event_consumer(UIMessage::ServerMessage(
                                        NetworkMessageC2S::Research {
                                            slot,
                                            mode,
                                            research: *research,
                                        },
                                    ));
                                }
                            }
                            None => {}
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
                && context.content.contains(input.cursor_position)
            {
                if input.buttons.is_just_down(MouseButton::Left) {
                    event_consumer(UIMessage::ServerMessage(NetworkMessageC2S::UIButtonPress {
                        property: *property,
                        value: *value,
                        modify_mode: *modify_mode,
                    }));
                }
            }
        }
        UIElementType::TextField {} => {
            let element_data = data.element_data.entry(element.uuid).or_default();
            context.draw_text(
                UIPos {
                    x: 0.,
                    y: context.content.size.y,
                },
                &element_data.text,
                20.,
                Color::WHITE,
            );
            if data.time % 1. > 0.5 && element_data.selected {
                let cursor_width = text_renderer().get_size("|", 20.).x;
                context.draw_text(
                    UIPos {
                        x: text_renderer()
                            .get_size(&element_data.text[..element_data.text_cursor], 20.)
                            .x
                            - cursor_width / 2.,
                        y: context.content.size.y,
                    },
                    "|",
                    20.,
                    Color::WHITE,
                );
            }
            if let Some(input) = input {
                if input.buttons.is_just_down(MouseButton::Left) {
                    element_data.selected = context.content.contains(input.cursor_position);
                }
                if element_data.selected {
                    for logical in &input.logical {
                        match logical {
                            winit::keyboard::Key::Named(NamedKey::ArrowLeft) => {
                                if element_data.text_cursor > 0 {
                                    element_data.text_cursor -= 1;
                                }
                            }
                            winit::keyboard::Key::Named(NamedKey::ArrowRight) => {
                                if element_data.text_cursor < element_data.text.len() {
                                    element_data.text_cursor += 1;
                                }
                            }
                            winit::keyboard::Key::Named(NamedKey::Backspace) => {
                                if element_data.text_cursor > 0 {
                                    element_data.text_cursor -= 1;
                                    element_data.text.remove(element_data.text_cursor);
                                }
                            }
                            winit::keyboard::Key::Named(NamedKey::Space) => {
                                element_data.text.insert_str(element_data.text_cursor, " ");
                                element_data.text_cursor += 1;
                            }
                            winit::keyboard::Key::Character(char) => {
                                let Some(char) = char.chars().next() else {
                                    continue;
                                };
                                if !char.is_ascii_alphanumeric() && !char.is_ascii_punctuation() {
                                    continue;
                                }
                                element_data.text.insert(element_data.text_cursor, char);
                                element_data.text_cursor += 1;
                            }
                            _ => {}
                        }
                    }
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
    fn map_point(&self, p: UIPos) -> UIPos {
        UIPos {
            x: (self.content.pos.x + p.x) / self.gui_size as f32 * 2. - self.aspect_ratio,
            y: -((self.content.pos.y + p.y) / self.gui_size as f32 * 2. - 1.),
        }
    }
    pub fn make_clip(&self) -> UIRect {
        UIRect {
            pos: UIPos {
                x: self.content.pos.x / self.gui_size as f32 * 2. - self.aspect_ratio,
                y: -((self.content.pos.y + self.content.size.y) / self.gui_size as f32 * 2. - 1.),
            },
            size: UIPos {
                x: self.content.size.x / self.gui_size as f32 * 2.,
                y: self.content.size.y / self.gui_size as f32 * 2.,
            },
        }
    }
    pub fn draw_quad(&mut self, quad: UIRect, texture: TexCoords, color: Color) {
        self.buffer.add_quad_clip(
            UIRect {
                pos: self.map_point(UIPos {
                    x: quad.pos.x,
                    y: quad.pos.y + quad.size.y,
                }),
                size: UIPos {
                    x: quad.size.x / self.gui_size as f32 * 2.,
                    y: quad.size.y / self.gui_size as f32 * 2.,
                },
            },
            texture,
            color,
            self.make_clip(),
        );
    }
    pub fn draw_line(
        &mut self,
        p1: UIPos,
        p2: UIPos,
        texture: TexCoords,
        color: Color,
        width: f32,
    ) {
        self.buffer.add_line_clip(
            self.map_point(p1),
            self.map_point(p2),
            texture,
            color,
            width / self.gui_size as f32,
            self.make_clip(),
        );
    }
    pub fn draw_text(&mut self, position: UIPos, text: &str, size: f32, color: Color) -> UIPos {
        //todo: clip
        let size = text_renderer().draw(
            self.map_point(position),
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
        mod triclip {
            use crate::UIPos;
            use crate::ui::UIRect;
            use smallvec::SmallVec;

            #[derive(Clone, Copy)]
            pub struct Vertex {
                pub position: UIPos,
                pub uv: UIPos,
            }

            #[inline(always)]
            fn lerp_pos(p1: UIPos, p2: UIPos, t: f32) -> UIPos {
                UIPos {
                    x: p1.x + (p2.x - p1.x) * t,
                    y: p1.y + (p2.y - p1.y) * t,
                }
            }

            #[inline(always)]
            fn lerp_vertex(v1: &Vertex, v2: &Vertex, t: f32) -> Vertex {
                Vertex {
                    position: lerp_pos(v1.position, v2.position, t),
                    uv: lerp_pos(v1.uv, v2.uv, t),
                }
            }

            pub type PolygonBuffer = SmallVec<[Vertex; 12]>; //max should be 7

            fn clip_against_edge<F>(
                input: &[Vertex],
                output: &mut PolygonBuffer,
                inside: F,
                intersect: f32,
                is_horizontal: bool,
            ) where
                F: Fn(&Vertex, f32) -> bool,
            {
                output.clear();

                let Some(mut s) = input.last() else {
                    return;
                };

                for e in input {
                    if inside(e, intersect) {
                        if !inside(s, intersect) {
                            let t = get_t(s, e, intersect, is_horizontal);
                            output.push(lerp_vertex(s, e, t));
                        }
                        output.push(*e);
                    } else if inside(s, intersect) {
                        let t = get_t(s, e, intersect, is_horizontal);
                        output.push(lerp_vertex(s, e, t));
                    }
                    s = e;
                }
            }

            #[inline(always)]
            fn get_t(v1: &Vertex, v2: &Vertex, intersect: f32, is_horizontal: bool) -> f32 {
                let (v1_coord, v2_coord) = if is_horizontal {
                    (v1.position.y, v2.position.y)
                } else {
                    (v1.position.x, v2.position.x)
                };

                let denom = v2_coord - v1_coord;
                if denom.abs() < 1e-6 {
                    0.0
                } else {
                    (intersect - v1_coord) / denom
                }
            }

            pub fn clip_triangle_to_rect(triangle: [Vertex; 3], rect: UIRect) -> PolygonBuffer {
                let mut buf_a: PolygonBuffer = SmallVec::new();
                let mut buf_b: PolygonBuffer = SmallVec::new();

                clip_against_edge(
                    &triangle,
                    &mut buf_b,
                    |v, bound| v.position.x >= bound,
                    rect.pos.x,
                    false,
                );
                clip_against_edge(
                    &buf_b,
                    &mut buf_a,
                    |v, bound| v.position.x <= bound,
                    rect.pos.x + rect.size.x,
                    false,
                );
                clip_against_edge(
                    &buf_a,
                    &mut buf_b,
                    |v, bound| v.position.y >= bound,
                    rect.pos.y,
                    true,
                );
                clip_against_edge(
                    &buf_b,
                    &mut buf_a,
                    |v, bound| v.position.y <= bound,
                    rect.pos.y + rect.size.y,
                    true,
                );
                buf_a
            }
        }
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
            pub clip: UIRect,
        }
        impl MeshVertexConsumer for IconVertexConsumer<'_> {
            fn add_vertex(&mut self, vertex: crate::render::MeshVertex) -> u32 {
                let light_dot = vertex.normal.dot(self.light);
                self.mesh.add_vertex(GUIVertex {
                    position: [vertex.position.x, vertex.position.y],
                    tex_coords: vertex.uv,
                    color: Color::grayscale(((0.5 + light_dot / 2.) * 255.) as u8).into(),
                })
            }
            fn add_index(&mut self, index: u32) {
                self.mesh.add_index(index);
            }
            fn add_quad(&mut self, vertices: [crate::render::MeshVertex; 4]) {
                for indices in [[0, 3, 2], [2, 1, 0]] {
                    let normal = vertices[indices[0]].normal;
                    if normal.x + normal.y + normal.z <= 0. {
                        continue;
                    }
                    let polygon = triclip::clip_triangle_to_rect(
                        indices.map(|indice| {
                            let vertex = &vertices[indice];
                            let position = vertex.position.multiply_point(self.projection);
                            triclip::Vertex {
                                position: UIPos {
                                    x: self.rect.pos.x + (position.x + 1.) / 2. * self.rect.size.x,
                                    y: self.rect.pos.y + (position.y + 1.) / 2. * self.rect.size.y,
                                },
                                uv: UIPos {
                                    x: vertex.uv[0],
                                    y: vertex.uv[1],
                                },
                            }
                        }),
                        self.clip,
                    );
                    if polygon.len() < 3 {
                        continue;
                    }
                    let indices = polygon
                        .iter()
                        .map(|v| {
                            self.add_vertex(MeshVertex {
                                position: Vec3 {
                                    x: v.position.x,
                                    y: v.position.y,
                                    z: 0.,
                                },
                                normal,
                                uv: [v.uv.x, v.uv.y],
                            })
                        })
                        .collect::<SmallVec<[_; 12]>>();
                    for i in 1..(polygon.len() - 1) {
                        self.add_index(indices[0]);
                        self.add_index(indices[i]);
                        self.add_index(indices[i + 1]);
                    }
                }
            }
        }
        let x = (self.content.pos.x + quad.pos.x) / self.gui_size as f32 * 2. - self.aspect_ratio;
        let y = -((self.content.pos.y + quad.pos.y + quad.size.y) / self.gui_size as f32 * 2. - 1.);
        let w = quad.size.x / self.gui_size as f32 * 2.;
        let h = quad.size.y / self.gui_size as f32 * 2.;
        let mut icon_vertex_consumer = IconVertexConsumer {
            clip: self.make_clip(),
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
            taffy.set_node_context(node, Some(element)).unwrap();
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
#[derive(Copy, Clone, Default, Debug)]
pub struct UIPos {
    pub x: f32,
    pub y: f32,
}
impl UIPos {
    pub fn all(v: f32) -> UIPos {
        UIPos { x: v, y: v }
    }
}
#[derive(Copy, Clone, Debug)]
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
