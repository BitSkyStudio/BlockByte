use std::path::Path;

use anyhow::anyhow;
use roxmltree::Node;
use taffy::{
    AlignContent, AlignItems, AlignSelf, Dimension, FlexDirection, FlexWrap, JustifyContent,
    LengthPercentage, LengthPercentageAuto, Position, Size,
};

use crate::registry::{Key, RegistryConfigLoadable, TextureKey};

pub struct UIScreen {
    pub root: UIElement,
}
pub type UIScreenKey = Key<UIScreen>;
impl RegistryConfigLoadable for UIScreen {
    fn registry_load_from_config(config: &Path) -> anyhow::Result<Self> {
        let input = std::fs::read_to_string(config).unwrap();
        let doc = roxmltree::Document::parse(&input)?;
        Ok(UIScreen {
            root: UIElement::parse(&doc.root_element())?,
        })
    }
}
pub enum UIElementType {
    Box(Vec<UIElement>),
    Label(String),
    Image(TextureKey, f32, f32),
    ItemSlot(usize),
}
impl UIElementType {
    pub fn parse(node: &Node) -> anyhow::Result<Self> {
        Ok(match node.tag_name().name() {
            "box" => UIElementType::Box(
                node.children()
                    .filter_map(|child| {
                        if child.is_text() {
                            return None;
                        }
                        Some(UIElement::parse(&child).unwrap())
                    })
                    .collect(),
            ),
            "label" => UIElementType::Label(node.text().unwrap().to_string()),
            "image" => UIElementType::Image(
                TextureKey::id(node.attribute("texture").unwrap()).unwrap(),
                node.attribute("width")
                    .and_then(|n| n.parse().ok())
                    .unwrap(),
                node.attribute("height")
                    .and_then(|n| n.parse().ok())
                    .unwrap(),
            ),
            "slot" => {
                UIElementType::ItemSlot(node.attribute("id").and_then(|n| n.parse().ok()).unwrap())
            }
            other => unimplemented!("{}", other),
        })
    }
}
pub struct UIElement {
    pub element_type: UIElementType,
    pub style: UIStyleList,
    pub style_classes: Vec<UIStyleListKey>,
}
impl UIElement {
    pub fn parse(node: &Node) -> anyhow::Result<Self> {
        Ok(UIElement {
            element_type: UIElementType::parse(node)?,
            style: match node.attribute("style") {
                Some(style) => UIStyleList::parse(style)?,
                None => UIStyleList::default(),
            },
            style_classes: match node.attribute("class") {
                Some(classes) => classes
                    .split(" ")
                    .map(|class| UIStyleListKey::id(class).unwrap())
                    .collect(),
                None => Vec::new(),
            },
        })
    }
}
#[derive(Default)]
pub struct UIStyleList(pub Vec<UIStyleRule>);
impl UIStyleList {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let mut rules = Vec::new();
        for rule in input.split(";") {
            let rule = rule.trim();
            if rule.is_empty() {
                continue;
            }
            let (rule, value) = rule.split_once(":").unwrap();
            let rule = rule.trim();
            let value = value.trim();
            fn parse_style_length(input: &str) -> anyhow::Result<StyleLength> {
                if input == "auto" {
                    Ok(StyleLength::Auto)
                } else if input.ends_with("%") {
                    Ok(StyleLength::Percent(
                        input[0..input.len() - 1].parse::<f32>().unwrap() / 100.,
                    ))
                } else {
                    Ok(StyleLength::Length(input.parse::<f32>().unwrap()))
                }
            }
            fn parse_align_content(input: &str) -> anyhow::Result<AlignContent> {
                Ok(match input {
                    "center" => AlignContent::Center,
                    "end" => AlignContent::End,
                    "flex_end" => AlignContent::FlexEnd,
                    "flex_start" => AlignContent::FlexStart,
                    "space_around" => AlignContent::SpaceAround,
                    "space_between" => AlignContent::SpaceBetween,
                    "space_evenly" => AlignContent::SpaceEvenly,
                    "start" => AlignContent::Start,
                    "stretch" => AlignContent::Stretch,
                    _ => panic!(),
                })
            }
            fn parse_align_items(input: &str) -> anyhow::Result<AlignItems> {
                Ok(match input {
                    "center" => AlignItems::Center,
                    "end" => AlignItems::End,
                    "flex_end" => AlignItems::FlexEnd,
                    "flex_start" => AlignItems::FlexStart,
                    "start" => AlignItems::Start,
                    "stretch" => AlignItems::Stretch,
                    "baseline" => AlignItems::Baseline,
                    _ => panic!(),
                })
            }
            match rule {
                "flex_direction" => rules.push(UIStyleRule::FlexDirection(match value {
                    "row" => FlexDirection::Row,
                    "row_reverse" => FlexDirection::RowReverse,
                    "column" => FlexDirection::Column,
                    "column_reverse" => FlexDirection::ColumnReverse,
                    _ => unimplemented!(),
                })),
                "justify_content" => {
                    rules.push(UIStyleRule::JustifyContent(parse_align_content(value)?))
                }
                "justify_items" => rules.push(UIStyleRule::JustifyItems(parse_align_items(value)?)),
                "justify_self" => rules.push(UIStyleRule::JustifySelf(parse_align_items(value)?)),
                "align_content" => {
                    rules.push(UIStyleRule::AlignContent(parse_align_content(value)?))
                }
                "align_items" => rules.push(UIStyleRule::AlignItems(parse_align_items(value)?)),
                "align_self" => rules.push(UIStyleRule::AlignSelf(parse_align_items(value)?)),
                "size" => {
                    let (width, height) = value.split_once(" ").unwrap();
                    rules.push(UIStyleRule::Width(parse_style_length(width)?));
                    rules.push(UIStyleRule::Height(parse_style_length(height)?));
                }
                "width" => {
                    rules.push(UIStyleRule::Width(parse_style_length(value)?));
                }
                "height" => {
                    rules.push(UIStyleRule::Height(parse_style_length(value)?));
                }
                "padding" => {
                    let length = parse_style_length(value)?;
                    rules.push(UIStyleRule::PaddingTop(length));
                    rules.push(UIStyleRule::PaddingBottom(length));
                    rules.push(UIStyleRule::PaddingLeft(length));
                    rules.push(UIStyleRule::PaddingRight(length));
                }
                "padding_left" => rules.push(UIStyleRule::PaddingLeft(parse_style_length(value)?)),
                "padding_right" => {
                    rules.push(UIStyleRule::PaddingRight(parse_style_length(value)?))
                }
                "padding_top" => rules.push(UIStyleRule::PaddingTop(parse_style_length(value)?)),
                "padding_bottom" => {
                    rules.push(UIStyleRule::PaddingBottom(parse_style_length(value)?))
                }

                "margin" => {
                    let length = parse_style_length(value)?;
                    rules.push(UIStyleRule::MarginTop(length));
                    rules.push(UIStyleRule::MarginBottom(length));
                    rules.push(UIStyleRule::MarginLeft(length));
                    rules.push(UIStyleRule::MarginRight(length));
                }
                "margin_left" => rules.push(UIStyleRule::MarginLeft(parse_style_length(value)?)),
                "margin_right" => rules.push(UIStyleRule::MarginRight(parse_style_length(value)?)),
                "margin_top" => rules.push(UIStyleRule::MarginTop(parse_style_length(value)?)),
                "margin_bottom" => {
                    rules.push(UIStyleRule::MarginBottom(parse_style_length(value)?))
                }
                "position" => rules.push(UIStyleRule::Position(match value {
                    "relative" => Position::Relative,
                    "absolute" => Position::Absolute,
                    _ => panic!(),
                })),
                "inset" => {
                    let length = parse_style_length(value)?;
                    rules.push(UIStyleRule::InsetTop(length));
                    rules.push(UIStyleRule::InsetBottom(length));
                    rules.push(UIStyleRule::InsetLeft(length));
                    rules.push(UIStyleRule::InsetRight(length));
                }
                "inset_left" => rules.push(UIStyleRule::InsetLeft(parse_style_length(value)?)),
                "inset_right" => rules.push(UIStyleRule::InsetRight(parse_style_length(value)?)),
                "inset_top" => rules.push(UIStyleRule::InsetTop(parse_style_length(value)?)),
                "inset_bottom" => rules.push(UIStyleRule::InsetBottom(parse_style_length(value)?)),
                "font_size" => {
                    rules.push(UIStyleRule::FontSize(value.parse::<f32>().unwrap()));
                }
                "background" => {
                    rules.push(UIStyleRule::Background(TextureKey::id(value).unwrap()));
                }
                "flex_wrap" => {
                    rules.push(UIStyleRule::FlexWrap(match value {
                        "no_wrap" => FlexWrap::NoWrap,
                        "wrap" => FlexWrap::Wrap,
                        "wrap_reverse" => FlexWrap::WrapReverse,
                        _ => panic!(),
                    }));
                }
                "gap_column" => {
                    rules.push(UIStyleRule::GapColumn(parse_style_length(value)?));
                }
                "gap_row" => {
                    rules.push(UIStyleRule::GapRow(parse_style_length(value)?));
                }
                rule => return Err(anyhow!("unknown style rule '{}'", rule)),
            }
        }
        Ok(UIStyleList(rules))
    }
}
impl RegistryConfigLoadable for UIStyleList {
    fn registry_load_from_config(config: &Path) -> anyhow::Result<Self> {
        Self::parse(&std::fs::read_to_string(config).unwrap())
    }
}
pub type UIStyleListKey = Key<UIStyleList>;
pub enum UIStyleRule {
    FlexDirection(FlexDirection),
    AlignItems(AlignItems),
    AlignContent(AlignContent),
    AlignSelf(AlignItems),
    JustifyItems(AlignItems),
    JustifyContent(AlignContent),
    JustifySelf(AlignItems),
    Position(Position),
    InsetLeft(StyleLength),
    InsetRight(StyleLength),
    InsetTop(StyleLength),
    InsetBottom(StyleLength),
    Width(StyleLength),
    Height(StyleLength),
    PaddingLeft(StyleLength),
    PaddingRight(StyleLength),
    PaddingTop(StyleLength),
    PaddingBottom(StyleLength),
    MarginLeft(StyleLength),
    MarginRight(StyleLength),
    MarginTop(StyleLength),
    MarginBottom(StyleLength),
    FontSize(f32),
    Background(TextureKey),
    FlexWrap(FlexWrap),
    GapColumn(StyleLength),
    GapRow(StyleLength),
}
#[derive(Clone, Copy)]
pub enum StyleLength {
    Length(f32),
    Percent(f32),
    Auto,
}
impl Into<LengthPercentage> for StyleLength {
    fn into(self) -> LengthPercentage {
        match self {
            StyleLength::Length(value) => LengthPercentage::length(value),
            StyleLength::Percent(value) => LengthPercentage::percent(value),
            StyleLength::Auto => LengthPercentage::length(0.),
        }
    }
}
impl Into<LengthPercentageAuto> for StyleLength {
    fn into(self) -> LengthPercentageAuto {
        match self {
            StyleLength::Length(value) => LengthPercentageAuto::length(value),
            StyleLength::Percent(value) => LengthPercentageAuto::percent(value),
            StyleLength::Auto => LengthPercentageAuto::auto(),
        }
    }
}
impl Into<Dimension> for StyleLength {
    fn into(self) -> Dimension {
        match self {
            StyleLength::Length(value) => Dimension::length(value),
            StyleLength::Percent(value) => Dimension::percent(value),
            StyleLength::Auto => Dimension::auto(),
        }
    }
}
