//! Teal 编辑器的声明式增量具体语法树。
//!
//! 本库实现文法验证、词法分析、LR/GLR 分析表与语法捕获。
//! 解析接收文本快照，不拥有编辑器修订或线程调度；嵌入应用通过工作线程使用增量接口。

mod grammar;
mod lexer;
mod parser;
mod query;

pub use grammar::{
    ENGINE_ABI, GrammarBuilder, GrammarError, GrammarSpec, InjectionSpec, ProductionSpec, TokenSpec,
};
pub use lexer::{InputEdit, LexedToken, Lexer, TokenCache};
pub use parser::{ParseError, Parser, ParserTable, SyntaxNode, SyntaxTree};
pub use query::{Capture, HighlightQuery};
