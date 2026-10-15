//! 当前主机的 Teal 可执行文件、运行时资源、完整性清单与归档组装。

use std::{
    ffi::OsString,
    fs::{self, File},
    io::Read,
    path::{Component, Path, PathBuf},
    process::Command,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// 分发资源清单的固定文件名；源资源不得占用该名称。
const MANIFEST_NAME: &str = "manifest.json";
/// 当前资源清单结构版本。
const MANIFEST_FORMAT_VERSION: u32 = 1;

/// 发行版产品二进制及其 Cargo 所属 package。
pub(crate) const PRODUCTS: &[(&str, &str)] = &[
    ("teal", "teal"),
    ("teal-workbench-services", "teal-run"),
    ("teal-bench", "teal-perm"),
    ("teal-bench", "teal-config"),
    ("teal-bench", "teal-bench"),
    ("teal-workbench-automation", "teal-knowledge"),
];

/// `teal-lang` 的独立命令；只有显式传入语言仓库时才进入工作台发行包。
const LANGUAGE_PRODUCTS: &[(&str, &str)] = &[
    ("teal-lang", "teal-fmt"),
    ("teal-lang", "teal-lsp"),
    ("teal-lang", "teal-dap"),
    ("teal-runtime", "teal-c"),
    ("teal-runtime", "teal-test"),
    ("teal-model", "teal-auth"),
    ("teal-model", "teal-model"),
    ("teal-security", "teal-audit"),
];

/// 执行当前主机的 release 构建并组装目录与平台归档。
pub fn run(arguments: Vec<String>) -> Result<(), Box<dyn std::error::Error>> {
    let language_root = parse_language_root(&arguments)?;
    let workspace = workspace_root();
    let host = host_triple()?;
    let bundle_name = format!("teal-{}-{host}", env!("CARGO_PKG_VERSION"));
    let build_root = workspace.join("target/dist-build");
    let dist_root = workspace.join("target/dist");
    // 安全判断使用规范路径；传给 Cargo 和原生构建脚本的路径保留普通 Windows 形式。
    checked_target_path(&workspace, &build_root)?;
    checked_target_path(&workspace, &dist_root)?;
    build_products(&workspace, &build_root, language_root.as_deref())?;
    let runtime_source = if let Some(language_root) = &language_root {
        merge_runtime(&workspace.join("runtime"), language_root, &build_root)?
    } else {
        workspace.join("runtime")
    };
    let bundle = assemble_distribution(
        &runtime_source,
        &build_root.join("release"),
        &dist_root,
        &bundle_name,
    )?;
    if language_root.is_some() {
        for (_, binary) in LANGUAGE_PRODUCTS {
            copy_binary(&build_root.join("release"), &bundle, binary, binary)?;
        }
    }
    let archive = create_platform_archive(&dist_root, &bundle, &bundle_name)?;
    println!("Teal distribution directory: {}", bundle.display());
    println!("Teal distribution archive: {}", archive.display());
    Ok(())
}

fn parse_language_root(
    arguments: &[String],
) -> Result<Option<PathBuf>, Box<dyn std::error::Error>> {
    match arguments {
        [] => Ok(None),
        [option, path] if option == "--language-root" => {
            let path = PathBuf::from(path).canonicalize()?;
            if !path.join("Cargo.toml").is_file() {
                return Err(format!("language root has no Cargo.toml: {}", path.display()).into());
            }
            Ok(Some(path))
        }
        _ => Err("Usage: cargo xtask dist [--language-root PATH]".into()),
    }
}

/// 返回工作区根目录，不依赖调用命令时的当前目录。
fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("xtask must be located in the tools directory of the workspace")
        .to_owned()
}

/// 解析最近的已存在祖先，防止符号链接把输出重定向到 `target` 外。
fn resolve_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().ok_or("path must have a parent directory")?;
    let name = path.file_name().ok_or("path must have a file name")?;
    Ok(resolve_path(parent)?.join(name))
}

