//! Simple LLVM IR parser
//!
//! This module provides a basic LLVM IR parser that can handle common
//! accelerator patterns without requiring full LLVM library dependencies.
//! For complex IR, consider using inkwell or llvm-sys.

use crate::error::{Error, Result};
use crate::ir::{
    ICmpPredicate, LLVMBasicBlock, LLVMFunction, LLVMInstruction, LLVMModule, LLVMType, LLVMValue,
};
use std::collections::HashMap;

/// Simple LLVM IR parser for common accelerator patterns
pub struct LLVMParser {
    input: String,
    position: usize,
    value_counter: usize,
}

impl LLVMParser {
    pub fn new(input: String) -> Self {
        Self {
            input,
            position: 0,
            value_counter: 0,
        }
    }

    /// Parse LLVM IR module
    pub fn parse(&mut self) -> Result<LLVMModule> {
        let mut functions = Vec::new();
        let mut globals = HashMap::new();

        while !self.is_eof() {
            self.skip_whitespace();
            if self.is_eof() {
                break;
            }

            // Check for function definition
            if self.peek_word() == "define" {
                functions.push(self.parse_function()?);
            } else if self.peek_word().starts_with('@') {
                // Global variable
                let (name, value) = self.parse_global()?;
                globals.insert(name, value);
            } else {
                // Skip unknown lines
                self.skip_line();
            }
        }

        Ok(LLVMModule {
            name: "module".to_string(),
            functions,
            globals,
        })
    }

    fn parse_function(&mut self) -> Result<LLVMFunction> {
        // Skip "define"
        self.consume_word("define")?;
        self.skip_whitespace();

        // Parse return type
        let return_type = self.parse_type()?;
        self.skip_whitespace();

        // Parse function name (starts with @)
        self.consume_char('@')?;
        let name = self.parse_identifier()?;
        self.skip_whitespace();

        // Parse arguments
        self.consume_char('(')?;
        let arguments = self.parse_argument_list()?;
        self.consume_char(')')?;
        self.skip_whitespace();

        // Parse body
        self.consume_char('{')?;
        let basic_blocks = self.parse_basic_blocks()?;
        self.consume_char('}')?;

        Ok(LLVMFunction {
            name,
            arguments,
            basic_blocks,
            return_type,
        })
    }

    fn parse_basic_blocks(&mut self) -> Result<Vec<LLVMBasicBlock>> {
        let mut blocks = Vec::new();

        while !self.is_eof() && self.peek_char() != '}' {
            self.skip_whitespace();
            if self.peek_char() == '}' {
                break;
            }

            // Parse label
            let label = self.parse_label()?;
            self.consume_char(':')?;
            self.skip_whitespace();

            let mut bb = LLVMBasicBlock::new(label);

            // Parse instructions until terminator
            while !self.is_eof() {
                self.skip_whitespace();

                // Check for next BB or end
                if self.peek_char() == '}' || self.is_label() {
                    break;
                }

                let inst = self.parse_instruction()?;

                // Check if terminator
                if self.is_terminator(&inst) {
                    bb.set_terminator(inst);
                    break;
                } else {
                    bb.add_instruction(inst);
                }
            }

            blocks.push(bb);
        }

        Ok(blocks)
    }

    fn parse_instruction(&mut self) -> Result<LLVMInstruction> {
        self.skip_whitespace();

        // Check for assignment
        let dest = if self.peek_char() == '%' {
            let val = self.parse_value()?;
            self.skip_whitespace();
            self.consume_char('=')?;
            self.skip_whitespace();
            Some(val)
        } else {
            None
        };

        // Parse opcode
        let opcode = self.parse_word()?;
        self.skip_whitespace();

        // Parse based on opcode
        let inst = match opcode.as_str() {
            "add" => self.parse_add(dest.unwrap())?,
            "sub" => self.parse_sub(dest.unwrap())?,
            "mul" => self.parse_mul(dest.unwrap())?,
            "fadd" => self.parse_fadd(dest.unwrap())?,
            "fmul" => self.parse_fmul(dest.unwrap())?,
            "load" => self.parse_load(dest.unwrap())?,
            "store" => self.parse_store()?,
            "br" => self.parse_branch()?,
            "ret" => self.parse_ret()?,
            "icmp" => self.parse_icmp(dest.unwrap())?,
            _ => {
                log::warn!("Unsupported instruction: {}", opcode);
                self.skip_line();
                LLVMInstruction::Ret { value: None }
            }
        };

        Ok(inst)
    }

