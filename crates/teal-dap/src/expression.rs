//! 用于无副作用调试器表达式的限制性分析器。
//!
//! 工作台与 DAP 适配器共用这里的解析结果，`teal_vm` 只负责在暂停帧上求值。

use teal_compiler::{LexicalToken, LexicalTokenKind, Source, tokenize};
use teal_vm::{VmDebugBinaryOp, VmDebugConversion, VmDebugExpression, VmDebugLiteral, VmDebugUnaryOp};

/// 分析暂停的 VM 评估员所接受的纯表达式子集。
///
/// 函数和方法调用被拒绝。唯一的调用形状是内置的 `int`、 `float`、 `bool` 和 `string` 转换。
/// 返回的表达式随后交给 [`teal_vm::VmDebugState::evaluate`] 在指定帧上求值；本函数不访问
/// VM 状态，也不执行任何宿主操作。
///
/// # Errors
/// 输入包含未知词法单元或语法超出该子集时返回说明文本。
pub fn parse(text: &str) -> Result<VmDebugExpression, String> {
    let source = Source::new("<debug-evaluate>", text);
    let tokens = tokenize(&source)
        .into_iter()
        .filter(|token| token.kind != LexicalTokenKind::Comment)
        .collect::<Vec<_>>();
    if tokens
        .iter()
        .any(|token| token.kind == LexicalTokenKind::Error)
    {
        return Err("debug expression contains an unknown token".to_owned());
    }
    let mut parser = Parser { tokens, cursor: 0 };
    let expression = parser.expression(0)?;
    if parser.peek().is_some() {
        return Err(format!("unexpected token `{}`", parser.peek_text()));
    }
    Ok(expression)
}

struct Parser {
    tokens: Vec<LexicalToken>,
    cursor: usize,
}

impl Parser {
    fn expression(&mut self, minimum_precedence: u8) -> Result<VmDebugExpression, String> {
        let mut left = self.unary()?;
        while let Some((precedence, operator)) = binary_operator(self.peek_text()) {
            if precedence < minimum_precedence {
                break;
            }
            self.cursor += 1;
            let right = self.expression(precedence + 1)?;
            left = VmDebugExpression::Binary(operator, Box::new(left), Box::new(right));
        }
        Ok(left)
    }

    fn unary(&mut self) -> Result<VmDebugExpression, String> {
        if self.consume("!") {
            return Ok(VmDebugExpression::Unary(
                VmDebugUnaryOp::Not,
                Box::new(self.unary()?),
            ));
        }
        if self.consume("-") {
            return Ok(VmDebugExpression::Unary(
                VmDebugUnaryOp::Negate,
                Box::new(self.unary()?),
            ));
        }
        let mut expression = self.primary()?;
        loop {
            if self.consume("[") {
                let index = self.expression(0)?;
                self.expect("]")?;
                expression = VmDebugExpression::Index(Box::new(expression), Box::new(index));
            } else if self.consume(".") {
                let name = self.identifier()?;
                expression = VmDebugExpression::Field(Box::new(expression), name);
            } else {
                break;
            }
        }
        Ok(expression)
    }

