use std::{cmp::Ordering, collections::HashMap, path::Path};

use anyhow::anyhow;
use roxmltree::Node;
use serde::{Deserialize, Serialize};
use taffy::{
    AlignContent, AlignItems, AlignSelf, Dimension, Display, FlexDirection, FlexWrap,
    JustifyContent, LengthPercentage, LengthPercentageAuto, Position, Size,
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
    ItemSlot { slot: usize },
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
            "slot" => UIElementType::ItemSlot {
                slot: node.attribute("id").and_then(|n| n.parse().ok()).unwrap(),
            },
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
pub struct UIStyleList(pub Vec<(UIStyleRule, Option<PropertyCondition>)>);
impl UIStyleList {
    pub fn parse(input: &str) -> anyhow::Result<Self> {
        let mut rules = Vec::new();
        for rule in input.split(";") {
            let rule = rule.trim();
            if rule.is_empty() {
                continue;
            }
            let (rule, value) = rule.split_once(":").unwrap();
            let mut rule = rule.trim();
            let mut condition = None;
            if rule.starts_with("[") {
                let (c, r) = rule.split_once("]").unwrap();
                rule = r.trim();
                let comparator = if c.contains("<") {
                    Ordering::Less
                } else if c.contains(">") {
                    Ordering::Greater
                } else if c.contains("=") {
                    Ordering::Equal
                } else {
                    panic!()
                };
                let (l, r) = c
                    .split_once(match comparator {
                        Ordering::Less => "<",
                        Ordering::Equal => "=",
                        Ordering::Greater => ">",
                    })
                    .unwrap();
                condition = Some(PropertyCondition {
                    property: l[1..].trim().to_string(),
                    comparator,
                    value: r.parse::<f32>().unwrap(),
                });
            }
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
                    "flex-end" => AlignContent::FlexEnd,
                    "flex-start" => AlignContent::FlexStart,
                    "space-around" => AlignContent::SpaceAround,
                    "space-between" => AlignContent::SpaceBetween,
                    "space-evenly" => AlignContent::SpaceEvenly,
                    "start" => AlignContent::Start,
                    "stretch" => AlignContent::Stretch,
                    _ => panic!(),
                })
            }
            fn parse_align_items(input: &str) -> anyhow::Result<AlignItems> {
                Ok(match input {
                    "center" => AlignItems::Center,
                    "end" => AlignItems::End,
                    "flex-end" => AlignItems::FlexEnd,
                    "flex-start" => AlignItems::FlexStart,
                    "start" => AlignItems::Start,
                    "stretch" => AlignItems::Stretch,
                    "baseline" => AlignItems::Baseline,
                    _ => panic!(),
                })
            }
            match rule {
                "flex-direction" => rules.push((
                    UIStyleRule::FlexDirection(match value {
                        "row" => FlexDirection::Row,
                        "row-reverse" => FlexDirection::RowReverse,
                        "column" => FlexDirection::Column,
                        "column-reverse" => FlexDirection::ColumnReverse,
                        _ => unimplemented!(),
                    }),
                    condition,
                )),
                "justify-content" => rules.push((
                    UIStyleRule::JustifyContent(parse_align_content(value)?),
                    condition,
                )),
                "justify-items" => rules.push((
                    UIStyleRule::JustifyItems(parse_align_items(value)?),
                    condition,
                )),
                "justify-self" => rules.push((
                    UIStyleRule::JustifySelf(parse_align_items(value)?),
                    condition,
                )),
                "align-content" => rules.push((
                    UIStyleRule::AlignContent(parse_align_content(value)?),
                    condition,
                )),
                "align-items" => rules.push((
                    UIStyleRule::AlignItems(parse_align_items(value)?),
                    condition,
                )),
                "align-self" => {
                    rules.push((UIStyleRule::AlignSelf(parse_align_items(value)?), condition))
                }
                "size" => {
                    let (width, height) = value.split_once(" ").unwrap();
                    rules.push((
                        UIStyleRule::Width(parse_style_length(width)?),
                        condition.clone(),
                    ));
                    rules.push((UIStyleRule::Height(parse_style_length(height)?), condition));
                }
                "width" => {
                    rules.push((UIStyleRule::Width(parse_style_length(value)?), condition));
                }
                "height" => {
                    rules.push((UIStyleRule::Height(parse_style_length(value)?), condition));
                }
                "padding" => {
                    let length = parse_style_length(value)?;
                    rules.push((UIStyleRule::PaddingTop(length), condition.clone()));
                    rules.push((UIStyleRule::PaddingBottom(length), condition.clone()));
                    rules.push((UIStyleRule::PaddingLeft(length), condition.clone()));
                    rules.push((UIStyleRule::PaddingRight(length), condition));
                }
                "padding-left" => rules.push((
                    UIStyleRule::PaddingLeft(parse_style_length(value)?),
                    condition,
                )),
                "padding-right" => rules.push((
                    UIStyleRule::PaddingRight(parse_style_length(value)?),
                    condition,
                )),
                "padding-top" => rules.push((
                    UIStyleRule::PaddingTop(parse_style_length(value)?),
                    condition,
                )),
                "padding-bottom" => rules.push((
                    UIStyleRule::PaddingBottom(parse_style_length(value)?),
                    condition,
                )),

                "margin" => {
                    let length = parse_style_length(value)?;
                    rules.push((UIStyleRule::MarginTop(length), condition.clone()));
                    rules.push((UIStyleRule::MarginBottom(length), condition.clone()));
                    rules.push((UIStyleRule::MarginLeft(length), condition.clone()));
                    rules.push((UIStyleRule::MarginRight(length), condition));
                }
                "margin-left" => rules.push((
                    UIStyleRule::MarginLeft(parse_style_length(value)?),
                    condition,
                )),
                "margin-right" => rules.push((
                    UIStyleRule::MarginRight(parse_style_length(value)?),
                    condition,
                )),
                "margin-top" => rules.push((
                    UIStyleRule::MarginTop(parse_style_length(value)?),
                    condition,
                )),
                "margin-bottom" => rules.push((
                    UIStyleRule::MarginBottom(parse_style_length(value)?),
                    condition,
                )),
                "position" => rules.push((
                    UIStyleRule::Position(match value {
                        "relative" => Position::Relative,
                        "absolute" => Position::Absolute,
                        _ => panic!(),
                    }),
                    condition,
                )),
                "inset" => {
                    let length = parse_style_length(value)?;
                    rules.push((UIStyleRule::InsetTop(length), condition.clone()));
                    rules.push((UIStyleRule::InsetBottom(length), condition.clone()));
                    rules.push((UIStyleRule::InsetLeft(length), condition.clone()));
                    rules.push((UIStyleRule::InsetRight(length), condition));
                }
                "inset-left" => rules.push((
                    UIStyleRule::InsetLeft(parse_style_length(value)?),
                    condition,
                )),
                "inset-right" => rules.push((
                    UIStyleRule::InsetRight(parse_style_length(value)?),
                    condition,
                )),
                "inset-top" => {
                    rules.push((UIStyleRule::InsetTop(parse_style_length(value)?), condition))
                }
                "inset-bottom" => rules.push((
                    UIStyleRule::InsetBottom(parse_style_length(value)?),
                    condition,
                )),
                "font-size" => {
                    rules.push((
                        UIStyleRule::FontSize(value.parse::<f32>().unwrap()),
                        condition,
                    ));
                }
                "background" => {
                    rules.push((
                        UIStyleRule::Background(TextureKey::id(value).unwrap()),
                        condition,
                    ));
                }
                "flex-wrap" => {
                    rules.push((
                        UIStyleRule::FlexWrap(match value {
                            "no-wrap" => FlexWrap::NoWrap,
                            "wrap" => FlexWrap::Wrap,
                            "wrap-reverse" => FlexWrap::WrapReverse,
                            _ => panic!(),
                        }),
                        condition,
                    ));
                }
                "gap-column" => {
                    rules.push((
                        UIStyleRule::GapColumn(parse_style_length(value)?),
                        condition,
                    ));
                }
                "gap-row" => {
                    rules.push((UIStyleRule::GapRow(parse_style_length(value)?), condition));
                }
                "display" => {
                    rules.push((
                        UIStyleRule::Display(match value {
                            "block" => Display::Block,
                            "flex" => Display::Flex,
                            "grid" => Display::Grid,
                            "none" => Display::None,
                            _ => panic!(),
                        }),
                        condition,
                    ));
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
    Display(Display),
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

#[derive(Clone, Deserialize, Serialize)]
pub struct PropertyMap(pub HashMap<String, f32>);
#[derive(Clone, Debug)]
pub struct PropertyCondition {
    pub property: String,
    pub comparator: std::cmp::Ordering,
    pub value: f32,
}
impl PropertyCondition {
    pub fn satisfies(&self, map: &PropertyMap) -> bool {
        let property_value = map.0.get(&self.property).cloned().unwrap_or(0.);
        match self.comparator {
            std::cmp::Ordering::Less => property_value < self.value,
            std::cmp::Ordering::Equal => property_value == self.value,
            std::cmp::Ordering::Greater => property_value > self.value,
        }
    }
}