    fn parse_add(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        let ty = self.parse_type()?;
        self.skip_whitespace();
        let lhs = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let rhs = self.parse_value()?;
        self.skip_line();
        Ok(LLVMInstruction::Add { dest, lhs, rhs, ty })
    }

    fn parse_sub(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        let ty = self.parse_type()?;
        self.skip_whitespace();
        let lhs = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let rhs = self.parse_value()?;
        self.skip_line();
        Ok(LLVMInstruction::Sub { dest, lhs, rhs, ty })
    }

    fn parse_mul(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        let ty = self.parse_type()?;
        self.skip_whitespace();
        let lhs = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let rhs = self.parse_value()?;
        self.skip_line();
        Ok(LLVMInstruction::Mul { dest, lhs, rhs, ty })
    }

    fn parse_fadd(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        let ty = self.parse_type()?;
        self.skip_whitespace();
        let lhs = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let rhs = self.parse_value()?;
        self.skip_line();
        Ok(LLVMInstruction::FAdd { dest, lhs, rhs, ty })
    }

    fn parse_fmul(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        let ty = self.parse_type()?;
        self.skip_whitespace();
        let lhs = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let rhs = self.parse_value()?;
        self.skip_line();
        Ok(LLVMInstruction::FMul { dest, lhs, rhs, ty })
    }

    fn parse_load(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        let ty = self.parse_type()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let _ptr_ty = self.parse_type()?;
        self.skip_whitespace();
        let ptr = self.parse_value()?;
        self.skip_line();

        Ok(LLVMInstruction::Load { dest, ptr, ty })
    }

    fn parse_store(&mut self) -> Result<LLVMInstruction> {
        let _val_ty = self.parse_type()?;
        self.skip_whitespace();
        let value = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let _ptr_ty = self.parse_type()?;
        self.skip_whitespace();
        let ptr = self.parse_value()?;
        self.skip_line();

        Ok(LLVMInstruction::Store { value, ptr })
    }

    fn parse_branch(&mut self) -> Result<LLVMInstruction> {
        self.skip_whitespace();

        // Check if conditional
        if self.peek_word() == "label" {
            // Unconditional branch
            self.consume_word("label")?;
            self.skip_whitespace();
            let target = self.parse_label()?;
            self.skip_line();
            Ok(LLVMInstruction::Br { target })
        } else {
            // Conditional branch
            let _ty = self.parse_type()?;
            self.skip_whitespace();
            let condition = self.parse_value()?;
            self.skip_whitespace();
            self.consume_char(',')?;
            self.skip_whitespace();
            self.consume_word("label")?;
            self.skip_whitespace();
            let true_target = self.parse_label()?;
            self.skip_whitespace();
            self.consume_char(',')?;
            self.skip_whitespace();
            self.consume_word("label")?;
            self.skip_whitespace();
            let false_target = self.parse_label()?;
            self.skip_line();

            Ok(LLVMInstruction::CondBr {
                condition,
                true_target,
                false_target,
            })
        }
    }

    fn parse_ret(&mut self) -> Result<LLVMInstruction> {
        self.skip_whitespace();

        if self.peek_word() == "void" {
            self.consume_word("void")?;
            self.skip_line();
            Ok(LLVMInstruction::Ret { value: None })
        } else {
            let _ty = self.parse_type()?;
            self.skip_whitespace();
            let value = self.parse_value()?;
            self.skip_line();
            Ok(LLVMInstruction::Ret { value: Some(value) })
        }
    }

