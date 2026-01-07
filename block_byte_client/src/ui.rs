use block_byte_common::{
    coord::Pos,
    registry::TextureKey,
    ui::{StyleLength, UIElement, UIElementType, UIScreen, UIStyleRule},
};
use taffy::{
    AlignItems, AvailableSpace, Dimension, FlexDirection, JustifyContent, Layout, LengthPercentage,
    LengthPercentageAuto, NodeId, Rect, Style, TaffyTree, prelude::TaffyZero,
};
use winit::dpi::PhysicalSize;

use crate::{GUIMesh, TexCoordsExt, text_renderer};

pub fn render_screen(screen: &UIScreen, size: PhysicalSize<u32>, mesh: &mut GUIMesh) {
    let mut taffy: TaffyTree<&UIElement> = TaffyTree::new();
    let root = add_element_to_taffy(&screen.root, &mut taffy);
    taffy
        .compute_layout_with_measure(
            root,
            taffy::Size {
                width: AvailableSpace::Definite(size.width as f32),
                height: AvailableSpace::Definite(size.height as f32),
            },
            |_known_dimensions, _available_space, _node_id, node_context, _style| {
                measure_element(node_context.unwrap())
            },
        )
        .unwrap();
    render_element(
        root,
        &taffy,
        size,
        Pos {
            x: 0.,
            y: 0.,
            z: 0.,
        },
        mesh,
    );
}
pub fn measure_element(element: &UIElement) -> taffy::Size<f32> {
    let style = get_element_style(element);
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
    }
}
fn render_element(
    node: NodeId,
    taffy: &TaffyTree<&UIElement>,
    size: PhysicalSize<u32>,
    parent_offset: Pos,
    mesh: &mut GUIMesh,
) {
    let layout = taffy.layout(node).unwrap();
    let element = *taffy.get_node_context(node).unwrap();
    let aspect_ratio = size.width as f32 / size.height as f32;
    let style = get_element_style(element);
    if let Some(background) = style.background {
        let width = layout.size.width - layout.border.left - layout.border.right;
        let height = layout.size.height - layout.border.top - layout.border.bottom;

        mesh.add_quad(
            Pos {
                x: (layout.location.x + layout.border.left + parent_offset.x) / size.height as f32
                    * 2.
                    - aspect_ratio,
                y: -((layout.location.y + layout.border.top + parent_offset.y + height)
                    / size.height as f32
                    * 2.
                    - 1.),
                z: 0.,
            },
            Pos {
                x: width / size.height as f32 * 2.,
                y: height / size.height as f32 * 2.,
                z: 0.,
            },
            background.tex_coords(),
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
                    mesh,
                );
            }
        }
        UIElementType::Label(text) => {
            text_renderer().draw(
                Pos {
                    x: (layout.content_box_x() + parent_offset.x) / size.height as f32 * 2.
                        - aspect_ratio,
                    y: -((layout.content_box_y() + parent_offset.y + layout.content_box_height())
                        / size.height as f32
                        * 2.
                        - 1.),
                    z: 0.,
                },
                &text,
                style.font_size / size.height as f32 * 2.,
                mesh,
            );
        }
        UIElementType::Image(key, width, height) => {
            mesh.add_quad(
                Pos {
                    x: (layout.content_box_x() + parent_offset.x) / size.height as f32 * 2.
                        - aspect_ratio,
                    y: -((layout.content_box_y() + parent_offset.y + layout.content_box_height())
                        / size.height as f32
                        * 2.
                        - 1.),
                    z: 0.,
                },
                Pos {
                    x: layout.content_box_width() / size.height as f32 * 2.,
                    y: layout.content_box_height() / size.height as f32 * 2.,
                    z: 0.,
                },
                key.tex_coords(),
            );
        }
    }
}
fn add_element_to_taffy<'a>(
    element: &'a UIElement,
    taffy: &mut TaffyTree<&'a UIElement>,
) -> NodeId {
    let style = get_element_style(element);
    match &element.element_type {
        UIElementType::Box(children) => {
            let children = children
                .iter()
                .map(|child| add_element_to_taffy(child, taffy))
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
fn get_element_style(element: &UIElement) -> BBStyle {
    //todo: this should be precomputed
    let mut style = BBStyle::default();
    for style_list in &element.style_classes {
        for rule in &style_list.data().0 {
            style.patch(rule);
        }
    }
    for rule in &element.style.0 {
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
        }
    }
}
