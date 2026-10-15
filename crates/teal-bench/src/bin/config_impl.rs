//! `teal config` 执行边界：分层展示与受控写入。
//!
//! 命令本身不做分层、合并或存盘决策；这些规则全部来自 `teal-config`。本模块只负责把命令行
//! 输入翻译成类别路径、把结果渲染到终端，并在写入工作区层之前确认信任。

use crate::ConfigCommand;
use teal_config::{
    ConfigCategory, ConfigDocument, ConfigError, ConfigFile, ConfigPaths, ConfigScope,
    FileConfigSource,
};

/// 执行一次 `teal config` 调用。
pub(super) fn run(command: ConfigCommand) -> Result<(), String> {
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let paths = ConfigPaths::discover(Some(&workspace)).map_err(|error| error.to_string())?;
    let source = FileConfigSource::new(paths.clone());
    match command {
        ConfigCommand::List { file, layer, json } => list(&paths, file, layer, json),
        ConfigCommand::Get {
            file,
            key,
            layer,
            json,
        } => get(&paths, &source, file, &key, layer, json),
        ConfigCommand::Set {
            file,
            key,
            value,
            layer,
            json,
        } => set(&source, file, &key, &value, layer, json),
        ConfigCommand::Unset { file, key, layer } => unset(&source, file, &key, layer),
        ConfigCommand::Paths { json } => {
            print_paths(&paths, json);
            Ok(())
        }
    }
}