    fn parse_icmp(&mut self, dest: LLVMValue) -> Result<LLVMInstruction> {
        // Parse predicate
        let pred_str = self.parse_word()?;
        let predicate = match pred_str.as_str() {
            "eq" => ICmpPredicate::EQ,
            "ne" => ICmpPredicate::NE,
            "slt" => ICmpPredicate::SLT,
            "sle" => ICmpPredicate::SLE,
            "sgt" => ICmpPredicate::SGT,
            "sge" => ICmpPredicate::SGE,
            _ => ICmpPredicate::EQ,
        };

        self.skip_whitespace();
        let _ty = self.parse_type()?;
        self.skip_whitespace();
        let lhs = self.parse_value()?;
        self.skip_whitespace();
        self.consume_char(',')?;
        self.skip_whitespace();
        let rhs = self.parse_value()?;
        self.skip_line();

        Ok(LLVMInstruction::ICmp {
            dest,
            predicate,
            lhs,
            rhs,
        })
    }

    // Helper methods

    fn parse_type(&mut self) -> Result<LLVMType> {
        let word = self.parse_word()?;
        match word.as_str() {
            "void" => Ok(LLVMType::Void),
            "float" => Ok(LLVMType::Float),
            "double" => Ok(LLVMType::Double),
            w if w.starts_with('i') => {
                let bits = w[1..].parse::<u32>().unwrap_or(32);
                Ok(LLVMType::Integer { bits })
            }
            _ => Ok(LLVMType::Integer { bits: 32 }),
        }
    }

    fn parse_value(&mut self) -> Result<LLVMValue> {
        if self.peek_char() == '%' {
            self.consume_char('%')?;
            let name = self.parse_identifier()?;
            let id = self.value_counter;
            self.value_counter += 1;
            Ok(LLVMValue::register(name, id))
        } else if self.peek_char().is_ascii_digit() || self.peek_char() == '-' {
            let num_str = self.parse_number()?;
            let value = num_str.parse::<i64>().unwrap_or(0);
            Ok(LLVMValue::const_int(value, 32))
        } else if self.peek_char() == '@' {
            self.consume_char('@')?;
            let name = self.parse_identifier()?;
            Ok(LLVMValue::Global { name })
        } else {
            Err(Error::ParseError(format!(
                "Unexpected character: {}",
                self.peek_char()
            )))
        }
    }

    fn parse_argument_list(&mut self) -> Result<Vec<LLVMValue>> {
        let mut args = Vec::new();
        self.skip_whitespace();

        while self.peek_char() != ')' {
            let _ty = self.parse_type()?;
            self.skip_whitespace();
            let val = self.parse_value()?;
            args.push(val);

            self.skip_whitespace();
            if self.peek_char() == ',' {
                self.consume_char(',')?;
                self.skip_whitespace();
            }
        }

        Ok(args)
    }

    fn parse_global(&mut self) -> Result<(String, LLVMValue)> {
        self.consume_char('@')?;
        let name = self.parse_identifier()?;
        self.skip_line();
        Ok((name.clone(), LLVMValue::Global { name }))
    }

    fn parse_label(&mut self) -> Result<String> {
        if self.peek_char() == '%' {
            self.consume_char('%')?;
        }
        self.parse_identifier()
    }

    fn parse_identifier(&mut self) -> Result<String> {
        let start = self.position;
        while !self.is_eof()
            && (self.peek_char().is_alphanumeric()
                || self.peek_char() == '_'
                || self.peek_char() == '.')
        {
            self.position += 1;
        }
        Ok(self.input[start..self.position].to_string())
    }

    fn parse_word(&mut self) -> Result<String> {
        let start = self.position;
        while !self.is_eof()
            && !self.peek_char().is_whitespace()
            && self.peek_char() != ','
            && self.peek_char() != '('
            && self.peek_char() != ')'
        {
            self.position += 1;
        }
        Ok(self.input[start..self.position].to_string())
    }

