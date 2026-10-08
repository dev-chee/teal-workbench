//! 遍历产品库的模块图，检查公开 API 的中文文档。

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use quote::ToTokens;
use syn::{
    Attribute, Expr, Fields, ImplItem, Item, Lit, Meta, TraitItem, Visibility,
    punctuated::Punctuated, token::Comma,
};

/// 文档工具检查的一个产品库。
#[derive(Clone, Copy)]
pub struct ProductCrate {
    /// Cargo 包名称。
    pub package: &'static str,
    /// rustdoc 输出目录使用的 Rust crate 名称。
    pub rustdoc_name: &'static str,
    /// 相对于仓库的库入口路径。
    pub root: &'static str,
}

/// 一个公开 API 对应的源码文档单元。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DocUnit {
    /// 稳定的源码条目标识。
    pub key: String,
    /// 相对于仓库的源文件路径。
    pub source: PathBuf,
    /// 默认构建使用的 Markdown 说明。
    pub markdown: String,
    /// 是否存在不允许的条件式文档文本。
    pub has_conditional_text: bool,
}

/// 源码审计的覆盖范围与文档条目。
pub struct Audit {
    /// 产品库中收集到的公开文档单元。
    pub units: Vec<DocUnit>,
    /// 从产品库模块图遍历到的文件。
    pub files: BTreeSet<PathBuf>,
}

/// 一个条目的文档片段；条件文本单独记录以便拒绝语言分支。
#[derive(Default)]
struct Documentation {
    /// 无条件提供的文档文本。
    parts: Vec<String>,
    /// 是否发现条件式文档。
    conditional: bool,
}

impl Documentation {
    /// 合并文档片段；无说明的重导出等条目交由 rustdoc 覆盖检查处理。
    fn into_unit(self, key: String, source: PathBuf) -> Option<DocUnit> {
        if self.parts.is_empty() && !self.conditional {
            return None;
        }
        Some(DocUnit {
            key,
            source,
            markdown: self.parts.join("\n").replace("\r\n", "\n"),
            has_conditional_text: self.conditional,
        })
    }
}

/// 从所有产品库的模块图收集公开文档，不遍历测试专用模块。
pub fn collect(
    workspace: &Path,
    products: &[ProductCrate],
) -> Result<Audit, Box<dyn std::error::Error>> {
    let mut collector = Collector {
        workspace,
        units: Vec::new(),
        files: BTreeSet::new(),
        duplicate_keys: BTreeMap::new(),
    };
    for product in products {
        let root = workspace.join(product.root);
        collector.visit_file(product.package, &root, product.package, true)?;
    }
    collector
        .units
        .sort_by(|left, right| left.key.cmp(&right.key));
    Ok(Audit {
        units: collector.units,
        files: collector.files,
    })
}

/// 检查说明包含中文且无需条件配置；缺失公开文档由严格 rustdoc 构建拒绝。
pub fn validate(audit: &Audit) -> Result<(), Box<dyn std::error::Error>> {
    let mut errors = Vec::new();
    for unit in &audit.units {
        if unit.has_conditional_text {
            errors.push(format!(
                "{} ({}) contains conditional documentation; provide the Chinese description directly",
                unit.key,
                unit.source.display()
            ));
        }
        if !contains_chinese_prose(&unit.markdown) {
            errors.push(format!(
                "{} ({}) lacks a Chinese description",
                unit.key,
                unit.source.display()
            ));
        }
    }
    if errors.is_empty() {
        return Ok(());
    }
    Err(format!("Chinese rustdoc audit failed:\n{}", errors.join("\n")).into())
}

/// 检测代码围栏外的中文正文，避免仅凭示例字符串误判说明语言。
fn contains_chinese_prose(markdown: &str) -> bool {
    let mut in_fence = false;
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") || line.trim_start().starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence
            && line
                .chars()
                .any(|character| ('\u{4e00}'..='\u{9fff}').contains(&character))
        {
            return true;
        }
    }
    false
}

/// 保留遍历状态，去重源文件并为重名条目分配稳定标识。
struct Collector<'a> {
    /// 仓库根目录。
    workspace: &'a Path,
    /// 已收集的文档单元。
    units: Vec<DocUnit>,
    /// 已遍历的规范源文件路径。
    files: BTreeSet<PathBuf>,
    /// 每个条目标识的出现次数。
    duplicate_keys: BTreeMap<String, usize>,
}