    fn primary(&mut self) -> Result<VmDebugExpression, String> {
        if self.consume("(") {
            let expression = self.expression(0)?;
            self.expect(")")?;
            return Ok(expression);
        }
        if self.consume("[") {
            let mut elements = Vec::new();
            while !self.consume("]") {
                elements.push(self.expression(0)?);
                if self.consume("]") {
                    break;
                }
                self.expect(",")?;
            }
            return Ok(VmDebugExpression::Array(elements));
        }
        if self.consume("{") {
            let mut entries = Vec::new();
            while !self.consume("}") {
                let key = self.expression(0)?;
                self.expect(":")?;
                let value = self.expression(0)?;
                entries.push((key, value));
                if self.consume("}") {
                    break;
                }
                self.expect(",")?;
            }
            return Ok(VmDebugExpression::Map(entries));
        }

        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| "debug expression ended unexpectedly".to_owned())?;
        self.cursor += 1;
        match token.kind {
            LexicalTokenKind::Number => {
                if token.text.contains('.') {
                    token
                        .text
                        .parse::<f64>()
                        .map(VmDebugLiteral::Float)
                        .map(VmDebugExpression::Literal)
                        .map_err(|_| format!("invalid floating-point literal `{}`", token.text))
                } else {
                    token
                        .text
                        .parse::<i32>()
                        .map(VmDebugLiteral::Int)
                        .map(VmDebugExpression::Literal)
                        .map_err(|_| {
                            format!(
                                "integer literal `{}` is outside the 32-bit range",
                                token.text
                            )
                        })
                }
            }
            LexicalTokenKind::String => serde_json::from_str::<String>(&token.text)
                .map(VmDebugLiteral::String)
                .map(VmDebugExpression::Literal)
                .map_err(|_| "debug string must be a plain quoted literal".to_owned()),
            LexicalTokenKind::Keyword if token.text == "true" => {
                Ok(VmDebugExpression::Literal(VmDebugLiteral::Bool(true)))
            }
            LexicalTokenKind::Keyword if token.text == "false" => {
                Ok(VmDebugExpression::Literal(VmDebugLiteral::Bool(false)))
            }
            LexicalTokenKind::Keyword if token.text == "none" => {
                Ok(VmDebugExpression::Literal(VmDebugLiteral::None))
            }
            LexicalTokenKind::Identifier | LexicalTokenKind::Keyword => {
                let name = token.text.to_string();
                if self.consume("(") {
                    let conversion = match name.as_str() {
                        "int" => VmDebugConversion::Int,
                        "float" => VmDebugConversion::Float,
                        "bool" => VmDebugConversion::Bool,
                        "string" => VmDebugConversion::String,
                        _ => {
                            return Err(
                                "function and method calls are not allowed while evaluating"
                                    .to_owned(),
                            );
                        }
                    };
                    let value = self.expression(0)?;
                    self.expect(")")?;
                    Ok(VmDebugExpression::Convert(conversion, Box::new(value)))
                } else {
                    Ok(VmDebugExpression::Variable(name))
                }
            }
            _ => Err(format!("unexpected token `{}`", token.text)),
        }
    }

    fn identifier(&mut self) -> Result<String, String> {
        let token = self
            .peek()
            .cloned()
            .ok_or_else(|| "expected an identifier".to_owned())?;
        if token.kind != LexicalTokenKind::Identifier {
            return Err(format!("expected an identifier, found `{}`", token.text));
        }
        self.cursor += 1;
        Ok(token.text.to_string())
    }

    fn expect(&mut self, expected: &str) -> Result<(), String> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(format!(
                "expected `{expected}`, found `{}`",
                self.peek_text()
            ))
        }
    }

    fn consume(&mut self, spelling: &str) -> bool {
        if self.peek_text() == spelling {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<&LexicalToken> {
        self.tokens.get(self.cursor)
    }

    fn peek_text(&self) -> &str {
        self.peek().map_or("<end>", |token| token.text.as_str())
    }
}

fn binary_operator(spelling: &str) -> Option<(u8, VmDebugBinaryOp)> {
    let (precedence, operator) = match spelling {
        "||" => (1, VmDebugBinaryOp::Or),
        "&&" => (2, VmDebugBinaryOp::And),
        "==" => (3, VmDebugBinaryOp::Equal),
        "!=" => (3, VmDebugBinaryOp::NotEqual),
        "<" => (4, VmDebugBinaryOp::Less),
        "<=" => (4, VmDebugBinaryOp::LessEqual),
        ">" => (4, VmDebugBinaryOp::Greater),
        ">=" => (4, VmDebugBinaryOp::GreaterEqual),
        "+" => (5, VmDebugBinaryOp::Add),
        "-" => (5, VmDebugBinaryOp::Subtract),
        "*" => (6, VmDebugBinaryOp::Multiply),
        "/" => (6, VmDebugBinaryOp::Divide),
        "%" => (6, VmDebugBinaryOp::Modulo),
        _ => return None,
    };
    Some((precedence, operator))
}

#[cfg(test)]
mod tests {
    use super::parse;
    use teal_vm::{VmDebugBinaryOp, VmDebugExpression, VmDebugLiteral};

    #[test]
    fn none_literal_is_available_to_debug_evaluation() {
        assert_eq!(
            parse("none").unwrap(),
            VmDebugExpression::Literal(VmDebugLiteral::None)
        );
    }

    #[test]
    fn precedence_and_postfix_forms_are_preserved() {
        let expression = parse("items[1].value + int(delta) * 2").unwrap();
        assert!(matches!(
            expression,
            VmDebugExpression::Binary(VmDebugBinaryOp::Add, _, right)
                if matches!(*right, VmDebugExpression::Binary(VmDebugBinaryOp::Multiply, _, _))
        ));
    }

    #[test]
    fn calls_and_assignments_are_rejected() {
        assert!(parse("host_call(1)").unwrap_err().contains("not allowed"));
        assert!(parse("value = 1").is_err());
    }
}