fn list(
    paths: &ConfigPaths,
    file: Option<ConfigFile>,
    layer: Option<ConfigScope>,
    json: bool,
) -> Result<(), String> {
    let effective =
        teal_security::load_declarations_paths(paths.clone()).map_err(|error| error.to_string())?;
    let files = file.map_or_else(|| ConfigFile::ALL.to_vec(), |file| vec![file]);
    let mut output = serde_json::Map::new();
    for file in files {
        let loaded = effective
            .load_file(file)
            .map_err(|error| error.to_string())?;
        let mut entries = serde_json::Map::new();
        if let Some(layer) = layer {
            let Some(value) = loaded
                .scopes()
                .iter()
                .find(|value| value.scope() == layer)
                .map(teal_config::ScopeValue::value)
            else {
                continue;
            };
            for (key, value) in teal_config::flatten(value) {
                entries.insert(
                    key,
                    serde_json::json!({"layer": layer.name(), "value": value}),
                );
            }
        } else {
            for (key, value) in teal_config::flatten(loaded.value()) {
                let origin = loaded.origin(&key).map_or_else(
                    || "default".to_owned(),
                    |origin| origin.scope.name().to_owned(),
                );
                entries.insert(key, serde_json::json!({"layer": origin, "value": value}));
            }
        }
        output.insert(file.name().to_owned(), serde_json::Value::Object(entries));
    }
    if json {
        println!(
            "{}",
            crate::output::json("config.list", serde_json::Value::Object(output))
                .map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    for (file, entries) in &output {
        let Some(entries) = entries.as_object() else {
            continue;
        };
        for (key, entry) in entries {
            let layer = entry
                .get("layer")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("default");
            let value = entry
                .get("value")
                .map_or_else(|| "".to_owned(), render_json);
            println!("{file}.{key} = {value}\t[{layer}]");
        }
    }
    Ok(())
}

fn get(
    paths: &ConfigPaths,
    source: &FileConfigSource,
    file: ConfigFile,
    key: &str,
    layer: Option<ConfigScope>,
    json: bool,
) -> Result<(), String> {
    let effective =
        teal_security::load_declarations_paths(paths.clone()).map_err(|error| error.to_string())?;
    let loaded = effective
        .load_file(file)
        .map_err(|error| error.to_string())?;
    let (value, origin) = match layer {
        Some(layer) => {
            let value = loaded
                .scopes()
                .iter()
                .find(|value| value.scope() == layer)
                .and_then(|value| teal_config::value_at_path(value.value(), key))
                .ok_or_else(|| {
                    format!(
                        "`{}.{key}` is not set in the {} layer",
                        file.name(),
                        layer.name()
                    )
                })?;
            (value, layer.name())
        }
        None => {
            let value = teal_config::value_at_path(loaded.value(), key).ok_or_else(|| {
                format!(
                    "`{}.{key}` is not set in the effective configuration",
                    file.name()
                )
            })?;
            let origin = loaded
                .origin(key)
                .map_or("default", |origin| origin.scope.name());
            (value, origin)
        }
    };
    if json {
        println!(
            "{}",
            crate::output::json(
                "config.get",
                serde_json::json!({
                    "file": file.name(),
                    "key": key,
                    "layer": origin,
                    "value": value,
                })
            )
            .map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    println!("{}.{key} = {}\t[{origin}]", file.name(), render_toml(value));
    Ok(())
}

fn set(
    source: &FileConfigSource,
    file: ConfigFile,
    key: &str,
    raw: &str,
    layer: ConfigScope,
    json: bool,
) -> Result<(), String> {
    require_available(file)?;
    let value = parse_value(raw, json)?;
    if layer == ConfigScope::Workspace {
        crate::require_workspace_trust()?;
    }
    let update = value.clone();
    teal_config::edit_value_with_observer(
        source,
        layer,
        file,
        None,
        allowed_paths(file, layer),
        |document| {
            teal_config::set_value_at_path(document, key, update)?;
            validate_document(file, document)
        },
        &teal_security::process_context(),
    )
    .map_err(|error| error.to_string())?;
    println!("Set `{}.{key}` in the {} layer.", file.name(), layer.name());
    Ok(())
}

fn unset(
    source: &FileConfigSource,
    file: ConfigFile,
    key: &str,
    layer: ConfigScope,
) -> Result<(), String> {
    require_available(file)?;
    if layer == ConfigScope::Workspace {
        crate::require_workspace_trust()?;
    }
    let existing =
        teal_config::read_scope(source, layer, file).map_err(|error| error.to_string())?;
    let present = existing
        .as_ref()
        .and_then(|value| teal_config::value_at_path(value, key))
        .is_some();
    if !present {
        return Err(format!(
            "`{}.{key}` is not set in the {} layer",
            file.name(),
            layer.name()
        ));
    }
    teal_config::edit_value_with_observer(
        source,
        layer,
        file,
        None,
        allowed_paths(file, layer),
        |document| {
            teal_config::remove_value_at_path(document, key)?;
            validate_document(file, document)
        },
        &teal_security::process_context(),
    )
    .map_err(|error| error.to_string())?;
    println!(
        "Removed `{}.{key}` from the {} layer.",
        file.name(),
        layer.name()
    );
    Ok(())
}

fn print_paths(paths: &ConfigPaths, json: bool) {
    let mut entries = Vec::new();
    for (layer, file, path) in paths.all_files() {
        entries.push(serde_json::json!({
            "layer": layer.name(),
            "file": file.name(),
            "path": path.to_string_lossy(),
            "present": path.exists(),
        }));
    }
    for layer in [ConfigScope::User, ConfigScope::Workspace] {
        if let Some(path) = paths.env_path(layer) {
            entries.push(serde_json::json!({
                "layer": layer.name(),
                "file": "env",
                "path": path.to_string_lossy(),
                "present": path.exists(),
            }));
        }
    }
    if json {
        println!(
            "{}",
            crate::output::json("config.paths", serde_json::Value::Array(entries))
                .unwrap_or_else(|_| "[]".to_owned())
        );
        return;
    }
    for entry in entries {
        let state = if entry["present"].as_bool() == Some(true) {
            "present"
        } else {
            "missing"
        };
        println!(
            "{}\t{}\t{}\t{}",
            entry["layer"].as_str().unwrap_or(""),
            entry["file"].as_str().unwrap_or(""),
            state,
            entry["path"].as_str().unwrap_or("")
        );
    }
}

fn render_json(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 文本输出使用的简写形式：字符串不带引号，其它取值保持 TOML 字面量。
fn render_toml(value: &toml::Value) -> String {
    match value {
        toml::Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// 把命令行取值解析为 TOML 值；未加 `--json` 时按字面量类型推断，无法解析的文本按字符串处理。
fn parse_value(raw: &str, json: bool) -> Result<toml::Value, String> {
    if json {
        let parsed: serde_json::Value = serde_json::from_str(raw)
            .map_err(|error| format!("value is not valid JSON: {error}"))?;
        return toml::Value::try_from(parsed)
            .map_err(|error| format!("value cannot be represented in TOML: {error}"));
    }
    let wrapped = format!("value = {raw}");
    match toml::from_str::<toml::Value>(&wrapped) {
        Ok(mut table) => Ok(table
            .as_table_mut()
            .and_then(|table| table.remove("value"))
            .unwrap_or_else(|| toml::Value::String(raw.to_owned()))),
        Err(_) => Ok(toml::Value::String(raw.to_owned())),
    }
}

fn require_available(_file: ConfigFile) -> Result<(), String> {
    Ok(())
}

/// 返回某个类别文件在某一层允许出现的键路径。
fn allowed_paths(file: ConfigFile, layer: ConfigScope) -> &'static [&'static str] {
    match file {
        ConfigFile::Config => <teal_security::TealConfig as ConfigCategory>::allowed_paths(layer),
        ConfigFile::Model => <teal_model::ModelDocument as ConfigCategory>::allowed_paths(layer),
        ConfigFile::Bench => bench_allowed_paths(layer),
        ConfigFile::Runtime => <teal::RuntimeDocument as ConfigCategory>::allowed_paths(layer),
        ConfigFile::Observability => teal_config::ALLOW_ALL,
        ConfigFile::Agent => teal_config::ALLOW_ALL,
        ConfigFile::Lint => <teal::LintDefaults as ConfigCategory>::allowed_paths(layer),
        ConfigFile::Compiler => <teal::CompilerDocument as ConfigCategory>::allowed_paths(layer),
    }
}

fn bench_allowed_paths(layer: ConfigScope) -> &'static [&'static str] {
    <teal_bench::config::BenchDocument as ConfigCategory>::allowed_paths(layer)
}

/// 用所属类别的文档类型校验一份候选内容。
fn validate_document(file: ConfigFile, value: &toml::Value) -> Result<(), ConfigError> {
    match file {
        ConfigFile::Config => validate::<teal_security::TealConfig>(value),
        ConfigFile::Model => validate::<teal_model::ModelDocument>(value),
        ConfigFile::Bench => validate::<teal_bench::config::BenchDocument>(value),
        ConfigFile::Runtime => validate::<teal::RuntimeDocument>(value),
        ConfigFile::Observability => teal::validate_configuration_document(file, value)
            .map_err(|message| ConfigError::invalid(file, message)),
        ConfigFile::Agent => teal::validate_configuration_document(file, value)
            .map_err(|message| ConfigError::invalid(file, message)),
        ConfigFile::Lint => validate::<teal::LintDefaults>(value),
        ConfigFile::Compiler => validate::<teal::CompilerDocument>(value),
    }
}

fn validate<C: ConfigCategory>(value: &toml::Value) -> Result<(), ConfigError> {
    let document = teal_config::parse_document::<C::Document>(value, C::FILE)?;
    ConfigDocument::validate(&document).map_err(|message| ConfigError::invalid(C::FILE, message))
}
