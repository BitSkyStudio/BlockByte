use std::{collections::HashMap, marker::PhantomData};

use num_integer::Integer;
use serde::{Deserialize, Serialize};

pub type RegisterId = usize;
pub type ScriptLabel = usize;
pub type ScriptValue = u16;

#[derive(Copy, Clone)]
pub enum JumpCondition {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}
pub enum RegisterOrImmediate {
    Register(RegisterId),
    Immediate(ScriptValue),
}
#[derive(Copy, Clone)]
pub enum Operation {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Or,
    Xor,
    And,
    Not,
    Bsl,
    Bsr,
}

pub enum ScriptByteCode<E> {
    Move {
        from: RegisterOrImmediate,
        to: RegisterId,
    },
    Jump {
        label: ScriptLabel,
    },
    JumpConditional {
        label: ScriptLabel,
        a: RegisterId,
        b: RegisterOrImmediate,
        condition: JumpCondition,
    },
    Operation {
        a: RegisterId,
        b: RegisterOrImmediate,
        target: RegisterId,
        operation: Operation,
    },
    External(E),
}
pub struct CompiledScript<E> {
    pub instructions: Vec<ScriptByteCode<E>>,
    pub named_registers: Vec<String>,
}
pub fn expect_argument_count(
    line_num: usize,
    arguments: &[&str],
    expect: usize,
) -> Result<(), ScriptParseError<'static>> {
    let arg_count = arguments.len();
    if arg_count != expect {
        Err(ScriptParseError::IllegalArguments {
            line: line_num,
            expected_count: expect,
            actual_count: arg_count,
        })
    } else {
        Ok(())
    }
}
pub trait ExternalScriptByteCode: Sized {
    fn parse<'a>(
        opcode: &'a str,
        arguments: &[&'a str],
        parse_context: &mut ScriptParseContext,
    ) -> Result<Self, ScriptParseError<'a>>;
}
impl<E: ExternalScriptByteCode> CompiledScript<E> {
    pub fn parse<'a>(input: &'a str) -> Result<Self, ScriptParseError<'a>> {
        let mut instructions = Vec::new();
        let mut parse_context = ScriptParseContext::default();
        for (line_num, line) in input.lines().enumerate() {
            let line = match line.split_once("#") {
                Some((line, comment)) => line,
                None => line,
            };
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if line.ends_with(":") {
                if parse_context
                    .labels
                    .insert(line[..line.len() - 1].to_string(), instructions.len())
                    .is_some()
                {
                    return Err(ScriptParseError::DuplicateLabel { line: line_num });
                }
            } else {
                instructions.push((line_num, line));
            }
        }
        let instructions: Vec<ScriptByteCode<E>> = instructions
            .into_iter()
            .map(|(line_num, line)| {
                parse_context.current_line_num = line_num;
                let parts: Vec<_> = line.split(" ").filter(|part| !part.is_empty()).collect();
                let arguments = &parts[1..];
                Ok(match parts[0] {
                    "move" => {
                        expect_argument_count(line_num, arguments, 2)?;
                        ScriptByteCode::Move {
                            from: parse_context.parse_value(arguments[1]),
                            to: parse_context.parse_register(arguments[0]),
                        }
                    }
                    "jmp" => {
                        expect_argument_count(line_num, arguments, 1)?;
                        ScriptByteCode::Jump {
                            label: parse_context.parse_label(arguments[0])?,
                        }
                    }
                    "jl" | "jle" | "jg" | "jge" | "je" | "jne" => {
                        expect_argument_count(line_num, arguments, 3)?;
                        ScriptByteCode::JumpConditional {
                            label: parse_context.parse_label(arguments[0])?,
                            b: parse_context.parse_value(arguments[2]),
                            a: parse_context.parse_register(arguments[1]),
                            condition: match parts[0] {
                                "jl" => JumpCondition::Less,
                                "jle" => JumpCondition::LessEqual,
                                "jg" => JumpCondition::Greater,
                                "jge" => JumpCondition::GreaterEqual,
                                "je" => JumpCondition::Equal,
                                "jne" => JumpCondition::NotEqual,
                                _ => unreachable!(),
                            },
                        }
                    }
                    "add" | "sub" | "mul" | "div" | "mod" | "or" | "xor" | "and" | "bsl"
                    | "bsr" => {
                        expect_argument_count(line_num, arguments, 3)?;
                        ScriptByteCode::Operation {
                            b: parse_context.parse_value(arguments[2]),
                            a: parse_context.parse_register(arguments[1]),
                            target: parse_context.parse_register(arguments[0]),
                            operation: match parts[0] {
                                "add" => Operation::Add,
                                "sub" => Operation::Sub,
                                "mul" => Operation::Mul,
                                "div" => Operation::Div,
                                "mod" => Operation::Mod,
                                "or" => Operation::Or,
                                "xor" => Operation::Xor,
                                "and" => Operation::And,
                                "bsl" => Operation::Bsl,
                                "bsr" => Operation::Bsr,
                                _ => unreachable!(),
                            },
                        }
                    }
                    "not" => {
                        expect_argument_count(line_num, arguments, 2)?;
                        ScriptByteCode::Operation {
                            b: RegisterOrImmediate::Immediate(0),
                            a: parse_context.parse_register(arguments[1]),
                            target: parse_context.parse_register(arguments[0]),
                            operation: Operation::Not,
                        }
                    }
                    opcode => {
                        ScriptByteCode::External(E::parse(opcode, arguments, &mut parse_context)?)
                    }
                })
            })
            .collect::<Result<Vec<ScriptByteCode<E>>, ScriptParseError>>()?;
        Ok(CompiledScript {
            instructions,
            named_registers: parse_context.registers,
        })
    }
}
impl<'de, E: ExternalScriptByteCode> serde::Deserialize<'de> for CompiledScript<E> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct ScriptVisitor<E>(PhantomData<E>);
        impl<'de, V: ExternalScriptByteCode> serde::de::Visitor<'de> for ScriptVisitor<V> {
            type Value = CompiledScript<V>;
            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("valid model")
            }
            fn visit_str<E>(self, v: &str) -> Result<Self::Value, E>
            where
                E: serde::de::Error,
            {
                CompiledScript::<V>::parse(v)
                    .map_err(|error| serde::de::Error::custom(format!("{:?}", error)))
            }
        }
        deserializer.deserialize_str(ScriptVisitor::<E>(PhantomData))
    }
}