    fn parse_number(&mut self) -> Result<String> {
        let start = self.position;
        if self.peek_char() == '-' {
            self.position += 1;
        }
        while !self.is_eof() && (self.peek_char().is_ascii_digit() || self.peek_char() == '.') {
            self.position += 1;
        }
        Ok(self.input[start..self.position].to_string())
    }

    fn peek_word(&self) -> String {
        let mut pos = self.position;
        while pos < self.input.len() && self.input.chars().nth(pos).unwrap().is_whitespace() {
            pos += 1;
        }
        let start = pos;
        while pos < self.input.len()
            && !self.input.chars().nth(pos).unwrap().is_whitespace()
            && self.input.chars().nth(pos) != Some(',')
        {
            pos += 1;
        }
        self.input[start..pos].to_string()
    }

    fn consume_word(&mut self, expected: &str) -> Result<()> {
        let word = self.parse_word()?;
        if word != expected {
            return Err(Error::ParseError(format!(
                "Expected '{}', got '{}'",
                expected, word
            )));
        }
        Ok(())
    }

    fn consume_char(&mut self, expected: char) -> Result<()> {
        self.skip_whitespace();
        if self.peek_char() != expected {
            return Err(Error::ParseError(format!(
                "Expected '{}', got '{}'",
                expected,
                self.peek_char()
            )));
        }
        self.position += 1;
        Ok(())
    }

    fn peek_char(&self) -> char {
        if self.is_eof() {
            '\0'
        } else {
            self.input.chars().nth(self.position).unwrap()
        }
    }

    fn skip_whitespace(&mut self) {
        while !self.is_eof() && self.peek_char().is_whitespace() {
            self.position += 1;
        }
        // Skip comments
        while !self.is_eof() && self.peek_char() == ';' {
            self.skip_line();
            while !self.is_eof() && self.peek_char().is_whitespace() {
                self.position += 1;
            }
        }
    }

    fn skip_line(&mut self) {
        while !self.is_eof() && self.peek_char() != '\n' {
            self.position += 1;
        }
        if !self.is_eof() {
            self.position += 1; // Skip newline
        }
    }

    fn is_eof(&self) -> bool {
        self.position >= self.input.len()
    }

    fn is_label(&self) -> bool {
        let word = self.peek_word();
        word.ends_with(':')
            || (!word.starts_with('%') && !word.is_empty() && self.peek_char().is_alphabetic())
    }

    fn is_terminator(&self, inst: &LLVMInstruction) -> bool {
        matches!(
            inst,
            LLVMInstruction::Br { .. }
                | LLVMInstruction::CondBr { .. }
                | LLVMInstruction::Ret { .. }
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_function() {
        let ir = r#"
define i32 @add(i32 %a, i32 %b) {
entry:
  %result = add i32 %a, %b
  ret i32 %result
}
        "#;

        let mut parser = LLVMParser::new(ir.to_string());
        let module = parser.parse().unwrap();

        assert_eq!(module.functions.len(), 1);
        assert_eq!(module.functions[0].name, "add");
    }

    #[test]
    fn test_parse_types() {
        let mut parser = LLVMParser::new("i32 float double void".to_string());

        assert_eq!(parser.parse_type().unwrap(), LLVMType::Integer { bits: 32 });
        parser.skip_whitespace();
        assert_eq!(parser.parse_type().unwrap(), LLVMType::Float);
        parser.skip_whitespace();
        assert_eq!(parser.parse_type().unwrap(), LLVMType::Double);
        parser.skip_whitespace();
        assert_eq!(parser.parse_type().unwrap(), LLVMType::Void);
    }

    #[test]
    fn test_parse_values() {
        let mut parser = LLVMParser::new("%reg 42 @global".to_string());

        let val1 = parser.parse_value().unwrap();
        assert!(matches!(val1, LLVMValue::Register { .. }));

        parser.skip_whitespace();
        let val2 = parser.parse_value().unwrap();
        assert!(matches!(val2, LLVMValue::ConstInt { value: 42, .. }));

        parser.skip_whitespace();
        let val3 = parser.parse_value().unwrap();
        assert!(matches!(val3, LLVMValue::Global { .. }));
    }
}
