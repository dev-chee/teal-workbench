//! 构建中文 API 单站点；产物只写入工作区的 `target` 子目录。

use std::{
    collections::BTreeSet,
    error::Error,
    fs, io,
    path::{Component, Path, PathBuf},
    process::Command,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use super::PRODUCTS;

/// 构建全部登记产品库的中文文档并组装发布目录。
///
/// 输出目录会重建，不得指向工作区、`target` 本身或内部构建缓存。
pub fn build(workspace: &Path, output: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let requested_build_root = workspace.join("target/rustdoc-build");
    let output = checked_target_path(workspace, output)?;
    let mut build_root = requested_build_root.clone();
    let mut normalized_build = checked_target_path(workspace, &build_root)?;
    if output.starts_with(&normalized_build) || normalized_build.starts_with(&output) {
        return Err(
            "Documentation output directory cannot overlap with internal build cache".into(),
        );
    }

    // 保留编译缓存，但清空旧文档，避免已删除的 API 页面留在发布产物中。
    if let Err(error) = recreate_directory(workspace, &build_root.join("doc")) {
        if !is_permission_denied(error.as_ref()) {
            return Err(error);
        }
        // Windows 可能保留来自上一次 rustdoc/索引器进程的文件删除句柄。
        // 不修改 ACL，也不复用不完整的旧目录；在同一受验证的 target 范围内
        // 使用一次性构建目录，避免把旧文档混入本次站点。
        build_root = fresh_build_root(workspace)?;
        normalized_build = checked_target_path(workspace, &build_root)?;
        eprintln!(
            "Unable to clean {}; using isolated rustdoc build root {}: {error}",
            requested_build_root.join("doc").display(),
            build_root.display()
        );
        recreate_directory(workspace, &build_root.join("doc"))?;
    }
    // Cargo 的原生依赖构建脚本可能不接受 Windows 扩展路径前缀；
    // 安全检查使用规范路径，传给 Cargo 的仍是原始绝对路径。
    let cargo_started = Instant::now();
    cargo_doc(workspace, &build_root)?;
    println!(
        "rustdoc stage cargo-doc: {:.3}s",
        cargo_started.elapsed().as_secs_f64()
    );
    let publish_started = Instant::now();
    recreate_directory(workspace, &output)?;
    copy_directory(&build_root.join("doc"), &output)?;
    for relative in html_pages(&output)? {
        decorate_page(&output.join(relative))?;
    }
    fs::write(output.join("index.html"), landing_page())?;
    fs::write(output.join(".nojekyll"), "")?;
    println!(
        "rustdoc stage site-publish: {:.3}s",
        publish_started.elapsed().as_secs_f64()
    );
    println!("Chinese API site: {}", output.join("index.html").display());
    Ok(())
}

/// 判断目录清理失败是否来自 Windows 的权限/删除句柄冲突。
fn is_permission_denied(error: &(dyn Error + 'static)) -> bool {
    error
        .downcast_ref::<io::Error>()
        .is_some_and(|error| error.kind() == io::ErrorKind::PermissionDenied)
}

/// 在已验证的 `target` 范围内生成不复用旧文件的一次性 rustdoc 构建目录。
///
/// 返回未带 Windows `\\?\` 前缀的绝对路径；Cargo 的部分原生构建脚本尚不接受
/// 扩展路径，但安全边界仍由 `checked_target_path` 负责验证。
fn fresh_build_root(workspace: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("system clock is before UNIX_EPOCH: {error}"))?
        .as_nanos();
    let base = workspace.join("target");
    for attempt in 0..16u8 {
        let candidate = base.join(format!(
            "rustdoc-build-run-{}-{stamp}-{attempt}",
            std::process::id()
        ));
        let normalized = checked_target_path(workspace, &candidate)?;
        if !normalized.exists() {
            return Ok(candidate);
        }
    }
    Err("Unable to allocate an isolated rustdoc build directory".into())
}

/// 在默认中文配置下运行所有产品库的文档示例测试。
pub fn test_doctests(workspace: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // 文档生成可能因为旧索引文件被 Windows 锁定而切换到一次性构建目录；
    // doctest 也使用新目录，避免重新打开同一份不可删除的旧缓存。
    let target = fresh_build_root(workspace)?;
    let mut command = cargo_command(workspace, &target);
    command.arg("test");
    add_product_packages(&mut command);
    command.args(["--all-features", "--doc"]);
    let started = Instant::now();
    run(&mut command, "Chinese document sample test")?;
    println!(
        "rustdoc stage doctest: {:.3}s",
        started.elapsed().as_secs_f64()
    );
    Ok(())
}

/// 生成库文档；不发布依赖项或工具二进制的文档。
fn cargo_doc(workspace: &Path, target: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let mut command = cargo_command(workspace, target);
    command.arg("doc");
    add_product_packages(&mut command);
    command.args(["--all-features", "--no-deps", "--lib"]);
    run(&mut command, "Chinese rustdoc build")
}

/// 使用独立缓存及严格文档检查，不再传入语言条件配置。
fn cargo_command(workspace: &Path, target: &Path) -> Command {
    let mut command = Command::new("cargo");
    command
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", target)
        .env("RUSTDOCFLAGS", "-D warnings -D missing-docs");
    command
}

/// 给 Cargo 命令附加站点中所有产品库。
fn add_product_packages(command: &mut Command) {
    for product in PRODUCTS {
        command.args(["--package", product.package]);
    }
}

/// 执行子进程并保留失败状态。
fn run(command: &mut Command, description: &str) -> Result<(), Box<dyn std::error::Error>> {
    let status = command.status()?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{description} failed: {status}").into())
    }
}

