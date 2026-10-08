//! 确定性 LR 表构建、有界 GLR 执行与具体语法树。
//!
//! 缓存使用 bincode 并校验完整消费、文法哈希及引擎 ABI。冲突候选保持稳定顺序，
//! 重复列表节点展平；增量接口复用可匹配的节点身份，不承诺常数时间重新解析。

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque},
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{ENGINE_ABI, GrammarError, GrammarSpec, InputEdit, LexedToken, Lexer, TokenCache};

static NEXT_NODE_ID: AtomicU64 = AtomicU64::new(1);

const EOF: &str = "$";
const MAX_GLR_STACKS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Production {
    lhs: String,
    rhs: Vec<String>,
    node_kind: String,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
struct Item {
    production: usize,
    dot: usize,
    lookahead: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
enum Action {
    Shift(usize),
    Reduce(usize),
    Accept,
}

/// 保留全部冲突供 GLR 执行的生成式规范 LR(1) 表。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParserTable {
    productions: Vec<Production>,
    actions: BTreeMap<(usize, String), Vec<Action>>,
    gotos: BTreeMap<(usize, String), usize>,
    state_count: usize,
    grammar_hash: [u8; 32],
}

#[derive(Serialize, Deserialize)]
struct CachedTable {
    engine_abi: u16,
    table: ParserTable,
}

impl ParserTable {
    /// 构建规范 LR(1) 状态并保留移进/归约或归约/归约冲突。
    pub fn build(grammar: &GrammarSpec) -> Result<Self, ParseError> {
        grammar.validate()?;

        let mut productions = vec![Production {
            lhs: "@start".to_owned(),
            rhs: vec![grammar.start.clone()],
            node_kind: grammar.start.clone(),
        }];
        productions.extend(grammar.productions.iter().map(|production| Production {
            lhs: production.lhs.clone(),
            rhs: production.rhs.clone(),
            node_kind: production.node_kind.clone(),
        }));

        let non_terminals = productions
            .iter()
            .map(|production| production.lhs.clone())
            .collect::<BTreeSet<_>>();
        let (first, nullable) = first_sets(&productions, &non_terminals);
        let initial = closure(
            BTreeSet::from([Item {
                production: 0,
                dot: 0,
                lookahead: EOF.to_owned(),
            }]),
            &productions,
            &non_terminals,
            &first,
            &nullable,
        );

        let mut states = vec![initial];
        let mut transitions = BTreeMap::new();
        let mut cursor = 0;
        while cursor < states.len() {
            let symbols = states[cursor]
                .iter()
                .filter_map(|item| productions[item.production].rhs.get(item.dot))
                .cloned()
                .collect::<BTreeSet<_>>();
            for symbol in symbols {
                let moved = states[cursor]
                    .iter()
                    .filter(|item| productions[item.production].rhs.get(item.dot) == Some(&symbol))
                    .map(|item| Item {
                        production: item.production,
                        dot: item.dot + 1,
                        lookahead: item.lookahead.clone(),
                    })
                    .collect::<BTreeSet<_>>();
                let target = closure(moved, &productions, &non_terminals, &first, &nullable);
                let target_id = states
                    .iter()
                    .position(|state| state == &target)
                    .unwrap_or_else(|| {
                        states.push(target);
                        states.len() - 1
                    });
                transitions.insert((cursor, symbol), target_id);
            }
            cursor += 1;
        }

        let mut actions: BTreeMap<(usize, String), Vec<Action>> = BTreeMap::new();
        let mut gotos = BTreeMap::new();
        for (state_id, state) in states.iter().enumerate() {
            for item in state {
                let production = &productions[item.production];
                if let Some(symbol) = production.rhs.get(item.dot) {
                    let target = transitions[&(state_id, symbol.clone())];
                    if non_terminals.contains(symbol) {
                        gotos.insert((state_id, symbol.clone()), target);
                    } else {
                        actions
                            .entry((state_id, symbol.clone()))
                            .or_default()
                            .push(Action::Shift(target));
                    }
                } else if item.production == 0 && item.lookahead == EOF {
                    actions
                        .entry((state_id, EOF.to_owned()))
                        .or_default()
                        .push(Action::Accept);
                } else {
                    actions
                        .entry((state_id, item.lookahead.clone()))
                        .or_default()
                        .push(Action::Reduce(item.production));
                }
            }
        }

        // 保持冲突候选顺序稳定，确保 GLR 探索具有确定性。
        for values in actions.values_mut() {
            values.sort();
            values.dedup();
        }

        Ok(Self {
            productions,
            actions,
            gotos,
            state_count: states.len(),
            grammar_hash: grammar.cache_key(),
        })
    }

    /// 返回生成的状态数。
    #[must_use]
    pub const fn state_count(&self) -> usize {
        self.state_count
    }

    /// 返回包含 GLR 冲突的表格单元数。
    #[must_use]
    pub fn conflict_count(&self) -> usize {
        self.actions
            .values()
            .filter(|actions| actions.len() > 1)
            .count()
    }

    /// 序列化受引擎 ABI 与文法哈希保护的缓存。
    #[must_use]
    pub fn to_cache_bytes(&self) -> Vec<u8> {
        bincode::serde::encode_to_vec(
            CachedTable {
                engine_abi: ENGINE_ABI,
                table: self.clone(),
            },
            bincode::config::standard(),
        )
        .expect("ParserTable cache serialization does not fail")
    }

    /// 验证引擎 ABI、完整文法哈希和全部字节消费后加载缓存表。
    ///
    /// 损坏、旧版本、错误文法或尾随数据均返回 [`ParseError::Cache`]。本函数不会隐式重建表，
    /// 调用方可据此丢弃缓存并重新构建；使用 JSON 替代 bincode 会破坏复合键表示。
    pub fn from_cache_bytes(bytes: &[u8], grammar: &GrammarSpec) -> Result<Self, ParseError> {
        let (cache, consumed): (CachedTable, usize) =
            bincode::serde::decode_from_slice(bytes, bincode::config::standard())
                .map_err(|error| ParseError::Cache(error.to_string()))?;

        // 只有有效前缀不足以通过检查；同时拒绝尾随数据与不兼容的表。
        if consumed != bytes.len() {
            return Err(ParseError::Cache(
                "Trailing bytes present in parser cache".to_owned(),
            ));
        }
        if cache.engine_abi != ENGINE_ABI || cache.table.grammar_hash != grammar.cache_key() {
            return Err(ParseError::Cache(
                "The parser cache is incompatible with this grammar".to_owned(),
            ));
        }

        cache.table.validate_cache_references(grammar)?;

        Ok(cache.table)
    }

    fn validate_cache_references(&self, grammar: &GrammarSpec) -> Result<(), ParseError> {
        grammar.validate()?;
        let expected = std::iter::once(Production {
            lhs: "@start".to_owned(),
            rhs: vec![grammar.start.clone()],
            node_kind: grammar.start.clone(),
        })
        .chain(grammar.productions.iter().map(|production| Production {
            lhs: production.lhs.clone(),
            rhs: production.rhs.clone(),
            node_kind: production.node_kind.clone(),
        }))
        .collect::<Vec<_>>();
        if self.productions != expected || self.state_count == 0 {
            return Err(ParseError::Cache(
                "The parser cache has invalid productions or states".to_owned(),
            ));
        }
        let terminals = grammar
            .tokens
            .iter()
            .map(|token| token.name.as_str())
            .chain(std::iter::once(EOF))
            .collect::<BTreeSet<_>>();
        let non_terminals = expected
            .iter()
            .map(|production| production.lhs.as_str())
            .collect::<BTreeSet<_>>();
        for ((state, terminal), actions) in &self.actions {
            if *state >= self.state_count
                || !terminals.contains(terminal.as_str())
                || actions.is_empty()
                || actions.iter().any(|action| match action {
                    Action::Shift(target) => *target >= self.state_count,
                    Action::Reduce(production) => {
                        *production == 0 || *production >= self.productions.len()
                    }
                    Action::Accept => terminal != EOF,
                })
            {
                return Err(ParseError::Cache(
                    "The parser cache has invalid action references".to_owned(),
                ));
            }
        }
        for ((state, non_terminal), target) in &self.gotos {
            if *state >= self.state_count
                || *target >= self.state_count
                || !non_terminals.contains(non_terminal.as_str())
            {
                return Err(ParseError::Cache(
                    "The parser cache has invalid goto references".to_owned(),
                ));
            }
        }
        Ok(())
    }
}

/// 在增量重新解析之间具有稳定标识的具体语法节点。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyntaxNode {
    id: u64,
    kind: String,
    range: Range<usize>,
    children: Vec<Self>,
    error: bool,
    missing: bool,
}