impl Collector<'_> {
    /// 解析源文件，按需记录模块说明后继续遍历条目。
    fn visit_file(
        &mut self,
        package: &str,
        path: &Path,
        module: &str,
        document_module: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let canonical = path.canonicalize()?;
        if !self.files.insert(canonical.clone()) {
            return Ok(());
        }
        let source = fs::read_to_string(&canonical)?;
        let parsed = syn::parse_file(&source)
            .map_err(|error| format!("Unable to parse {}: {error}", canonical.display()))?;
        if document_module {
            self.record(format!("{module}::module"), &canonical, &parsed.attrs)?;
        }
        self.visit_items(package, &canonical, module, &parsed.items)
    }

    /// 遍历公开成员和子模块；重导出仅收集其显式说明。
    fn visit_items(
        &mut self,
        package: &str,
        source: &Path,
        module: &str,
        items: &[Item],
    ) -> Result<(), Box<dyn std::error::Error>> {
        for item in items {
            match item {
                Item::Const(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::const {}", item.ident),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Enum(item) if is_public(&item.vis) => {
                    let path = format!("{module}::enum {}", item.ident);
                    self.record(path.clone(), source, &item.attrs)?;
                    for variant in &item.variants {
                        let variant_path = format!("{path}::variant {}", variant.ident);
                        self.record(variant_path.clone(), source, &variant.attrs)?;
                        self.record_fields(source, &variant_path, &variant.fields, true)?;
                    }
                }
                Item::ExternCrate(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::extern {}", item.ident),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Fn(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::fn {}", item.sig.ident),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Macro(item) if item.attrs.iter().any(is_macro_export) => {
                    let name = item
                        .ident
                        .as_ref()
                        .map_or("anonymous".to_owned(), ToString::to_string);
                    self.record(format!("{module}::macro {name}"), source, &item.attrs)?;
                }
                Item::Mod(item) => {
                    let child_module = format!("{module}::{}", item.ident);
                    if is_public(&item.vis) {
                        self.record(format!("{child_module}::module"), source, &item.attrs)?;
                    }
                    if has_cfg_test(&item.attrs) {
                        continue;
                    }
                    if let Some((_, children)) = &item.content {
                        self.visit_items(package, source, &child_module, children)?;
                    } else {
                        let child = resolve_module_file(source, item)?;
                        self.visit_file(package, &child, &child_module, is_public(&item.vis))?;
                    }
                }
                Item::Static(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::static {}", item.ident),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Struct(item) if is_public(&item.vis) => {
                    let path = format!("{module}::struct {}", item.ident);
                    self.record(path.clone(), source, &item.attrs)?;
                    self.record_fields(source, &path, &item.fields, false)?;
                }
                Item::Trait(item) if is_public(&item.vis) => {
                    let path = format!("{module}::trait {}", item.ident);
                    self.record(path.clone(), source, &item.attrs)?;
                    for member in &item.items {
                        let (name, attrs) = match member {
                            TraitItem::Const(member) => {
                                (format!("const {}", member.ident), &member.attrs)
                            }
                            TraitItem::Fn(member) => {
                                (format!("fn {}", member.sig.ident), &member.attrs)
                            }
                            TraitItem::Type(member) => {
                                (format!("type {}", member.ident), &member.attrs)
                            }
                            TraitItem::Macro(member) => ("macro".to_owned(), &member.attrs),
                            _ => continue,
                        };
                        self.record(format!("{path}::{name}"), source, attrs)?;
                    }
                }
                Item::TraitAlias(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::trait alias {}", item.ident),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Type(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::type {}", item.ident),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Union(item) if is_public(&item.vis) => {
                    let path = format!("{module}::union {}", item.ident);
                    self.record(path.clone(), source, &item.attrs)?;
                    for field in &item.fields.named {
                        if is_public(&field.vis) {
                            let name = field.ident.as_ref().expect("Union fields must have names");
                            self.record(format!("{path}::field {name}"), source, &field.attrs)?;
                        }
                    }
                }
                Item::Use(item) if is_public(&item.vis) => {
                    self.record(
                        format!("{module}::use {}", item.tree.to_token_stream()),
                        source,
                        &item.attrs,
                    )?;
                }
                Item::Impl(item) => {
                    let self_type = item.self_ty.to_token_stream().to_string();
                    let trait_name = item
                        .trait_
                        .as_ref()
                        .map(|(_, path, _)| path.to_token_stream().to_string())
                        .unwrap_or_else(|| "inherent".to_owned());
                    let path = format!("{module}::impl {trait_name} for {self_type}");
                    for member in &item.items {
                        let (name, attrs, visible) = match member {
                            ImplItem::Const(member) => (
                                format!("const {}", member.ident),
                                &member.attrs,
                                is_public(&member.vis),
                            ),
                            ImplItem::Fn(member) => (
                                format!("fn {}", member.sig.ident),
                                &member.attrs,
                                is_public(&member.vis),
                            ),
                            ImplItem::Type(member) => (
                                format!("type {}", member.ident),
                                &member.attrs,
                                is_public(&member.vis),
                            ),
                            ImplItem::Macro(member) => ("macro".to_owned(), &member.attrs, false),
                            _ => continue,
                        };
                        if visible {
                            self.record(format!("{path}::{name}"), source, attrs)?;
                        }
                    }
                }
                _ => {}
            }
        }
        let _ = package;
        Ok(())
    }

    /// 收集公开字段；枚举变体的字段沿用枚举可见性。
    fn record_fields(
        &mut self,
        source: &Path,
        parent: &str,
        fields: &Fields,
        enum_fields: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        for (index, field) in fields.iter().enumerate() {
            if !enum_fields && !is_public(&field.vis) {
                continue;
            }
            let name = field
                .ident
                .as_ref()
                .map_or_else(|| index.to_string(), ToString::to_string);
            self.record(format!("{parent}::field {name}"), source, &field.attrs)?;
        }
        Ok(())
    }

    /// 将条目属性转为文档单元，不为无说明的条目虚构文本。
    fn record(
        &mut self,
        key: String,
        source: &Path,
        attributes: &[Attribute],
    ) -> Result<(), Box<dyn std::error::Error>> {
        let docs = documentation(attributes, source)?;
        if let Some(unit) = docs.into_unit(self.unique_key(key), self.relative(source)) {
            self.units.push(unit);
        }
        Ok(())
    }

    /// 通过递增后缀区分同名实现成员。
    fn unique_key(&mut self, key: String) -> String {
        let count = self.duplicate_keys.entry(key.clone()).or_default();
        *count += 1;
        if *count == 1 {
            key
        } else {
            format!("{key}#{}", *count)
        }
    }

    /// 去除工作区前缀，避免 Windows 规范路径泄漏到诊断中。
    fn relative(&self, path: &Path) -> PathBuf {
        let workspace = self
            .workspace
            .canonicalize()
            .unwrap_or_else(|_| self.workspace.to_owned());
        path.strip_prefix(workspace).unwrap_or(path).to_owned()
    }
}

