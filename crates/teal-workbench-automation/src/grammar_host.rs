//! 将类型化 Teal 宿主调用收集为已验证的文法声明。
//!
//! 脚本句柄只在对应宿主状态中有效；声明完成后生成可供原生分析器使用的文法数据。

use std::{
    collections::BTreeMap,
    fmt::Write as _,
    sync::{Arc, Mutex},
};

use teal_workbench_core::syntax::{GrammarBuilder, GrammarSpec};
use teal::{
    ExternalCallRecovery, HostError, HostCallSignature, HostFunctionDefinition, HostRegistry,
    HostValue, RuntimeType,
};

#[derive(Default)]
struct GrammarState {
    next_handle: i32,
    builders: BTreeMap<i32, GrammarBuilder>,
    grammars: Vec<GrammarSpec>,
}

fn register_host<F>(
    registry: &mut HostRegistry,
    module: &'static str,
    function: impl Into<String>,
    signature: HostCallSignature,
    capability: &'static str,
    callback: F,
) -> Result<(), teal::HostRegistryError>
where
    F: Fn(&[HostValue]) -> Result<HostValue, HostError> + Send + Sync + 'static,
{
    registry.register(HostFunctionDefinition::new(
        module,
        function,
        signature,
        capability,
        ExternalCallRecovery::Unsupported,
        callback,
    ))
}

/// 构建不可变原生文法规格的 Teal `std.bench.syntax` 模块。
#[derive(Clone)]
pub struct GrammarHost {
    state: Arc<Mutex<GrammarState>>,
}

impl Default for GrammarHost {
    fn default() -> Self {
        Self::new()
    }
}