impl SyntaxNode {
    /// 返回稳定节点标识。
    #[must_use]
    pub const fn id(&self) -> u64 {
        self.id
    }

    /// 返回具体节点或词元类型。
    #[must_use]
    pub fn kind(&self) -> &str {
        &self.kind
    }

    /// 返回覆盖的 UTF-8 字节范围。
    #[must_use]
    pub fn range(&self) -> Range<usize> {
        self.range.clone()
    }

    /// 按源码顺序返回子节点。
    #[must_use]
    pub fn children(&self) -> &[Self] {
        &self.children
    }

    /// 返回此节点是否由解析器恢复产生。
    #[must_use]
    pub const fn is_error(&self) -> bool {
        self.error
    }

    /// 返回此节点是否为插入的零宽占位符。
    #[must_use]
    pub const fn is_missing(&self) -> bool {
        self.missing
    }

    /// 包含此节点的深度优先遍历。
    pub fn walk(&self) -> impl Iterator<Item = &Self> {
        SyntaxWalk { stack: vec![self] }
    }
}

/// 已解析源码、词元缓存、恢复计数与修订号。
#[derive(Clone, Debug)]
pub struct SyntaxTree {
    source: String,
    token_cache: TokenCache,
    root: SyntaxNode,
    errors: usize,
    revision: u64,
}

