use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use anyhow::anyhow;
use roxmltree::Node;
use serde::{Deserialize, Serialize};
use taffy::{
    AlignContent, AlignItems, Dimension, Display, FlexDirection, FlexWrap, LengthPercentage,
    LengthPercentageAuto, Position,
};
use uuid::Uuid;

use crate::{
    InternString,
    net::PropertyModifyMode,
    registry::{Key, KeyGroup, RecipeData, RegistryConfigLoadable, ResearchData, TextureKey},
    scripts::ScriptValue,
};
#[derive(Default)]
struct UIParseContext {
    display_properties: HashSet<InternString>,
    button_properties: HashSet<InternString>,
}
pub struct UIScreen {
    pub root: UIElement,
    pub display_properties: HashSet<InternString>,
    pub button_properties: HashSet<InternString>,
}
pub type UIScreenKey = Key<UIScreen>;
impl RegistryConfigLoadable for UIScreen {
    fn registry_load_from_config(config: &Vec<PathBuf>, _key: Key<Self>) -> anyhow::Result<Self> {
        let input = std::fs::read_to_string(config.last().unwrap()).unwrap();
        let doc = roxmltree::Document::parse(&input)?;
        let mut context = UIParseContext::default();
        Ok(UIScreen {
            root: UIElement::parse(&doc.root_element(), &mut context)?,
            button_properties: context.button_properties,
            display_properties: context.display_properties,
        })
    }
}
#[derive(Clone, Copy)]
pub enum SlotId {
    Id(usize),
    Trash,
}
pub enum CraftAreaRecipes {
    Recipes(KeyGroup<RecipeData>),
    CheatMenu,
}
pub enum UIElementType {
    Box(Vec<UIElement>),
    Label(String),
    ItemSlot {
        slot: SlotId,
    },
    CraftArea {
        recipes: CraftAreaRecipes,
    },
    ResearchTree {
        research: KeyGroup<ResearchData>,
    },
    Button {
        text: String,
        property: String,
        value: ScriptValue,
        modify_mode: PropertyModifyMode,
    },
}
impl UIElementType {
    fn parse(node: &Node, context: &mut UIParseContext) -> anyhow::Result<Self> {
        Ok(match node.tag_name().name() {
            "box" => UIElementType::Box(
                node.children()
                    .filter_map(|child| {
                        if child.is_text() {
                            return None;
                        }
                        Some(UIElement::parse(&child, context))
                    })
                    .collect::<anyhow::Result<Vec<UIElement>>>()?,
            ),
            "label" => UIElementType::Label({
                let text = node.text().unwrap().to_string();
                PropertyMap::extract_patched_properties_from_text(
                    text.as_str(),
                    &mut context.display_properties,
                );
                text
            }),
            "slot" => UIElementType::ItemSlot {
                slot: {
                    let attribute = node.attribute("id").unwrap();
                    match attribute {
                        "trash" => SlotId::Trash,
                        id => SlotId::Id(id.parse().unwrap()),
                    }
                },
            },
            "craft" => UIElementType::CraftArea {
                recipes: match node.attribute("recipes").unwrap() {
                    "cheat_menu" => CraftAreaRecipes::CheatMenu,
                    recipe_type => CraftAreaRecipes::Recipes(KeyGroup::parse(recipe_type).unwrap()),
                },
            },
            "research" => UIElementType::ResearchTree {
                research: KeyGroup::parse(node.attribute("research").unwrap()).unwrap(),
            },
            "buttton" => UIElementType::Button {
                text: {
                    let text = node.text().unwrap().to_string();
                    PropertyMap::extract_patched_properties_from_text(
                        text.as_str(),
                        &mut context.display_properties,
                    );
                    text
                },
                property: {
                    let property = node.attribute("property").unwrap().to_string();
                    context
                        .button_properties
                        .insert(InternString::intern(&property.clone()));
                    property
                },
                value: node
                    .attribute("value")
                    .unwrap()
                    .parse::<i16>()
                    .unwrap()
                    .cast_unsigned(),
                modify_mode: match node.attribute("mode").unwrap() {
                    "add" => PropertyModifyMode::Add,
                    "set" => PropertyModifyMode::Set,
                    _ => panic!(),
                },
            },
            other => unimplemented!("{}", other),
        })
    }
}
pub struct UIElement {
    pub element_type: UIElementType,
    pub style: UIStyleList,
    pub style_classes: Vec<UIStyleListKey>,
    pub uuid: Uuid,
}
impl UIElement {
    fn parse(node: &Node, context: &mut UIParseContext) -> anyhow::Result<Self> {
        Ok(UIElement {
            element_type: UIElementType::parse(node, context)?,
            style: match node.attribute("style") {
                Some(style) => {
                    let style_list = UIStyleList::parse(style)?;
                    context
                        .display_properties
                        .extend(style_list.properties.iter().cloned());
                    style_list
                }
                None => UIStyleList::default(),
            },
            style_classes: match node.attribute("class") {
                Some(classes) => classes
                    .split(" ")
                    .map(|class| class.trim())
                    .filter(|class| !class.is_empty())
                    .map(|class| UIStyleListKey::id(class).unwrap()) //todo: add properties to context
                    .collect(),
                None => Vec::new(),
            },
            uuid: Uuid::new_v4(),
        })
    }
}
#[derive(Default)]
pub struct UIStyleList {
    pub rules: Vec<(UIStyleRule, Option<PropertyCondition>)>,
    pub properties: HashSet<InternString>,
}
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
                    property: InternString::intern(l[1..].trim()),
                    comparator,
                    value: r.parse::<f32>().unwrap(),
                });
            }
            let value = value.trim();
            fn parse_style_value(input: &str) -> anyhow::Result<StyleValue> {
                Ok(match input.parse::<f32>() {
                    Ok(value) => StyleValue::Constant(value),
                    Err(_) => StyleValue::Property(InternString::intern(input)),
                })
            }
            fn parse_style_length(input: &str) -> anyhow::Result<StyleLength> {
                if input == "auto" {
                    Ok(StyleLength::Auto)
                } else if input.ends_with("%") {
                    Ok(StyleLength::Percent(parse_style_value(
                        &input[0..input.len() - 1],
                    )?))
                } else {
                    Ok(StyleLength::Length(parse_style_value(input)?))
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
                    rules.push((UIStyleRule::PaddingTop(length.clone()), condition.clone()));
                    rules.push((
                        UIStyleRule::PaddingBottom(length.clone()),
                        condition.clone(),
                    ));
                    rules.push((UIStyleRule::PaddingLeft(length.clone()), condition.clone()));
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
                    rules.push((UIStyleRule::MarginTop(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::MarginBottom(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::MarginLeft(length.clone()), condition.clone()));
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
                "border" => {
                    let length = parse_style_length(value)?;
                    rules.push((UIStyleRule::BorderTop(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::BorderBottom(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::BorderLeft(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::BorderRight(length), condition));
                }
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
                    rules.push((UIStyleRule::InsetTop(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::InsetBottom(length.clone()), condition.clone()));
                    rules.push((UIStyleRule::InsetLeft(length.clone()), condition.clone()));
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
                    rules.push((UIStyleRule::FontSize(parse_style_value(value)?), condition));
                }
                "background" => {
                    rules.push((
                        UIStyleRule::Background(StretchTexture::parse(value)?),
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
        Ok(UIStyleList {
            rules,
            properties: HashSet::new(),
        })
    }
}
impl RegistryConfigLoadable for UIStyleList {
    fn registry_load_from_config(config: &Vec<PathBuf>, _key: Key<Self>) -> anyhow::Result<Self> {
        Self::parse(&std::fs::read_to_string(config.last().unwrap()).unwrap())
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
    BorderLeft(StyleLength),
    BorderRight(StyleLength),
    BorderTop(StyleLength),
    BorderBottom(StyleLength),
    FontSize(StyleValue),
    Background(StretchTexture),
    FlexWrap(FlexWrap),
    GapColumn(StyleLength),
    GapRow(StyleLength),
    Display(Display),
}
#[derive(Clone)]
pub enum StyleValue {
    Constant(f32),
    Property(InternString),
}
impl StyleValue {
    pub fn calc(&self, properties: &PropertyMap) -> f32 {
        match self {
            StyleValue::Constant(value) => *value,
            StyleValue::Property(property) => *properties.0.get(property).unwrap_or(&0.),
        }
    }
}
#[derive(Clone)]
pub enum StyleLength {
    Length(StyleValue),
    Percent(StyleValue),
    Auto,
}
impl StyleLength {
    pub fn as_length_percentage(&self, properties: &PropertyMap) -> LengthPercentage {
        match self {
            StyleLength::Length(value) => LengthPercentage::length(value.calc(properties)),
            StyleLength::Percent(value) => LengthPercentage::percent(value.calc(properties) / 100.),
            StyleLength::Auto => LengthPercentage::length(0.),
        }
    }
    pub fn as_length_percentage_auto(&self, properties: &PropertyMap) -> LengthPercentageAuto {
        match self {
            StyleLength::Length(value) => LengthPercentageAuto::length(value.calc(properties)),
            StyleLength::Percent(value) => {
                LengthPercentageAuto::percent(value.calc(properties) / 100.)
            }
            StyleLength::Auto => LengthPercentageAuto::auto(),
        }
    }
    pub fn as_dimension(&self, properties: &PropertyMap) -> Dimension {
        match self {
            StyleLength::Length(value) => Dimension::length(value.calc(properties)),
            StyleLength::Percent(value) => Dimension::percent(value.calc(properties) / 100.),
            StyleLength::Auto => Dimension::auto(),
        }
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct PropertyMap(pub HashMap<InternString, f32>);
impl PropertyMap {
    pub fn extract_patched_properties_from_text(
        text: &str,
        properties: &mut HashSet<InternString>,
    ) {
        let mut current_property = None;
        for char in text.chars() {
            if char == '{' {
                current_property = Some(String::new());
            } else if char == '}' {
                properties.insert(InternString::intern(&current_property.take().unwrap()));
            } else if let Some(current_property) = &mut current_property {
                current_property.push(char);
            }
        }
    }
    pub fn patch_text(&self, text: &str) -> String {
        let mut output_text = String::new();
        let mut current_property = None;
        for char in text.chars() {
            if char == '{' {
                current_property = Some(String::new());
            } else if char == '}' {
                output_text += self
                    .0
                    .get(&InternString::intern(current_property.as_ref().unwrap()))
                    .cloned()
                    .unwrap_or(0.)
                    .to_string()
                    .as_str();
            } else if let Some(current_property) = &mut current_property {
                current_property.push(char);
            } else {
                output_text.push(char);
            }
        }
        output_text
    }
}
#[derive(Clone)]
pub struct PropertyCondition {
    pub property: InternString,
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
#[derive(Copy, Clone)]
pub struct StretchTexture {
    pub texture: TextureKey,
    pub border: u32,
}
impl StretchTexture {
    pub fn parse(text: &str) -> anyhow::Result<Self> {
        let (texture, border) = match text.split_once(",") {
            Some((texture, border)) => (
                texture,
                border
                    .parse::<u32>()
                    .map_err(|_| anyhow!("expected number for border"))?,
            ),
            None => (text, 0),
        };
        Ok(StretchTexture {
            texture: TextureKey::id(texture)
                .ok_or_else(|| anyhow!("texture {} not found", texture))?,
            border,
        })
    }
}
