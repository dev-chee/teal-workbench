//! 可序列化文法中间表示与声明校验。
//!
//! 词元、产生式、起始符号和捕获共同定义语言及缓存身份；文法语义变化须与引擎缓存 ABI 协调。

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

/// 文法 IR 与解析器生成器的缓存兼容版本。
pub const ENGINE_ABI: u16 = 1;

/// 在命名词法模式中生效的一个正则词元。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenSpec {
    /// 发送给解析器的终结符。
    pub name: String,

    /// 在当前字节位置匹配的 Rust 正则。
    pub pattern: String,

    /// 匹配长度相等时较高优先级胜出。
    pub priority: i32,

    /// 应用模式转换后跳过此词元。
    pub skip: bool,

    /// 此词元生效的词法模式；默认为 `main`。
    pub mode: String,

    /// 匹配后压入的可选模式。
    pub push_mode: Option<String>,

    /// 匹配后弹出一个非根模式。
    pub pop_mode: bool,
}

/// 一条上下文无关文法产生式。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProductionSpec {
    /// 此规则产生的非终结符。
    pub lhs: String,

    /// 按源码顺序排列的终结符和非终结符。
    pub rhs: Vec<String>,

    /// 具体语法节点类型，通常与 `lhs` 相同。
    pub node_kind: String,
}

/// 附加到一个具体节点类型的语言注入。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionSpec {
    /// 其内容使用另一种语言的宿主文法节点类型。
    pub node_kind: String,

    /// 注入的语言 ID。
    pub language: String,
}

/// 由 Teal 或 Rust 构建器生成的不可变可序列化文法。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrammarSpec {
    /// 稳定语言 ID。
    pub language: String,

    /// 不带点且不区分大小写的文件扩展名。
    pub extensions: Vec<String>,

    /// 词法词元定义。
    pub tokens: Vec<TokenSpec>,

    /// 上下文无关产生式。
    pub productions: Vec<ProductionSpec>,

    /// 根非终结符。
    pub start: String,

    /// 节点/词元类型到语义高亮捕获的映射。
    pub captures: BTreeMap<String, String>,

    /// 嵌套语言区域。
    pub injections: Vec<InjectionSpec>,
}

impl GrammarSpec {
    /// 验证名称、符号、模式、正则与起始规则。
    pub fn validate(&self) -> Result<(), GrammarError> {
        if self.language.trim().is_empty() {
            return Err(GrammarError::EmptyLanguage);
        }
        let mut terminals = BTreeSet::new();
        let mut modes = BTreeSet::from(["main".to_owned()]);
        for token in &self.tokens {
            if token.name.is_empty() || !terminals.insert(token.name.clone()) {
                return Err(GrammarError::DuplicateToken(token.name.clone()));
            }
            regex::Regex::new(&format!(r"\A(?:{})", token.pattern))
                .map_err(|error| GrammarError::Regex(token.name.clone(), error.to_string()))?;
            modes.insert(token.mode.clone());
            if let Some(mode) = &token.push_mode {
                modes.insert(mode.clone());
            }
        }
        let non_terminals = self
            .productions
            .iter()
            .map(|production| production.lhs.clone())
            .collect::<BTreeSet<_>>();
        if !non_terminals.contains(&self.start) {
            return Err(GrammarError::MissingStart(self.start.clone()));
        }
        for production in &self.productions {
            if production.lhs.is_empty() || production.node_kind.is_empty() {
                return Err(GrammarError::EmptyProduction);
            }
            for symbol in &production.rhs {
                if !terminals.contains(symbol) && !non_terminals.contains(symbol) {
                    return Err(GrammarError::UnknownSymbol(symbol.clone()));
                }
            }
        }
        if modes.iter().any(String::is_empty) {
            return Err(GrammarError::EmptyMode);
        }
        Ok(())
    }

    /// 返回确定性的文法/解析器缓存键。
    #[must_use]
    pub fn cache_key(&self) -> [u8; 32] {
        let mut hash = Sha256::new();
        hash.update(ENGINE_ABI.to_le_bytes());
        hash.update(serde_json::to_vec(self).expect("GrammarSpec serialization does not fail"));
        hash.finalize().into()
    }
}