impl GrammarHost {
    /// 创建空文法宿主。
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(GrammarState::default())),
        }
    }

    /// 将所有文法构建器函数注册到现有 Teal 注册表。
    pub fn register(&self, registry: &mut HostRegistry) -> Result<(), teal::HostRegistryError> {
        let state = Arc::clone(&self.state);
        register_host(
            registry,
            "std.bench.syntax",
            "begin",
            signature([RuntimeType::String], RuntimeType::Int),
            "syntax.register",
            move |arguments| {
                let language = string_arg(arguments, 0)?;
                let mut state = lock(&state)?;
                let handle = state.next_handle;
                state.next_handle = state
                    .next_handle
                    .checked_add(1)
                    .ok_or_else(|| host_error("Grammar handle overflow"))?;
                state.builders.insert(handle, GrammarBuilder::new(language));
                Ok(HostValue::Int(handle))
            },
        )?;
        register_builder_call(
            registry,
            "extension",
            [RuntimeType::Int, RuntimeType::String],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        builder.extension(string_arg(arguments, 1)?);
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        register_builder_call(
            registry,
            "token",
            [
                RuntimeType::Int,
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::Int,
            ],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        builder.token(
                            string_arg(arguments, 1)?,
                            string_arg(arguments, 2)?,
                            int_arg(arguments, 3)?,
                        );
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        register_builder_call(
            registry,
            "skip",
            [
                RuntimeType::Int,
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::Int,
            ],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        builder.skip(
                            string_arg(arguments, 1)?,
                            string_arg(arguments, 2)?,
                            int_arg(arguments, 3)?,
                        );
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        register_builder_call(
            registry,
            "production",
            [
                RuntimeType::Int,
                RuntimeType::String,
                RuntimeType::String,
                RuntimeType::String,
            ],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        let rhs = string_arg(arguments, 2)?
                            .split_whitespace()
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>();
                        builder.production(
                            string_arg(arguments, 1)?,
                            rhs,
                            string_arg(arguments, 3)?,
                        );
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        register_builder_call(
            registry,
            "start",
            [RuntimeType::Int, RuntimeType::String],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        builder.start(string_arg(arguments, 1)?);
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        register_builder_call(
            registry,
            "capture",
            [RuntimeType::Int, RuntimeType::String, RuntimeType::String],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        builder.capture(string_arg(arguments, 1)?, string_arg(arguments, 2)?);
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        register_builder_call(
            registry,
            "injection",
            [RuntimeType::Int, RuntimeType::String, RuntimeType::String],
            {
                let state = Arc::clone(&self.state);
                move |arguments| {
                    builder(&state, int_arg(arguments, 0)?, |builder| {
                        builder.injection(string_arg(arguments, 1)?, string_arg(arguments, 2)?);
                        Ok(())
                    })?;
                    Ok(HostValue::None)
                }
            },
        )?;
        let state = Arc::clone(&self.state);
        register_host(
            registry,
            "std.bench.syntax",
            "finish",
            signature([RuntimeType::Int], RuntimeType::String),
            "syntax.register",
            move |arguments| {
                let handle = int_arg(arguments, 0)?;
                let mut state = lock(&state)?;
                let grammar = state
                    .builders
                    .remove(&handle)
                    .ok_or_else(|| host_error(format!("Unknown grammar handle {handle}")))?
                    .finish()
                    .map_err(|error| host_error(error.to_string()))?;
                let mut key = String::with_capacity(64);
                for byte in grammar.cache_key() {
                    write!(key, "{byte:02x}").expect("Writing to string does not fail");
                }
                state.grammars.push(grammar);
                Ok(HostValue::String(key))
            },
        )?;
        Ok(())
    }

    /// 移除并返回所有已完成文法。
    pub fn take_grammars(&self) -> Result<Vec<GrammarSpec>, HostError> {
        Ok(std::mem::take(&mut lock(&self.state)?.grammars))
    }
}

fn register_builder_call<const N: usize, F>(
    registry: &mut HostRegistry,
    name: &str,
    arguments: [RuntimeType; N],
    callback: F,
) -> Result<(), teal::HostRegistryError>
where
    F: Fn(&[HostValue]) -> Result<HostValue, HostError> + Send + Sync + 'static,
{
    register_host(
        registry,
        "std.bench.syntax",
        name,
        signature(arguments, RuntimeType::None),
        "syntax.register",
        callback,
    )
}

fn signature<const N: usize>(
    arguments: [RuntimeType; N],
    return_type: RuntimeType,
) -> HostCallSignature {
    HostCallSignature::new(arguments, return_type)
        .expect("Static Teal Bench host signature is valid")
}

fn builder(
    state: &Arc<Mutex<GrammarState>>,
    handle: i32,
    action: impl FnOnce(&mut GrammarBuilder) -> Result<(), HostError>,
) -> Result<(), HostError> {
    let mut state = lock(state)?;
    let builder = state
        .builders
        .get_mut(&handle)
        .ok_or_else(|| host_error(format!("Unknown grammar handle {handle}")))?;
    action(builder)
}

fn lock<T>(mutex: &Mutex<T>) -> Result<std::sync::MutexGuard<'_, T>, HostError> {
    mutex
        .lock()
        .map_err(|_| host_error("Teal Bench host state lock is poisoned"))
}

fn int_arg(arguments: &[HostValue], index: usize) -> Result<i32, HostError> {
    match arguments.get(index) {
        Some(HostValue::Int(value)) => Ok(*value),
        _ => Err(host_error(format!("Parameter {index} must be int"))),
    }
}

fn string_arg(arguments: &[HostValue], index: usize) -> Result<String, HostError> {
    match arguments.get(index) {
        Some(HostValue::String(value)) => Ok(value.clone()),
        _ => Err(host_error(format!("Parameter {index} must be string"))),
    }
}

fn host_error(message: impl Into<String>) -> HostError {
    HostError::new(std::io::ErrorKind::InvalidData, message)
}

/// 返回所有随附语言文法包的 Teal 源码。
#[must_use]
pub fn builtin_grammar_scripts() -> Vec<(String, String)> {
    let languages = [
        (
            "rust",
            &["rs"][..],
            "fn|let|mut|pub|impl|trait|struct|enum|match|if|else|for|while|loop|return|use|mod|async|await|move|where|const|static|type|crate|self|Self",
        ),
        (
            "teal",
            &["tl", "teal"][..],
            "fn|let|static|pub|impl|trait|class|enum|match|if|else|for|in|loop|return|import|dylib|host|as|true|false",
        ),
        ("markdown", &["md", "markdown"][..], ""),
        ("json", &["json"][..], "true|false|null"),
        ("toml", &["toml"][..], "true|false"),
        (
            "c",
            &["c", "h"][..],
            "if|else|for|while|return|struct|enum|typedef|const|static|void|int|char|float|double|unsigned|signed|sizeof",
        ),
        (
            "cpp",
            &["cc", "cpp", "cxx", "hpp", "hh"][..],
            "if|else|for|while|return|struct|class|enum|template|typename|namespace|using|const|static|void|int|char|float|double|auto|constexpr|public|private|protected",
        ),
        (
            "python",
            &["py", "pyi"][..],
            "def|class|if|elif|else|for|while|return|import|from|as|async|await|with|try|except|finally|raise|yield|lambda|True|False|None",
        ),
        (
            "javascript",
            &["js", "mjs", "cjs"][..],
            "function|const|let|var|class|if|else|for|while|return|import|export|from|async|await|try|catch|finally|throw|new|this|true|false|null|undefined",
        ),
        (
            "typescript",
            &["ts", "tsx", "mts", "cts"][..],
            "function|const|let|var|class|interface|type|enum|namespace|if|else|for|while|return|import|export|from|async|await|try|catch|finally|throw|new|this|true|false|null|undefined|implements|extends|public|private|protected",
        ),
        (
            "go",
            &["go"][..],
            "package|import|func|type|struct|interface|map|chan|go|defer|select|case|if|else|for|range|return|var|const|true|false|nil",
        ),
    ];
    languages
        .into_iter()
        .map(|(language, extensions, keywords)| {
            (
                language.to_owned(),
                grammar_script(language, extensions, keywords),
            )
        })
        .collect()
}

fn grammar_script(language: &str, extensions: &[&str], keywords: &str) -> String {
    let mut body = String::new();
    for extension in extensions {
        writeln!(body, "syntax::extension(g, {});", literal(extension))
            .expect("Writing to string does not fail");
    }
    body.push_str("syntax::skip(g, \"whitespace\", \"\\\\s+\", 0);\n");
    if !keywords.is_empty() {
        writeln!(
            body,
            "syntax::token(g, \"keyword\", {}, 20);",
            literal(&format!(r"\b(?:{keywords})\b"))
        )
        .expect("Writing to string does not fail");
    }
    body.push_str(
        r#"
        syntax::token(g, "comment", "//[^\r\n]*|#[^\r\n]*", 15);
        syntax::token(g, "string", "\"([^\"\\\\]|\\\\.)*\"|'([^'\\\\]|\\\\.)*'", 10);
        syntax::token(g, "number", "[0-9]+(\\.[0-9]+)?", 5);
        syntax::token(g, "identifier", "[A-Za-z_][A-Za-z0-9_]*", 1);
        syntax::token(g, "punctuation", "[{{}}\\[\\]().,;:+*/=<>!?&|~-]+", 0);
        syntax::token(g, "other", "(?s).", -10);
        syntax::production(g, "source", "items", "source");
        syntax::production(g, "items", "items item", "items");
        syntax::production(g, "items", "item", "items");
        "#,
    );
    for terminal in [
        "keyword",
        "comment",
        "string",
        "number",
        "identifier",
        "punctuation",
        "other",
    ] {
        if terminal == "keyword" && keywords.is_empty() {
            continue;
        }
        writeln!(
            body,
            "syntax::production(g, \"item\", \"{terminal}\", \"item\");"
        )
        .expect("Writing to string does not fail");
    }
    for (kind, capture) in [
        ("keyword", "keyword"),
        ("comment", "comment"),
        ("string", "string"),
        ("number", "number"),
        ("identifier", "variable"),
        ("punctuation", "punctuation"),
    ] {
        if kind == "keyword" && keywords.is_empty() {
            continue;
        }
        writeln!(body, "syntax::capture(g, \"{kind}\", \"{capture}\");")
            .expect("Writing to string does not fail");
    }
    if language == "markdown" {
        body.push_str("syntax::injection(g, \"string\", \"code-fence\");\n");
    }
    body.push_str("syntax::start(g, \"source\");\n_ = syntax::finish(g);\n");
    format!(
        r#"
        host "std.bench.syntax" as syntax {{
            fn begin(arg0: string) -> int;
            fn extension(arg0: int, arg1: string);
            fn token(arg0: int, arg1: string, arg2: string, arg3: int);
            fn skip(arg0: int, arg1: string, arg2: string, arg3: int);
            fn production(arg0: int, arg1: string, arg2: string, arg3: string);
            fn start(arg0: int, arg1: string);
            fn capture(arg0: int, arg1: string, arg2: string);
            fn injection(arg0: int, arg1: string, arg2: string);
            fn finish(arg0: int) -> string;
        }}
         fn main() {{
            let g = syntax::begin({});
            {body}
        }}
        "#,
        literal(language)
    )
}

fn literal(value: &str) -> String {
    serde_json::to_string(value).expect("JSON strings are valid Teal string literals")
}