/// 返回严格位于工作区 `target` 内的规范输出路径。
fn checked_target_path(
    workspace: &Path,
    path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err("distribution path cannot contain parent-directory traversal".into());
    }
    let target = resolve_path(&workspace.join("target"))?;
    let normalized = resolve_path(path)?;
    if normalized == target || !normalized.starts_with(&target) {
        return Err(format!(
            "distribution output must be strictly inside target: {}",
            normalized.display()
        )
        .into());
    }
    Ok(normalized)
}

/// 查询当前 Rust 主机三元组，用于区分平台发行产物。
fn host_triple() -> Result<String, Box<dyn std::error::Error>> {
    let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| OsString::from("rustc"));
    let output = Command::new(rustc).arg("-vV").output()?;
    if !output.status.success() {
        return Err(format!(
            "Unable to query Rust host triplet: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    String::from_utf8(output.stdout)?
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .map(str::to_owned)
        .ok_or_else(|| "rustc -vV does not return host".into())
}

/// 使用锁文件构建显式产品清单中的 release 二进制。
fn build_products(
    workspace: &Path,
    build_root: &Path,
    language_root: Option<&Path>,
) -> Result<(), Box<dyn std::error::Error>> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    if let Some(language_root) = language_root {
        verify_language_checkout(workspace, language_root)?;
    }
    let build =
        |root: &Path, products: &[(&str, &str)]| -> Result<(), Box<dyn std::error::Error>> {
            for (package, binary) in products {
                let mut command = Command::new(&cargo);
                command
                    .current_dir(root)
                    .args(["build", "--release", "--locked", "--target-dir"])
                    .arg(build_root)
                    .args(["--package", package, "--bin", binary]);
                if *package == "teal-workbench-services" {
                    command.args(["--features", "tools"]);
                }
                let status = command.status()?;
                if !status.success() {
                    return Err(
                        format!("release build failed for {package}/{binary}: {status}").into(),
                    );
                }
            }
            Ok(())
        };
    build(workspace, PRODUCTS)?;
    if let Some(language_root) = language_root {
        build(language_root, LANGUAGE_PRODUCTS)?;
    }
    Ok(())
}

fn verify_language_checkout(
    workspace: &Path,
    language_root: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let status = Command::new("git")
        .args(["status", "--porcelain", "--untracked-files=all"])
        .current_dir(language_root)
        .output()?;
    if !status.status.success() || !status.stdout.is_empty() {
        return Err(format!(
            "language checkout must be clean: {}",
            language_root.display()
        )
        .into());
    }
    let head = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(language_root)
            .output()?
            .stdout,
    )?
    .trim()
    .to_owned();
    let output = Command::new("cargo")
        .args(["metadata", "--locked", "--format-version", "1"])
        .current_dir(workspace)
        .output()?;
    if !output.status.success() {
        return Err("cargo metadata could not resolve the locked teal-lang revision".into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let source = metadata["packages"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|package| package["name"] == "teal-runtime")
        .and_then(|package| package["source"].as_str())
        .ok_or("cargo metadata did not report teal-runtime's Git source")?;
    let locked_revision = source
        .rsplit_once('#')
        .map(|(_, revision)| revision)
        .ok_or("Cargo source has no locked Git revision")?;
    if head != locked_revision {
        return Err(format!("language checkout is {head}, expected {locked_revision}").into());
    }
    Ok(())
}

fn merge_runtime(
    workbench_runtime: &Path,
    language_root: &Path,
    build_root: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let merged = build_root.join("runtime-merged");
    if merged.exists() {
        fs::remove_dir_all(&merged)?;
    }
    copy_runtime(workbench_runtime, &merged)?;
    let language_runtime = language_root.join("runtime/packages");
    let std_source = language_runtime.join("std");
    if !std_source.is_dir() {
        return Err(format!(
            "language runtime has no std package: {}",
            std_source.display()
        )
        .into());
    }
    copy_runtime(&std_source, &merged.join("packages/std"))?;
    fs::copy(
        language_runtime.join("catalog.toml"),
        merged.join("packages/catalog-core.toml"),
    )?;
    merge_catalogs(&merged.join("packages"))?;
    Ok(merged)
}

fn merge_catalogs(packages: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let core: toml::Value =
        toml::from_str(&fs::read_to_string(packages.join("catalog-core.toml"))?)?;
    let workbench: toml::Value = toml::from_str(&fs::read_to_string(
        packages.join("catalog-workbench.toml"),
    )?)?;
    let mut names = core["packages"].as_array().cloned().unwrap_or_default();
    names.extend(
        workbench["packages"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
    );
    names.sort_by(|left, right| left.as_str().cmp(&right.as_str()));
    names.dedup();
    let mut catalog = toml::map::Map::new();
    catalog.insert("format".into(), toml::Value::Integer(6));
    catalog.insert("packages".into(), toml::Value::Array(names));
    fs::write(
        packages.join("catalog.toml"),
        toml::to_string(&toml::Value::Table(catalog))?,
    )?;
    Ok(())
}

/// 资源清单根对象；字段名是稳定的分发接口。
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct RuntimeManifest {
    /// 清单结构版本。
    format_version: u32,
    /// 按规范相对路径排序的资源条目。
    files: Vec<RuntimeFile>,
}

/// 一项运行时资源的路径、长度和内容摘要。
#[derive(Debug, Deserialize, Serialize)]
struct RuntimeFile {
    /// 使用 `/` 的 UTF-8 相对路径。
    path: String,
    /// 文件字节数。
    size: u64,
    /// 小写十六进制 SHA-256。
    sha256: String,
}

/// 文件系统条目的允许类别；符号链接和特殊文件不进入分发。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum EntryKind {
    /// 普通目录。
    Directory,
    /// 普通文件。
    File,
}

/// 根据文件类型标志执行拒绝优先的资源分类。
fn classify_entry(
    is_symlink: bool,
    is_directory: bool,
    is_file: bool,
) -> Result<EntryKind, &'static str> {
    if is_symlink {
        Err("Runtime resources cannot contain symbolic links")
    } else if is_directory {
        Ok(EntryKind::Directory)
    } else if is_file {
        Ok(EntryKind::File)
    } else {
        Err("Runtime resources can only contain ordinary files and directories")
    }
}

/// 重建一个分发目录，复制工作台工具和完整资源树，并生成资源清单。
///
/// 官方知识只以构建期内嵌制品的形式随二进制分发，不再物化到 `runtime/knowledge`。
fn assemble_distribution(
    runtime_source: &Path,
    binaries_source: &Path,
    dist_root: &Path,
    bundle_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    validate_bundle_name(bundle_name)?;
    let bundle = dist_root.join(bundle_name);
    if bundle.parent() != Some(dist_root) {
        return Err(
            "The distribution directory must be a direct subdirectory of the dist root directory"
                .into(),
        );
    }
    recreate_bundle_directory(&bundle)?;
    for (_, binary) in PRODUCTS {
        copy_binary(binaries_source, &bundle, binary, binary)?;
    }

    let runtime_target = bundle.join("runtime");
    let mut files = copy_runtime(runtime_source, &runtime_target)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let manifest = RuntimeManifest {
        format_version: MANIFEST_FORMAT_VERSION,
        files,
    };
    let mut encoded = serde_json::to_vec_pretty(&manifest)?;
    encoded.push(b'\n');
    fs::write(runtime_target.join(MANIFEST_NAME), encoded)?;
    Ok(bundle)
}

/// 只重建普通目录；拒绝输出位置上的符号链接和其他文件类型。
fn recreate_bundle_directory(bundle: &Path) -> Result<(), Box<dyn std::error::Error>> {
    match fs::symlink_metadata(bundle) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(format!(
                "The distribution directory cannot be a symbolic link: {}",
                bundle.display()
            )
            .into());
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(bundle)?,
        Ok(_) => {
            return Err(format!(
                "distribution output is not a regular directory: {}",
                bundle.display()
            )
            .into());
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    fs::create_dir_all(bundle)?;
    Ok(())
}

/// 拒绝可能逃出 dist 根目录或产生歧义归档路径的产物名称。
fn validate_bundle_name(name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let path = Path::new(name);
    if name.is_empty()
        || path.is_absolute()
        || path.components().count() != 1
        || !matches!(path.components().next(), Some(Component::Normal(_)))
    {
        Err(format!("Invalid distribution name: {name:?}").into())
    } else {
        Ok(())
    }
}

/// 复制一个 release 二进制，并允许 CLI 在分发中使用较短名称。
fn copy_binary(
    source_root: &Path,
    destination_root: &Path,
    source_name: &str,
    destination_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let source = source_root.join(executable_name(source_name));
    if !source.is_file() {
        return Err(format!("Missing release binary: {}", source.display()).into());
    }
    fs::copy(
        source,
        destination_root.join(executable_name(destination_name)),
    )?;
    Ok(())
}

/// 为当前平台附加可执行文件后缀。
fn executable_name(stem: &str) -> OsString {
    let mut name = OsString::from(stem);
    name.push(std::env::consts::EXE_SUFFIX);
    name
}

/// 复制完整运行时资源树并返回稳定排序的内容清单。
fn copy_runtime(
    source: &Path,
    destination: &Path,
) -> Result<Vec<RuntimeFile>, Box<dyn std::error::Error>> {
    let metadata = fs::symlink_metadata(source)?;
    classify_entry(
        metadata.file_type().is_symlink(),
        metadata.is_dir(),
        metadata.is_file(),
    )
    .map_err(|message| format!("{}: {}", source.display(), message))?;
    if source.join(MANIFEST_NAME).exists() {
        return Err(format!(
            "The source runtime directory must not contain reserved files {MANIFEST_NAME}"
        )
        .into());
    }
    fs::create_dir_all(destination)?;
    let mut files = Vec::new();
    copy_runtime_directory(source, source, destination, &mut files)?;
    files.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(files)
}

/// 递归复制目录；每一级都按文件名排序并拒绝链接或特殊文件。
fn copy_runtime_directory(
    root: &Path,
    directory: &Path,
    destination: &Path,
    files: &mut Vec<RuntimeFile>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut entries = fs::read_dir(directory)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry.file_type()?;
        let kind = classify_entry(
            file_type.is_symlink(),
            file_type.is_dir(),
            file_type.is_file(),
        )
        .map_err(|message| format!("{}: {message}", entry.path().display()))?;
        let relative = entry.path().strip_prefix(root)?.to_owned();
        let target = destination.join(&relative);
        match kind {
            EntryKind::Directory => {
                fs::create_dir_all(&target)?;
                copy_runtime_directory(root, &entry.path(), destination, files)?;
            }
            EntryKind::File => {
                if let Some(parent) = target.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(entry.path(), &target)?;
                files.push(RuntimeFile {
                    path: portable_path(&relative)?,
                    size: fs::metadata(&target)?.len(),
                    sha256: sha256_file(&target)?,
                });
            }
        }
    }
    Ok(())
}

/// 将受控相对路径转换为使用 `/` 的 UTF-8 清单路径。
fn portable_path(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(value) => parts.push(
                value
                    .to_str()
                    .ok_or("Runtime resource paths must be UTF-8")?,
            ),
            _ => {
                return Err(format!(
                    "Runtime resource path is not a canonical relative path: {}",
                    path.display()
                )
                .into());
            }
        }
    }
    Ok(parts.join("/"))
}