impl SyntaxTree {
    /// 返回根节点。
    #[must_use]
    pub const fn root(&self) -> &SyntaxNode {
        &self.root
    }

    /// 返回当前源码快照。
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// 返回未跳过的词法词元。
    #[must_use]
    pub fn tokens(&self) -> &[LexedToken] {
        self.token_cache.tokens()
    }

    /// 返回已恢复的意外词元数。
    #[must_use]
    pub const fn error_count(&self) -> usize {
        self.errors
    }

    /// 返回增量修订号。
    #[must_use]
    pub const fn revision(&self) -> u64 {
        self.revision
    }
}

/// 文法、缓存或解析器构造失败。
#[derive(Clone, Debug, PartialEq, Eq, Error)]
pub enum ParseError {
    /// 文法验证失败。
    #[error(transparent)]
    Grammar(#[from] GrammarError),

    /// 缓存表格式错误或不兼容。
    #[error("Analyzer cache error: {0}")]
    Cache(String),
}

/// 用于一个不可变文法的增量词法分析器与 GLR 解析器。
pub struct Parser {
    lexer: Lexer,
    table: ParserTable,
}

impl Parser {
    /// 构建词法分析器和规范 LR(1) 表。
    pub fn new(grammar: &GrammarSpec) -> Result<Self, ParseError> {
        Ok(Self {
            lexer: Lexer::new(grammar)?,
            table: ParserTable::build(grammar)?,
        })
    }

    /// 核对文法身份及表内引用后使用缓存表。
    ///
    /// # Errors
    /// 表中产生式、状态、动作或跳转引用无效时返回缓存错误。
    pub fn with_table(grammar: &GrammarSpec, table: ParserTable) -> Result<Self, ParseError> {
        if table.grammar_hash != grammar.cache_key() {
            return Err(ParseError::Cache(
                "Parse table does not match grammar".to_owned(),
            ));
        }
        table.validate_cache_references(grammar)?;
        Ok(Self {
            lexer: Lexer::new(grammar)?,
            table,
        })
    }

    /// 返回生成的解析表。
    #[must_use]
    pub const fn table(&self) -> &ParserTable {
        &self.table
    }