#[derive(Default)]
pub struct ScriptParseContext {
    pub register_map: HashMap<String, RegisterId>,
    pub registers: Vec<String>,
    pub labels: HashMap<String, ScriptLabel>,
    pub current_line_num: usize,
}
impl ScriptParseContext {
    pub fn parse_register(&mut self, name: &str) -> RegisterId {
        if let Some(register) = self.register_map.get(name) {
            return *register;
        }
        let id = self.registers.len();
        self.registers.push(name.to_string());
        self.register_map.insert(name.to_string(), id);
        id
    }
    pub fn parse_value(&mut self, input: &str) -> RegisterOrImmediate {
        match input.parse::<ScriptValue>() {
            Ok(immediate) => RegisterOrImmediate::Immediate(immediate),
            Err(_) => RegisterOrImmediate::Register(self.parse_register(input)),
        }
    }
    pub fn parse_label<'a>(&self, input: &'a str) -> Result<ScriptLabel, ScriptParseError<'a>> {
        self.labels
            .get(input)
            .cloned()
            .ok_or_else(|| ScriptParseError::UnknownLabel {
                line: self.current_line_num,
                label: input,
            })
    }
}
#[derive(Debug)]
pub enum ScriptParseError<'a> {
    UnknownOpCode {
        line: usize,
        opcode: &'a str,
    },
    UnknownLabel {
        line: usize,
        label: &'a str,
    },
    IllegalArguments {
        line: usize,
        expected_count: usize,
        actual_count: usize,
    },
    ExternalError {
        line: usize,
        error: String,
    },
    DuplicateLabel {
        line: usize,
    },
}
#[derive(Serialize, Deserialize)]
pub struct ScriptState {
    pub pc: usize,
    pub registers: Box<[ScriptValue]>,
}
impl ScriptState {
    pub fn new<E>(script: &CompiledScript<E>) -> Self {
        ScriptState {
            pc: 0,
            registers: std::iter::repeat_n(0, script.named_registers.len()).collect(),
        }
    }
    pub fn run<E>(
        &mut self,
        script: &CompiledScript<E>,
        mut callback: impl FnMut(&mut ScriptState, &E) -> CallbackResult,
        max_steps: usize,
    ) -> RunResult {
        if self.registers.len() != script.named_registers.len() {
            panic!("mismatched script variables");
        }
        if script.instructions.is_empty() {
            return RunResult::Suspended;
        }
        for _ in 0..max_steps {
            self.pc %= script.instructions.len();
            let previous_pc = self.pc;
            let instruction = &script.instructions[previous_pc];
            self.pc += 1;
            match instruction {
                ScriptByteCode::Move { from, to } => {
                    self.registers[*to] = self.resolve_value(from);
                }
                ScriptByteCode::External(action) => {
                    let result = callback(self, action);
                    match result {
                        CallbackResult::Continue => {}
                        CallbackResult::Suspend => {
                            return RunResult::Suspended;
                        }
                        CallbackResult::Wait => {
                            self.pc = previous_pc;
                            return RunResult::Suspended;
                        }
                    }
                }
                ScriptByteCode::Jump { label } => {
                    self.pc = *label;
                }
                ScriptByteCode::JumpConditional {
                    label,
                    a,
                    b,
                    condition,
                } => {
                    let a = self.registers[*a];
                    let b = self.resolve_value(b);
                    let passed = match condition {
                        JumpCondition::Less => a < b,
                        JumpCondition::LessEqual => a <= b,
                        JumpCondition::Greater => a > b,
                        JumpCondition::GreaterEqual => a >= b,
                        JumpCondition::Equal => a == b,
                        JumpCondition::NotEqual => a != b,
                    };
                    if passed {
                        self.pc = *label;
                    }
                }
                ScriptByteCode::Operation {
                    a,
                    b,
                    target,
                    operation,
                } => {
                    let a = self.registers[*a];
                    let b = self.resolve_value(b);
                    let output = match operation {
                        Operation::Add => a.wrapping_add(b),
                        Operation::Sub => a.wrapping_sub(b),
                        Operation::Mul => a.wrapping_mul(b),
                        Operation::Div => a.wrapping_div(b),
                        Operation::Mod => a.mod_floor(&b),
                        Operation::Or => a | b,
                        Operation::Xor => a ^ b,
                        Operation::And => a & b,
                        Operation::Not => !a,
                        Operation::Bsl => a << b,
                        Operation::Bsr => a >> b,
                    };
                    self.registers[*target] = output;
                }
            }
        }
        RunResult::TimedOut
    }
    pub fn resolve_value(&self, value: &RegisterOrImmediate) -> ScriptValue {
        match value {
            RegisterOrImmediate::Register(register) => self.registers[*register],
            RegisterOrImmediate::Immediate(value) => *value,
        }
    }
}
pub enum CallbackResult {
    Continue,
    Suspend,
    Wait,
}
pub enum RunResult {
    Suspended,
    TimedOut,
}