/// 以流式读取计算文件的 SHA-256 小写十六进制摘要。
fn sha256_file(path: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

/// 收集一个目录下全部普通目录和文件，供平台归档器稳定遍历。
fn archive_entries(root: &Path) -> Result<Vec<(PathBuf, EntryKind)>, Box<dyn std::error::Error>> {
    let mut output = Vec::new();
    collect_archive_entries(root, root, &mut output)?;
    output.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(output)
}

/// 递归收集归档条目并重复执行文件类型安全检查。
fn collect_archive_entries(
    root: &Path,
    directory: &Path,
    output: &mut Vec<(PathBuf, EntryKind)>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let kind = classify_entry(
            file_type.is_symlink(),
            file_type.is_dir(),
            file_type.is_file(),
        )
        .map_err(|message| format!("{}: {message}", entry.path().display()))?;
        let relative = entry.path().strip_prefix(root)?.to_owned();
        output.push((relative, kind));
        if kind == EntryKind::Directory {
            collect_archive_entries(root, &entry.path(), output)?;
        }
    }
    Ok(())
}

/// 生成当前平台的分发归档。
#[cfg(windows)]
fn create_platform_archive(
    dist_root: &Path,
    bundle: &Path,
    bundle_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    create_zip_archive(dist_root, bundle, bundle_name)
}