    /// 解析完整源码快照。
    #[must_use]
    pub fn parse(&self, source: impl Into<String>) -> SyntaxTree {
        let source = source.into();
        let token_cache = TokenCache::new(source.clone(), &self.lexer);
        let (root, errors) = self.parse_tokens(&source, token_cache.tokens());
        SyntaxTree {
            source,
            token_cache,
            root,
            errors,
            revision: 0,
        }
    }

    /// 应用一次 UTF-8 编辑，增量重新词法分析与解析，并复用未变化的节点 ID。
    #[must_use]
    pub fn reparse(&self, old: &SyntaxTree, edit: &InputEdit) -> SyntaxTree {
        let mut token_cache = old.token_cache.clone();
        let dirty = token_cache.apply(edit, &self.lexer);
        let source = token_cache.source().to_owned();
        let (mut root, errors) = self.parse_tokens(&source, token_cache.tokens());
        reuse_node_ids(&old.root, &old.source, &mut root, &source, dirty);
        SyntaxTree {
            source,
            token_cache,
            root,
            errors,
            revision: old.revision.wrapping_add(1),
        }
    }

    fn parse_tokens(&self, source: &str, tokens: &[LexedToken]) -> (SyntaxNode, usize) {
        let mut stacks = vec![ParseStack {
            states: vec![0],
            nodes: Vec::new(),
            errors: 0,
        }];
        for token in tokens {
            let previous = stacks;
            let expanded = self.expand(previous.clone(), &token.kind, token.range.start);
            let mut shifted = Vec::new();
            for stack in expanded {
                let state = *stack
                    .states
                    .last()
                    .expect("the parse stack always contains state zero");
                let actions = self
                    .table
                    .actions
                    .get(&(state, token.kind.clone()))
                    .cloned()
                    .unwrap_or_default();
                for action in actions {
                    if let Action::Shift(target) = action {
                        let mut next = stack.clone();
                        next.states.push(target);
                        next.nodes.push(SyntaxNode {
                            id: next_node_id(),
                            kind: token.kind.clone(),
                            range: token.range.clone(),
                            children: Vec::new(),
                            error: token.kind == "ERROR",
                            missing: false,
                        });
                        shifted.push(next);
                    }
                }
            }
            if shifted.is_empty() {
                stacks = previous;
                for stack in &mut stacks {
                    stack.nodes.push(SyntaxNode {
                        id: next_node_id(),
                        kind: "ERROR".to_owned(),
                        range: token.range.clone(),
                        children: Vec::new(),
                        error: true,
                        missing: false,
                    });
                    stack.errors += 1;
                }
            } else {
                stacks = prune(shifted);
            }
        }
        let position = source.len();
        let expanded = self.expand(stacks, EOF, position);
        let best = expanded
            .into_iter()
            .filter(|stack| {
                let state = *stack.states.last().unwrap_or(&0);
                self.table
                    .actions
                    .get(&(state, EOF.to_owned()))
                    .is_some_and(|actions| actions.contains(&Action::Accept))
            })
            .min_by_key(|stack| stack.errors);
        let mut best = best.unwrap_or_else(|| ParseStack {
            states: vec![0],
            nodes: Vec::new(),
            errors: tokens.len(),
        });
        let errors = best.errors;
        let root = if best.nodes.len() == 1 {
            best.nodes.pop().unwrap()
        } else {
            let missing = best.nodes.is_empty();
            SyntaxNode {
                id: next_node_id(),
                kind: if errors == 0 { "root" } else { "ERROR" }.to_owned(),
                range: 0..source.len(),
                children: best.nodes,
                error: errors != 0,
                missing,
            }
        };
        (root, errors)
    }

