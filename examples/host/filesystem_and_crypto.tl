@edition("2026");

// Tier Host：在调用方授予的临时父目录中覆盖完整文件与 SHA-256 接口。
// Permission 只允许操作类别，read/write 资源范围仍会在每次副作用前独立检查。
import "std/fs.tl" as osfs;
import "std/crypto.tl" as crypto;
task application() -> none | Error {
    workflow().await ?;
}

task workflow() -> none | Error {
    let arguments = argv();
    if arguments.len() != 1 {
        return Error::make("usage", "Pass a new scratch directory path", ErrorOrigin.user, false);
    }
    let root = arguments[0];
    // 不覆盖已有目录；调用方授权其已存在父目录，脚本只使用这个新子目录。
    if osfs::exists(root).await ? {
        return Error::make("filesystem", "Scratch directory already exists", ErrorOrigin.user, false);
    }
    osfs::create_dir_all(root).await ?;
    let input = root + "/input.txt";
    let copy = root + "/copy.txt";
    osfs::write(input, "hello").await ?;
    osfs::append(input, " world").await ?;
    print(osfs::read(input).await ?);
    _ = osfs::copy(input, copy, false).await ?;
    let canonical = osfs::canonicalize(copy).await ?;
    print(canonical.ends_with("copy.txt"));
    print(osfs::metadata(copy).await ?.size);
    print(osfs::list(root, false).await ?.len());
    let tail = osfs::read_tail(copy, 5).await ?;
    // tail 的摘要只绑定这份尾部快照，不等同于完整文件摘要。
    print(tail.text, tail.offset, tail.source_length, tail.snapshot_sha256 == crypto::sha256_file(copy).await.sha256);
    print(crypto::sha256_file(copy).await.sha256);
    osfs::delete(input).await ?;
    osfs::delete(copy).await ?;
    osfs::delete_dir(root).await ?;
    print("scratch removed");
}

/// 入口明确处理失败；未知外部效果仍由 VM 终止。
task main() -> int {
    let result = application().await;
    match result {
        Error { .. } => {
            print(result.message);
            return 1;
        } _ => {
            return 0;
        }
    }
}
