@edition("2026");

// Tier Host：在调用方授予的临时父目录中覆盖完整文件与 SHA-256 接口。
// Capability 只允许操作类别，read/write 资源范围仍会在每次副作用前独立检查。
import "std/fs.tl" as osfs;
import "std/crypto.tl" as crypto;
import "std/error.tl" as errors;

task main() { workflow().await; }

task workflow() {
    let arguments = argv();
    if arguments.len() != 1 { throw(errors::make("usage", "Pass a new scratch directory path", errors::ErrorOrigin.user, false)); }
    let root = arguments[0];
    // 不覆盖已有目录；调用方授权其已存在父目录，脚本只使用这个新子目录。
    if osfs::exists_or_throw(root).await { throw(errors::make("filesystem", "Scratch directory already exists", errors::ErrorOrigin.user, false)); }
    osfs::create_dir_all_or_throw(root).await;
    let input = root + "/input.txt";
    let copy = root + "/copy.txt";
    osfs::write_or_throw(input, "hello").await;
    osfs::append_or_throw(input, " world").await;
    print(osfs::read_or_throw(input).await);
    _ = osfs::copy_or_throw(input, copy, false).await;
    let canonical = osfs::canonicalize_or_throw(copy).await;
    print(canonical.ends_with("copy.txt"));
    print(osfs::metadata_or_throw(copy).await.size);
    print(osfs::list_or_throw(root, false).await.len());
    let tail = osfs::read_tail_or_throw(copy, 5).await;
    // tail 的摘要只绑定这份尾部快照，不等同于完整文件摘要。
    print(tail.text, tail.offset, tail.source_length, tail.snapshot_sha256 == crypto::sha256_file(copy).await.sha256);
    print(crypto::sha256_file(copy).await.sha256);
    osfs::delete_or_throw(input).await;
    osfs::delete_or_throw(copy).await;
    osfs::delete_dir_or_throw(root).await;
    print("scratch removed");
}