    fn expand(&self, stacks: Vec<ParseStack>, terminal: &str, position: usize) -> Vec<ParseStack> {
        let mut queue = VecDeque::from(stacks);
        let mut ready = Vec::new();
        let mut seen = HashSet::new();
        while let Some(stack) = queue.pop_front() {
            let signature = (
                stack.states.clone(),
                stack
                    .nodes
                    .iter()
                    .map(|node| (node.kind.clone(), node.range.start, node.range.end))
                    .collect::<Vec<_>>(),
            );
            if !seen.insert(signature) {
                continue;
            }
            let state = *stack
                .states
                .last()
                .expect("the parse stack always contains state zero");
            let actions = self
                .table
                .actions
                .get(&(state, terminal.to_owned()))
                .cloned()
                .unwrap_or_default();
            if actions
                .iter()
                .any(|action| matches!(action, Action::Shift(_) | Action::Accept))
            {
                ready.push(stack.clone());
            }
            for action in actions {
                let Action::Reduce(production_id) = action else {
                    continue;
                };
                let production = &self.table.productions[production_id];
                if production.rhs.len() >= stack.states.len()
                    || production.rhs.len() > stack.nodes.len()
                {
                    continue;
                }
                let mut reduced = stack.clone();
                let split = reduced.nodes.len() - production.rhs.len();
                let mut children = reduced.nodes.split_off(split);

                // 展平重复的左递归列表，避免每个元素增加一层树深度。
                if production.node_kind == production.lhs
                    && children
                        .first()
                        .is_some_and(|child| child.kind == production.node_kind)
                {
                    let first = children.remove(0);
                    let mut flattened = first.children;
                    flattened.extend(children);
                    children = flattened;
                }

                reduced
                    .states
                    .truncate(reduced.states.len() - production.rhs.len());
                let state = *reduced.states.last().unwrap_or(&0);
                let Some(target) = self
                    .table
                    .gotos
                    .get(&(state, production.lhs.clone()))
                    .copied()
                else {
                    continue;
                };
                let range = children
                    .first()
                    .zip(children.last())
                    .map_or(position..position, |(first, last)| {
                        first.range.start..last.range.end
                    });
                reduced.nodes.push(SyntaxNode {
                    id: next_node_id(),
                    kind: production.node_kind.clone(),
                    range,
                    error: children.iter().any(SyntaxNode::is_error),
                    missing: children.is_empty(),
                    children,
                });
                reduced.states.push(target);
                queue.push_back(reduced);
            }
            if queue.len() + ready.len() > MAX_GLR_STACKS {
                break;
            }
        }
        prune(ready)
    }
}

#[derive(Clone)]
struct ParseStack {
    states: Vec<usize>,
    nodes: Vec<SyntaxNode>,
    errors: usize,
}

fn first_sets(
    productions: &[Production],
    non_terminals: &BTreeSet<String>,
) -> (BTreeMap<String, BTreeSet<String>>, BTreeSet<String>) {
    let mut first = non_terminals
        .iter()
        .map(|symbol| (symbol.clone(), BTreeSet::new()))
        .collect::<BTreeMap<_, _>>();
    let mut nullable = BTreeSet::new();
    loop {
        let mut changed = false;
        for production in productions {
            if production.rhs.is_empty() {
                changed |= nullable.insert(production.lhs.clone());
                continue;
            }
            let mut all_nullable = true;
            for symbol in &production.rhs {
                if non_terminals.contains(symbol) {
                    let additions = first[symbol].clone();
                    let target = first
                        .get_mut(&production.lhs)
                        .expect("All left parts are nonterminals");
                    let before = target.len();
                    target.extend(additions);
                    changed |= target.len() != before;
                    if !nullable.contains(symbol) {
                        all_nullable = false;
                        break;
                    }
                } else {
                    changed |= first
                        .get_mut(&production.lhs)
                        .expect("All left parts are nonterminals")
                        .insert(symbol.clone());
                    all_nullable = false;
                    break;
                }
            }
            if all_nullable {
                changed |= nullable.insert(production.lhs.clone());
            }
        }
        if !changed {
            return (first, nullable);
        }
    }
}

fn closure(
    mut items: BTreeSet<Item>,
    productions: &[Production],
    non_terminals: &BTreeSet<String>,
    first: &BTreeMap<String, BTreeSet<String>>,
    nullable: &BTreeSet<String>,
) -> BTreeSet<Item> {
    loop {
        let mut additions = Vec::new();
        for item in &items {
            let production = &productions[item.production];
            let Some(symbol) = production.rhs.get(item.dot) else {
                continue;
            };
            if !non_terminals.contains(symbol) {
                continue;
            }
            let lookaheads = first_sequence(
                &production.rhs[item.dot + 1..],
                &item.lookahead,
                non_terminals,
                first,
                nullable,
            );
            for (production_id, _) in productions
                .iter()
                .enumerate()
                .filter(|(_, value)| &value.lhs == symbol)
            {
                for lookahead in &lookaheads {
                    additions.push(Item {
                        production: production_id,
                        dot: 0,
                        lookahead: lookahead.clone(),
                    });
                }
            }
        }
        let before = items.len();
        items.extend(additions);
        if items.len() == before {
            return items;
        }
    }
}

fn first_sequence(
    sequence: &[String],
    fallback: &str,
    non_terminals: &BTreeSet<String>,
    first: &BTreeMap<String, BTreeSet<String>>,
    nullable: &BTreeSet<String>,
) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    for symbol in sequence {
        if non_terminals.contains(symbol) {
            result.extend(first[symbol].iter().cloned());
            if !nullable.contains(symbol) {
                return result;
            }
        } else {
            result.insert(symbol.clone());
            return result;
        }
    }
    result.insert(fallback.to_owned());
    result
}

