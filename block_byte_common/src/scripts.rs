use std::{collections::HashMap, marker::PhantomData};

use num_integer::Integer;

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
trait ExternalScriptByteCode: Sized {
    fn parse<'a>(opcode: &'a str, arguments: &[&'a str]) -> Result<Self, ScriptParseError<'a>>;
}
impl<E: ExternalScriptByteCode> CompiledScript<E> {
    pub fn parse<'a>(input: &'a str) -> Result<Self, ScriptParseError<'a>> {
        let mut instructions = Vec::new();
        let mut labels = HashMap::new();
        let mut registers = RegisterAllocator::default();
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
                labels.insert(line[..line.len() - 1].to_string(), instructions.len());
            } else {
                instructions.push((line_num, line));
            }
        }
        let instructions: Vec<ScriptByteCode<E>> = instructions
            .into_iter()
            .map(|(line_num, line)| {
                let parts: Vec<_> = line.split(" ").filter(|part| !part.is_empty()).collect();
                let arguments = &parts[1..];
                let mut parse_value = |input: &str| -> RegisterOrImmediate {
                    match input.parse::<ScriptValue>() {
                        Ok(immediate) => RegisterOrImmediate::Immediate(immediate),
                        Err(_) => RegisterOrImmediate::Register(registers.register(input)),
                    }
                };
                let parse_label = |input: &'a str| -> Result<ScriptLabel, ScriptParseError<'a>> {
                    labels
                        .get(input)
                        .cloned()
                        .ok_or_else(|| ScriptParseError::UnknownLabel {
                            line: line_num,
                            label: input,
                        })
                };
                Ok(match parts[0] {
                    "move" => {
                        expect_argument_count(line_num, arguments, 2)?;
                        ScriptByteCode::Move {
                            from: parse_value(arguments[1]),
                            to: registers.register(arguments[0]),
                        }
                    }
                    "jmp" => {
                        expect_argument_count(line_num, arguments, 1)?;
                        ScriptByteCode::Jump {
                            label: parse_label(arguments[0])?,
                        }
                    }
                    "jl" | "jle" | "jg" | "jge" | "je" | "jne" => {
                        expect_argument_count(line_num, arguments, 3)?;
                        ScriptByteCode::JumpConditional {
                            label: parse_label(arguments[0])?,
                            b: parse_value(arguments[2]),
                            a: registers.register(arguments[1]),
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
                    "add" | "sub" | "mul" | "div" | "mod" => {
                        expect_argument_count(line_num, arguments, 3)?;
                        ScriptByteCode::Operation {
                            b: parse_value(arguments[2]),
                            a: registers.register(arguments[1]),
                            target: registers.register(arguments[0]),
                            operation: match parts[0] {
                                "add" => Operation::Add,
                                "sub" => Operation::Sub,
                                "mul" => Operation::Mul,
                                "div" => Operation::Div,
                                "mod" => Operation::Mod,
                                _ => unreachable!(),
                            },
                        }
                    }
                    opcode => ScriptByteCode::External(E::parse(opcode, arguments)?),
                })
            })
            .collect::<Result<Vec<ScriptByteCode<E>>, ScriptParseError>>()?;
        Ok(CompiledScript {
            instructions,
            named_registers: registers.registers,
        })
    }
}
#[derive(Default)]
pub struct RegisterAllocator {
    pub register_map: HashMap<String, RegisterId>,
    pub registers: Vec<String>,
}
impl RegisterAllocator {
    pub fn register(&mut self, name: &str) -> RegisterId {
        if let Some(register) = self.register_map.get(name) {
            return *register;
        }
        let id = self.registers.len();
        self.registers.push(name.to_string());
        self.register_map.insert(name.to_string(), id);
        id
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
}
pub struct ScriptState<E> {
    pub pc: usize,
    pub registers: Vec<ScriptValue>,
    pub _pd: PhantomData<E>,
}
impl<E> ScriptState<E> {
    pub fn new(script: &CompiledScript<E>) -> Self {
        ScriptState::<E> {
            pc: 0,
            registers: std::iter::repeat_n(0, script.named_registers.len()).collect(),
            _pd: PhantomData,
        }
    }
    pub fn run(
        &mut self,
        script: &CompiledScript<E>,
        mut callback: impl FnMut(&mut ScriptState<E>, &E) -> CallbackResult,
        max_steps: usize,
    ) -> RunResult {
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
                    };
                    self.registers[*target] = output;
                }
            }
        }
        RunResult::TimedOut
    }
    fn resolve_value(&self, value: &RegisterOrImmediate) -> ScriptValue {
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