/// 无效文法声明。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum GrammarError {
    /// 语言 ID 为空。
    #[error("Grammar language flag cannot be empty")]
    EmptyLanguage,

    /// 词元标识为空或重复。
    #[error("Token `{0}` is repeated or empty")]
    DuplicateToken(String),

    /// 词元正则无法编译。
    #[error("Invalid regular expression for token `{0}`: {1}")]
    Regex(String, String),

    /// 起始非终结符没有产生式。
    #[error("The starting nonterminal symbol `{0}` has no production")]
    MissingStart(String),

    /// 产生式左侧或节点类型为空。
    #[error("Production name cannot be empty")]
    EmptyProduction,

    /// 产生式引用未声明符号。
    #[error("Unknown grammar symbol `{0}`")]
    UnknownSymbol(String),

    /// 词法模式为空。
    #[error("Lexical pattern name cannot be empty")]
    EmptyMode,
}

/// Teal 宿主 API 使用的易用可变构建器。
#[derive(Clone, Debug)]
pub struct GrammarBuilder {
    spec: GrammarSpec,
}

impl GrammarBuilder {
    /// 开始一个空文法。
    #[must_use]
    pub fn new(language: impl Into<String>) -> Self {
        Self {
            spec: GrammarSpec {
                language: language.into(),
                extensions: Vec::new(),
                tokens: Vec::new(),
                productions: Vec::new(),
                start: String::new(),
                captures: BTreeMap::new(),
                injections: Vec::new(),
            },
        }
    }

    /// 添加一个文件扩展名。
    pub fn extension(&mut self, extension: impl Into<String>) -> &mut Self {
        self.spec.extensions.push(
            extension
                .into()
                .trim_start_matches('.')
                .to_ascii_lowercase(),
        );
        self
    }

    /// 在主词法模式中添加一个词元。
    pub fn token(
        &mut self,
        name: impl Into<String>,
        pattern: impl Into<String>,
        priority: i32,
    ) -> &mut Self {
        self.spec.tokens.push(TokenSpec {
            name: name.into(),
            pattern: pattern.into(),
            priority,
            skip: false,
            mode: "main".to_owned(),
            push_mode: None,
            pop_mode: false,
        });
        self
    }

    /// 添加空白或注释等跳过的词元。
    pub fn skip(
        &mut self,
        name: impl Into<String>,
        pattern: impl Into<String>,
        priority: i32,
    ) -> &mut Self {
        self.spec.tokens.push(TokenSpec {
            name: name.into(),
            pattern: pattern.into(),
            priority,
            skip: true,
            mode: "main".to_owned(),
            push_mode: None,
            pop_mode: false,
        });
        self
    }

    /// 添加一个带词法模式转换的完整词元。
    pub fn mode_token(&mut self, token: TokenSpec) -> &mut Self {
        self.spec.tokens.push(token);
        self
    }

    /// 添加一条产生式。
    pub fn production(
        &mut self,
        lhs: impl Into<String>,
        rhs: impl IntoIterator<Item = impl Into<String>>,
        node_kind: impl Into<String>,
    ) -> &mut Self {
        self.spec.productions.push(ProductionSpec {
            lhs: lhs.into(),
            rhs: rhs.into_iter().map(Into::into).collect(),
            node_kind: node_kind.into(),
        });
        self
    }

    /// 选择根非终结符。
    pub fn start(&mut self, symbol: impl Into<String>) -> &mut Self {
        self.spec.start = symbol.into();
        self
    }

    /// 将具体类型映射到高亮捕获。
    pub fn capture(&mut self, kind: impl Into<String>, capture: impl Into<String>) -> &mut Self {
        self.spec.captures.insert(kind.into(), capture.into());
        self
    }

    /// 添加嵌套语言注入。
    pub fn injection(&mut self, kind: impl Into<String>, language: impl Into<String>) -> &mut Self {
        self.spec.injections.push(InjectionSpec {
            node_kind: kind.into(),
            language: language.into(),
        });
        self
    }

    /// 验证并冻结此文法。
    pub fn finish(&self) -> Result<GrammarSpec, GrammarError> {
        let mut spec = self.spec.clone();
        spec.extensions.sort();
        spec.extensions.dedup();
        spec.validate()?;
        Ok(spec)
    }
}