fn prune(mut stacks: Vec<ParseStack>) -> Vec<ParseStack> {
    stacks.sort_by_key(|stack| (stack.errors, stack.states.len()));
    let mut seen = HashSet::new();
    stacks.retain(|stack| {
        seen.insert((
            stack.states.clone(),
            stack
                .nodes
                .iter()
                .map(|node| (node.kind.clone(), node.range.start, node.range.end))
                .collect::<Vec<_>>(),
        ))
    });
    stacks.truncate(MAX_GLR_STACKS);
    stacks
}

fn next_node_id() -> u64 {
    NEXT_NODE_ID.fetch_add(1, Ordering::Relaxed)
}

struct SyntaxWalk<'a> {
    stack: Vec<&'a SyntaxNode>,
}

impl<'a> Iterator for SyntaxWalk<'a> {
    type Item = &'a SyntaxNode;

    fn next(&mut self) -> Option<Self::Item> {
        let node = self.stack.pop()?;
        self.stack.extend(node.children.iter().rev());
        Some(node)
    }
}

fn reuse_node_ids(
    old: &SyntaxNode,
    old_source: &str,
    new: &mut SyntaxNode,
    new_source: &str,
    dirty: Range<usize>,
) {
    let mut reusable: HashMap<(String, String, usize, bool), VecDeque<u64>> = HashMap::new();
    for node in old.walk() {
        if node.range.end <= dirty.start
            && let Some(text) = old_source.get(node.range.clone())
        {
            reusable
                .entry((
                    node.kind.clone(),
                    text.to_owned(),
                    node.children.len(),
                    node.error,
                ))
                .or_default()
                .push_back(node.id);
        }
    }
    reuse_new_node(new, new_source, &mut reusable);
}

fn reuse_new_node(
    node: &mut SyntaxNode,
    source: &str,
    reusable: &mut HashMap<(String, String, usize, bool), VecDeque<u64>>,
) {
    if let Some(text) = source.get(node.range.clone())
        && let Some(ids) = reusable.get_mut(&(
            node.kind.clone(),
            text.to_owned(),
            node.children.len(),
            node.error,
        ))
        && let Some(id) = ids.pop_front()
    {
        node.id = id;
    }
    for child in &mut node.children {
        reuse_new_node(child, source, reusable);
    }
}

#[cfg(test)]
mod tests {
    use super::super::GrammarBuilder;
    use super::{InputEdit, Parser, ParserTable};

    use super::{Action, EOF};

    fn expression_grammar() -> super::GrammarSpec {
        let mut grammar = GrammarBuilder::new("expression");
        grammar.token("number", "[0-9]+", 0);
        grammar.token("plus", "\\+", 0);
        grammar.skip("space", "\\s+", 0);
        grammar.production("expr", ["expr", "plus", "number"], "sum");
        grammar.production("expr", ["number"], "number_expr");
        grammar.start("expr");
        grammar.finish().expect("valid expression grammar")
    }