/// 生成 Windows 使用的 zip 分发归档。
#[cfg_attr(
    all(not(windows), not(test)),
    expect(
        dead_code,
        reason = "Non-Windows distributions only generate tar.gz; zip writer is still covered by cross-format tests"
    )
)]
fn create_zip_archive(
    dist_root: &Path,
    bundle: &Path,
    bundle_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use zip::{CompressionMethod, DateTime, ZipWriter, write::SimpleFileOptions};

    let archive_path = dist_root.join(format!("{bundle_name}.zip"));
    if archive_path.exists() {
        fs::remove_file(&archive_path)?;
    }
    let mut archive = ZipWriter::new(File::create(&archive_path)?);
    let directory_options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Stored)
        .last_modified_time(DateTime::default())
        .unix_permissions(0o755);
    archive.add_directory(format!("{bundle_name}/"), directory_options)?;
    for (relative, kind) in archive_entries(bundle)? {
        let relative = portable_path(&relative)?;
        let name = format!("{bundle_name}/{relative}");
        match kind {
            EntryKind::Directory => archive.add_directory(format!("{name}/"), directory_options)?,
            EntryKind::File => {
                let file_options = SimpleFileOptions::default()
                    .compression_method(CompressionMethod::Deflated)
                    .last_modified_time(DateTime::default())
                    .unix_permissions(0o644);
                archive.start_file(name, file_options)?;
                std::io::copy(&mut File::open(bundle.join(relative))?, &mut archive)?;
            }
        }
    }
    archive.finish()?;
    Ok(archive_path)
}