/// 收集普通注释及包含文件，单独标记条件式文档以供审计拒绝。
fn documentation(
    attributes: &[Attribute],
    source: &Path,
) -> Result<Documentation, Box<dyn std::error::Error>> {
    let mut docs = Documentation::default();
    for attribute in attributes {
        if attribute.path().is_ident("doc") {
            if let Some(text) = doc_meta_text(&attribute.meta, source)? {
                docs.parts.push(text);
            }
            continue;
        }
        if !attribute.path().is_ident("cfg_attr") {
            continue;
        }
        let entries = attribute.parse_args_with(Punctuated::<Meta, Comma>::parse_terminated)?;
        let mut entries = entries.into_iter();
        let Some(_) = entries.next() else {
            continue;
        };
        for meta in entries {
            if doc_meta_text(&meta, source)?.is_some() {
                docs.conditional = true;
            }
        }
    }
    Ok(docs)
}

/// 读取字符串形式的文档属性；包含路径相对于当前源文件解析。
fn doc_meta_text(meta: &Meta, source: &Path) -> Result<Option<String>, Box<dyn std::error::Error>> {
    let Meta::NameValue(name_value) = meta else {
        return Ok(None);
    };
    if !name_value.path.is_ident("doc") {
        return Ok(None);
    }
    match &name_value.value {
        Expr::Lit(expression) => match &expression.lit {
            Lit::Str(text) => Ok(Some(text.value())),
            _ => Ok(None),
        },
        Expr::Macro(expression) if expression.mac.path.is_ident("include_str") => {
            let path = syn::parse2::<syn::LitStr>(expression.mac.tokens.clone())?.value();
            let included = source
                .parent()
                .expect("Source files must have a parent directory")
                .join(path);
            Ok(Some(fs::read_to_string(included)?))
        }
        _ => Ok(None),
    }
}

/// 只把无约束的 `pub` 视为公开 API。
fn is_public(visibility: &Visibility) -> bool {
    matches!(visibility, Visibility::Public(_))
}

/// 识别导出宏属性。
fn is_macro_export(attribute: &Attribute) -> bool {
    attribute.path().is_ident("macro_export")
}

/// 识别仅在测试构建中加载的模块。
fn has_cfg_test(attributes: &[Attribute]) -> bool {
    attributes.iter().any(|attribute| {
        attribute.path().is_ident("cfg")
            && attribute
                .meta
                .to_token_stream()
                .to_string()
                .replace(' ', "")
                == "cfg(test)"
    })
}

/// 遵循显式路径或 Rust 常规文件布局解析外部模块。
fn resolve_module_file(
    source: &Path,
    module: &syn::ItemMod,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if let Some(path) = path_attribute(&module.attrs)? {
        return Ok(source
            .parent()
            .expect("Source files must have a parent directory")
            .join(path));
    }
    let parent = source
        .parent()
        .expect("Source files must have a parent directory");
    let stem = source
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or_default();
    let module_directory = if matches!(stem, "lib" | "mod") {
        parent.to_owned()
    } else {
        parent.join(stem)
    };
    let direct = module_directory.join(format!("{}.rs", module.ident));
    if direct.is_file() {
        return Ok(direct);
    }
    let nested = module_directory
        .join(module.ident.to_string())
        .join("mod.rs");
    if nested.is_file() {
        return Ok(nested);
    }
    Err(format!(
        "Unable to resolve module {}, declaration at {}",
        module.ident,
        source.display()
    )
    .into())
}

/// 提取模块的 `path` 属性；不存在时使用常规文件布局。
fn path_attribute(attributes: &[Attribute]) -> Result<Option<String>, syn::Error> {
    for attribute in attributes {
        if !attribute.path().is_ident("path") {
            continue;
        }
        if let Meta::NameValue(value) = &attribute.meta
            && let Expr::Lit(expression) = &value.value
            && let Lit::Str(path) = &expression.lit
        {
            return Ok(Some(path.value()));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests;