    #[test]
    fn builds_lr_tables_parses_and_recovers_unexpected_tokens() {
        let grammar = expression_grammar();
        let table = ParserTable::build(&grammar).expect("generate parse table");
        assert!(table.state_count() >= 4);
        let cache = table.to_cache_bytes();
        let restored = ParserTable::from_cache_bytes(&cache, &grammar).expect("cache round trip");
        assert_eq!(restored, table);
        let parser = Parser::with_table(&grammar, restored).expect("matching parse table");
        let tree = parser.parse("1 + 2 + 3");
        assert_eq!(tree.error_count(), 0);
        assert_eq!(tree.root().kind(), "sum");

        let recovered = parser.parse("1 + ? 2");
        assert!(recovered.error_count() > 0);
        assert!(recovered.root().walk().any(super::SyntaxNode::is_error));
    }

    #[test]
    fn parser_cache_rejects_invalid_production_state_shift_and_goto_references() {
        let grammar = expression_grammar();
        let table = ParserTable::build(&grammar).unwrap();
        let invalid_state = table.state_count;
        let invalid_production = table.productions.len();

        let mut malformed = Vec::new();
        let mut reduce = table.clone();
        reduce
            .actions
            .insert((0, EOF.into()), vec![Action::Reduce(invalid_production)]);
        malformed.push(reduce);

        let mut action_state = table.clone();
        action_state
            .actions
            .insert((invalid_state, "number".into()), vec![Action::Shift(0)]);
        malformed.push(action_state);

        let mut shift = table.clone();
        shift
            .actions
            .insert((0, "number".into()), vec![Action::Shift(invalid_state)]);
        malformed.push(shift);

        let mut goto_state = table.clone();
        goto_state.gotos.insert((invalid_state, "expr".into()), 0);
        malformed.push(goto_state);

        let mut goto_target = table.clone();
        goto_target.gotos.insert((0, "expr".into()), invalid_state);
        malformed.push(goto_target);

        for malformed in malformed {
            assert!(
                ParserTable::from_cache_bytes(&malformed.to_cache_bytes(), &grammar).is_err(),
                "malformed parser cache references must be rejected before parsing"
            );
        }
    }

    #[test]
    fn ambiguous_grammar_retains_glr_conflicts_and_incremental_ids() {
        let mut grammar = GrammarBuilder::new("ambiguous");
        grammar.token("a", "a", 0);
        grammar.production("items", ["items", "items"], "pair");
        grammar.production("items", ["a"], "item");
        grammar.start("items");
        let grammar = grammar.finish().expect("effective ambiguity grammar");
        let parser = Parser::new(&grammar).expect("build parser");
        assert!(parser.table().conflict_count() > 0);
        let before = parser.parse("aaa");
        let first_id = before.tokens()[0].range.clone();
        let first_node = before
            .root()
            .walk()
            .find(|node| node.range() == first_id)
            .expect("first lexical node")
            .id();
        let after = parser.reparse(
            &before,
            &InputEdit {
                old_range: 2..3,
                inserted: "aa".to_owned(),
            },
        );
        assert_eq!(after.revision(), 1);
        assert!(after.root().walk().any(|node| node.id() == first_node));
    }

    #[test]
    fn repeated_list_nodes_are_flat_for_large_documents() {
        let mut grammar = GrammarBuilder::new("tokens");
        grammar.token("word", "[a-z]+", 0);
        grammar.skip("space", "\\s+", 0);
        grammar.production("source", ["items"], "source");
        grammar.production("items", ["items", "word"], "items");
        grammar.production("items", ["word"], "items");
        grammar.start("source");
        let grammar = grammar.finish().expect("valid lexical list grammar");
        let parser = Parser::new(&grammar).expect("build parser");
        let tree = parser.parse("word ".repeat(5_000));
        assert_eq!(tree.error_count(), 0);
        let items = tree
            .root()
            .walk()
            .find(|node| node.kind() == "items")
            .expect("flat list node");
        assert_eq!(items.children().len(), 5_000);
    }
}