/// 生成当前平台的分发归档。
#[cfg(not(windows))]
fn create_platform_archive(
    dist_root: &Path,
    bundle: &Path,
    bundle_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    create_tar_archive(dist_root, bundle, bundle_name)
}

/// 生成非 Windows 平台使用的 tar.gz 分发归档。
#[cfg_attr(
    all(windows, not(test)),
    expect(
        dead_code,
        reason = "Windows distribution only generates zip; tar writer still covered by cross-format tests"
    )
)]
fn create_tar_archive(
    dist_root: &Path,
    bundle: &Path,
    bundle_name: &str,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    use flate2::{Compression, GzBuilder};
    use tar::{Builder, HeaderMode};

    let archive_path = dist_root.join(format!("{bundle_name}.tar.gz"));
    if archive_path.exists() {
        fs::remove_file(&archive_path)?;
    }
    let encoder = GzBuilder::new()
        .mtime(0)
        .write(File::create(&archive_path)?, Compression::best());
    let mut archive = Builder::new(encoder);
    archive.mode(HeaderMode::Deterministic);
    archive.append_dir(bundle_name, bundle)?;
    for (relative, kind) in archive_entries(bundle)? {
        let name = Path::new(bundle_name).join(&relative);
        match kind {
            EntryKind::Directory => archive.append_dir(name, bundle.join(relative))?,
            EntryKind::File => archive.append_path_with_name(bundle.join(relative), name)?,
        }
    }
    archive.into_inner()?.finish()?;
    Ok(archive_path)
}

#[cfg(test)]
mod tests;