/// 解析最近的已存在祖先，确保符号链接不能绕过输出范围检查。
fn resolve_path(path: &Path) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().ok_or("Path must have a parent directory")?;
    let name = path.file_name().ok_or("path must have name")?;
    Ok(resolve_path(parent)?.join(name))
}

/// 返回严格位于 `target` 内的绝对路径；拒绝父目录跳转及根目录清理。
fn checked_target_path(
    workspace: &Path,
    path: &Path,
) -> Result<PathBuf, Box<dyn std::error::Error>> {
    if path
        .components()
        .any(|component| component == Component::ParentDir)
    {
        return Err("The document directory cannot contain parent directory jumps".into());
    }
    let target = resolve_path(&workspace.join("target"))?;
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        workspace.join(path)
    };
    let normalized = resolve_path(&absolute)?;
    if normalized == target || !normalized.starts_with(&target) {
        return Err(format!(
            "Refuse to clean non-target subdirectories: {}",
            normalized.display()
        )
        .into());
    }
    Ok(normalized)
}

/// 重建已验证范围内的文档目录，不删除工作区或整个构建缓存。
fn recreate_directory(workspace: &Path, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let normalized = checked_target_path(workspace, path)?;
    if normalized.exists() {
        fs::remove_dir_all(&normalized)?;
    }
    fs::create_dir_all(&normalized)?;
    Ok(())
}

/// 递归复制 rustdoc 的页面与静态资源，保留相对布局。
fn copy_directory(source: &Path, destination: &Path) -> Result<(), Box<dyn std::error::Error>> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_directory(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// 按稳定顺序返回全部 HTML 相对路径。
fn html_pages(root: &Path) -> Result<BTreeSet<PathBuf>, Box<dyn std::error::Error>> {
    let mut pages = BTreeSet::new();
    collect_html(root, root, &mut pages)?;
    Ok(pages)
}

/// 递归收集页面；静态脚本和样式不参与 HTML 后处理。
fn collect_html(
    root: &Path,
    directory: &Path,
    pages: &mut BTreeSet<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            collect_html(root, &entry.path(), pages)?;
        } else if entry.path().extension().and_then(std::ffi::OsStr::to_str) == Some("html") {
            pages.insert(entry.path().strip_prefix(root)?.to_owned());
        }
    }
    Ok(())
}

/// 标记正文语言；保留 rustdoc 原生控件，不注入语言切换或重写代码内容。
fn decorate_page(path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let html = fs::read_to_string(path)?;
    if !html.contains("<html lang=\"en\">") && !html.contains("<html lang=\"zh-CN\">") {
        return Err(format!(
            "Unable to recognize rustdoc page language tag: {}",
            path.display()
        )
        .into());
    }
    fs::write(
        path,
        html.replacen("<html lang=\"en\">", "<html lang=\"zh-CN\">", 1),
    )?;
    Ok(())
}

/// 提供无需脚本的中文入口；所有链接直接指向站点根目录下的库页面。
fn landing_page() -> String {
    let links = PRODUCTS
        .iter()
        .map(|product| {
            format!(
                "<li><a href=\"{}/index.html\">{}</a></li>",
                product.rustdoc_name, product.package
            )
        })
        .collect::<String>();
    format!(
        r#"<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Teal API Documentation</title>
</head>
<body><main><h1>Teal API Documentation</h1><p>Select a library:</p><ul>{links}</ul></main></body>
</html>
"#
    )
}

#[cfg(test)]
mod tests;
