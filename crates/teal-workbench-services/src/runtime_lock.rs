//! 项目运行时锁与只写入用户缓存的制品安装。

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

const LOCK_VERSION: u16 = 8;
const PACKAGE_FORMAT_VERSION: u16 = 5;
const MAX_LOCK_BYTES: u64 = 1024 * 1024;

/// 在解析具体版本字段前使用的最小锁文件头。
#[derive(Deserialize)]
struct RuntimeLockHeader {
    /// 声明的锁格式版本。
    version: u16,
}

/// 项目锁定的一项运行时或扩展制品。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeArtifact {
    /// 稳定制品名称。
    pub name: String,
    /// 版本或修订。
    pub version: String,
    /// 下载或复制内容的 SHA-256。
    pub sha256: String,
}

/// 项目锁定的一项内置扩展包。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePackage {
    /// 规范包名，例如 `std`。
    pub name: String,
    /// 精确包版本。
    pub version: String,
    /// 完整包预编译 IR 身份的 SHA-256。
    pub digest: String,
}

/// 项目锁定的版本化知识快照身份。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeKnowledge {
    /// 知识快照结构版本。
    pub format_version: u32,
    /// 权威工具链来源摘要。
    pub toolchain_digest: String,
    /// 知识内容摘要。
    pub content_digest: String,
}

/// 可提交到仓库、但不包含凭证或实际授权的 Teal 运行时锁。
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeLock {
    /// 锁文件格式版本。
    pub version: u16,
    /// 所需 Teal 发行版本。
    pub teal_version: String,
    /// 锁定的 Teal 源码语言 edition。
    #[serde(default = "default_language_edition")]
    pub language_edition: String,
    /// 核心 package catalog 的规范摘要。
    pub core_catalog_digest: String,
    /// 工作台扩展 catalog 的规范摘要；没有扩展 catalog 时省略。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extension_catalog_digest: Option<String>,
    /// 锁定的 package manifest 格式版本。
    pub package_format: u16,
    /// 按名称排序的内置扩展包。
    pub packages: Vec<RuntimePackage>,
    /// 按名称排序的扩展制品。
    pub artifacts: Vec<RuntimeArtifact>,
    /// 编译和执行目标运行时配置档。
    pub runtime_profile: String,
    /// 可选的版本化知识快照身份。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub knowledge: Option<RuntimeKnowledge>,
}

/// 当前发行资源与项目锁的比较结果。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RuntimeLockStatus {
    /// 版本、内置扩展包和制品元数据一致。
    Ready,
    /// 项目尚未创建锁文件。
    Missing,
    /// 当前 Teal 版本不同。
    VersionMismatch {
        /// 锁文件版本。
        expected: String,
        /// 当前二进制版本。
        actual: String,
    },
    /// 当前内置扩展包与项目锁不一致。
    PackageMismatch(Vec<String>),
    /// 锁文件尚未记录知识快照，需刷新。
    MissingKnowledge,
    /// 锁文件知识快照与当前发行不一致。
    KnowledgeMismatch {
        /// 锁文件记录的知识制品格式版本。
        expected_version: u32,
        /// 当前二进制的知识制品格式版本。
        actual_version: u32,
        /// 锁文件内容摘要。
        expected: String,
        /// 当前内容摘要。
        actual: String,
    },
    /// 用户缓存缺少给定制品。
    MissingArtifacts(Vec<String>),
}

/// 运行时锁、清单或用户缓存失败。
#[derive(Debug, thiserror::Error)]
pub enum RuntimeLockError {
    /// 文件访问失败。
    #[error("Teal runtime environment I/O failure: {0}")]
    Io(#[from] std::io::Error),
    /// JSON 无法解析或写入。
    #[error("Teal runtime lock JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    /// 版本、名称或摘要无效。
    #[error("Teal runtime lock is invalid: {0}")]
    Invalid(String),
}

impl RuntimeLock {
    /// 创建使用 RuntimeProfile 的规范锁文件。
    ///
    /// # Errors
    /// 版本、内置扩展包、制品或配置档无效时返回错误。
    pub fn new(
        teal_version: impl Into<String>,
        mut packages: Vec<RuntimePackage>,
        mut artifacts: Vec<RuntimeArtifact>,
        runtime_profile: impl Into<String>,
    ) -> Result<Self, RuntimeLockError> {
        artifacts.sort_by(|left, right| left.name.cmp(&right.name));
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        let lock = Self {
            version: LOCK_VERSION,
            teal_version: teal_version.into(),
            language_edition: "2026".into(),
            core_catalog_digest: catalog_digest(&packages),
            extension_catalog_digest: None,
            package_format: PACKAGE_FORMAT_VERSION,
            packages,
            artifacts,
            runtime_profile: runtime_profile.into(),
            knowledge: None,
        };
        lock.validate()?;
        Ok(lock)
    }

    /// 绑定拆仓后的核心与工作台扩展 catalog 身份。
    #[must_use]
    pub fn with_catalog_identity(
        mut self,
        core_catalog_digest: impl Into<String>,
        extension_catalog_digest: Option<String>,
        package_format: u16,
    ) -> Self {
        self.core_catalog_digest = core_catalog_digest.into();
        self.extension_catalog_digest = extension_catalog_digest;
        self.package_format = package_format;
        self
    }

    /// 设置当前发行知识快照身份。
    #[must_use]
    pub fn with_knowledge(mut self, knowledge: RuntimeKnowledge) -> Self {
        self.knowledge = Some(knowledge);
        self
    }

    /// 从 `.teal/runtime.lock` 读取并验证锁文件。
    ///
    /// # Errors
    /// 文件超过 1 MiB、版本不兼容或内容无效时返回错误。
    pub fn load(path: &Path) -> Result<Self, RuntimeLockError> {
        if path.metadata()?.len() > MAX_LOCK_BYTES {
            return Err(RuntimeLockError::Invalid(
                "runtime.lock exceeds 1 MiB".into(),
            ));
        }
        let bytes = std::fs::read(path)?;
        let header: RuntimeLockHeader = serde_json::from_slice(&bytes)?;
        if header.version != LOCK_VERSION {
            return Err(RuntimeLockError::Invalid(format!(
                "runtime.lock version {} is unsupported; this build uses {LOCK_VERSION}",
                header.version
            )));
        }
        let lock: Self = serde_json::from_slice(&bytes)?;
        lock.validate()?;
        Ok(lock)
    }

    /// 以原子替换写入规范 JSON。
    ///
    /// # Errors
    /// 父目录创建、序列化、刷新或替换失败时返回错误。
    pub fn save(&self, path: &Path) -> Result<(), RuntimeLockError> {
        self.validate()?;
        let parent = path
            .parent()
            .ok_or_else(|| RuntimeLockError::Invalid("lock file has no parent directory".into()))?;
        std::fs::create_dir_all(parent)?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        serde_json::to_writer_pretty(&mut temporary, self)?;
        temporary.write_all(b"\n")?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(path)
            .map_err(|error| RuntimeLockError::Io(error.error))?;
        Ok(())
    }

    /// 对比当前发行版本、内置扩展包及用户缓存。
    #[must_use]
    pub fn status(
        &self,
        teal_version: &str,
        packages: &[RuntimePackage],
        cache: &RuntimeCache,
    ) -> RuntimeLockStatus {
        self.status_with_catalog_identity(
            teal_version,
            packages,
            &catalog_digest_for_runtime_packages(packages),
            None,
            PACKAGE_FORMAT_VERSION,
            cache,
        )
    }

    /// 对比当前发行版本、核心/扩展 catalog 身份及用户缓存。
    #[must_use]
    pub fn status_with_catalog_identity(
        &self,
        teal_version: &str,
        packages: &[RuntimePackage],
        core_catalog_digest: &str,
        extension_catalog_digest: Option<&str>,
        package_format: u16,
        cache: &RuntimeCache,
    ) -> RuntimeLockStatus {
        if self.teal_version != teal_version {
            return RuntimeLockStatus::VersionMismatch {
                expected: self.teal_version.clone(),
                actual: teal_version.to_owned(),
            };
        }
        let mut packages = packages.to_vec();
        packages.sort_by(|left, right| left.name.cmp(&right.name));
        if packages != self.packages {
            let current = packages
                .iter()
                .map(|package| (package.name.as_str(), package))
                .collect::<std::collections::BTreeMap<_, _>>();
            let locked = self
                .packages
                .iter()
                .map(|package| (package.name.as_str(), package))
                .collect::<std::collections::BTreeMap<_, _>>();
            let names = current
                .keys()
                .chain(locked.keys())
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            return RuntimeLockStatus::PackageMismatch(
                names
                    .into_iter()
                    .filter(|name| current.get(name) != locked.get(name))
                    .map(ToOwned::to_owned)
                    .collect(),
            );
        }
        if self.package_format != package_format
            || self.core_catalog_digest != core_catalog_digest
            || self.extension_catalog_digest.as_deref() != extension_catalog_digest
        {
            return RuntimeLockStatus::PackageMismatch(vec!["catalog".into()]);
        }
        let missing = self
            .artifacts
            .iter()
            .filter(|artifact| !cache.contains(artifact))
            .map(|artifact| artifact.name.clone())
            .collect::<Vec<_>>();
        if missing.is_empty() {
            RuntimeLockStatus::Ready
        } else {
            RuntimeLockStatus::MissingArtifacts(missing)
        }
    }

    fn validate(&self) -> Result<(), RuntimeLockError> {
        if self.version != LOCK_VERSION {
            return Err(RuntimeLockError::Invalid(format!(
                "runtime.lock version {} is unsupported; this build uses {LOCK_VERSION}",
                self.version
            )));
        }
        if self.teal_version.trim().is_empty() {
            return Err(RuntimeLockError::Invalid("Teal version is invalid".into()));
        }
        if self.language_edition != "2026" {
            return Err(RuntimeLockError::Invalid(
                "runtime lock language edition is unsupported".into(),
            ));
        }
        if !valid_sha256(&self.core_catalog_digest)
            || self
                .extension_catalog_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || self.package_format != PACKAGE_FORMAT_VERSION
        {
            return Err(RuntimeLockError::Invalid(
                "runtime lock catalog identity or package format is invalid".into(),
            ));
        }
        let mut names = std::collections::BTreeSet::new();
        for package in &self.packages {
            if !valid_package_name(&package.name)
                || !valid_package_version(&package.version)
                || !valid_sha256(&package.digest)
                || !names.insert(&package.name)
            {
                return Err(RuntimeLockError::Invalid(
                    "runtime package name, version, or digest is invalid or duplicated".into(),
                ));
            }
        }
        names.clear();
        for artifact in &self.artifacts {
            if artifact.name.is_empty()
                || artifact.version.is_empty()
                || !valid_name(&artifact.name)
                || !valid_sha256(&artifact.sha256)
                || !names.insert(&artifact.name)
            {
                return Err(RuntimeLockError::Invalid(
                    "extension artifact name, version, or digest is invalid or duplicated".into(),
                ));
            }
        }
        if !valid_name(&self.runtime_profile) {
            return Err(RuntimeLockError::Invalid(
                "runtime profile name is invalid".into(),
            ));
        }
        if let Some(knowledge) = &self.knowledge
            && (knowledge.format_version == 0
                || !valid_sha256(&knowledge.toolchain_digest)
                || !valid_sha256(&knowledge.content_digest))
        {
            return Err(RuntimeLockError::Invalid(
                "runtime knowledge identity is invalid".into(),
            ));
        }
        Ok(())
    }

    /// 比较当前知识快照身份；缺少快照时报告需要刷新。
    #[must_use]
    pub fn knowledge_status(&self, current: &RuntimeKnowledge) -> Option<RuntimeLockStatus> {
        match &self.knowledge {
            None => Some(RuntimeLockStatus::MissingKnowledge),
            Some(locked) if locked != current => Some(RuntimeLockStatus::KnowledgeMismatch {
                expected_version: locked.format_version,
                actual_version: current.format_version,
                expected: locked.content_digest.clone(),
                actual: current.content_digest.clone(),
            }),
            Some(_) => None,
        }
    }
}

fn default_language_edition() -> String {
    "2026".into()
}

fn valid_package_name(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('/')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn valid_package_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'+'))
}

/// 以内容摘要寻址、不会修改系统 PATH 的用户制品缓存。
#[derive(Clone, Debug)]
pub struct RuntimeCache {
    root: PathBuf,
}

impl RuntimeCache {
    /// 创建用户缓存目录对象；构造时不写文件。
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// 返回制品是否已经存在且摘要匹配。
    #[must_use]
    pub fn contains(&self, artifact: &RuntimeArtifact) -> bool {
        std::fs::read(self.path(artifact))
            .is_ok_and(|contents| sha256(&contents) == artifact.sha256)
    }

    /// 把已经由上层下载或选择的字节校验后原子写入用户缓存。
    ///
    /// 本方法不访问网络、不解压、不运行制品，也不修改系统环境。
    ///
    /// # Errors
    /// 名称、摘要或文件写入无效时返回错误。
    pub fn install(
        &self,
        artifact: &RuntimeArtifact,
        bytes: &[u8],
    ) -> Result<PathBuf, RuntimeLockError> {
        if !valid_name(&artifact.name) || !valid_sha256(&artifact.sha256) {
            return Err(RuntimeLockError::Invalid(
                "cache artifact metadata is invalid".into(),
            ));
        }
        if sha256(bytes) != artifact.sha256 {
            return Err(RuntimeLockError::Invalid(format!(
                "SHA-256 mismatch for artifact {}",
                artifact.name
            )));
        }
        std::fs::create_dir_all(&self.root)?;
        let target = self.path(artifact);
        let mut temporary = tempfile::NamedTempFile::new_in(&self.root)?;
        temporary.write_all(bytes)?;
        temporary.as_file_mut().sync_all()?;
        temporary
            .persist(&target)
            .map_err(|error| RuntimeLockError::Io(error.error))?;
        Ok(target)
    }

    /// 从显式 HTTPS 地址下载制品，验证完整 SHA-256 后写入用户缓存。
    ///
    /// 本方法不解压、不运行制品，也不修改系统环境；响应上限为 512 MiB。
    ///
    /// # Errors
    /// 地址、HTTP 状态、响应大小、摘要或缓存写入失败时返回错误。
    pub async fn download(
        &self,
        artifact: &RuntimeArtifact,
        source: &str,
    ) -> Result<PathBuf, RuntimeLockError> {
        let url = reqwest::Url::parse(source)
            .map_err(|error| RuntimeLockError::Invalid(error.to_string()))?;
        if url.scheme() != "https" {
            return Err(RuntimeLockError::Invalid(
                "Runtime artifacts are only allowed to be downloaded from HTTPS addresses".into(),
            ));
        }
        let response = reqwest::Client::new()
            .get(url)
            .send()
            .await
            .map_err(|error| RuntimeLockError::Invalid(error.to_string()))?;
        if !response.status().is_success() {
            return Err(RuntimeLockError::Invalid(format!(
                "runtime artifact returned HTTP status {}",
                response.status()
            )));
        }
        if response.url().scheme() != "https" {
            return Err(RuntimeLockError::Invalid(
                "Runtime artifact redirects away from HTTPS".into(),
            ));
        }
        const MAX_ARTIFACT_BYTES: u64 = 512 * 1024 * 1024;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_ARTIFACT_BYTES)
        {
            return Err(RuntimeLockError::Invalid(
                "Runtime artifact exceeds 512 MiB".into(),
            ));
        }
        let bytes = response
            .bytes()
            .await
            .map_err(|error| RuntimeLockError::Invalid(error.to_string()))?;
        if bytes.len() as u64 > MAX_ARTIFACT_BYTES {
            return Err(RuntimeLockError::Invalid(
                "Runtime artifact exceeds 512 MiB".into(),
            ));
        }
        self.install(artifact, &bytes)
    }

    fn path(&self, artifact: &RuntimeArtifact) -> PathBuf {
        self.root.join(format!(
            "{}-{}-{}.artifact",
            artifact.name, artifact.version, artifact.sha256
        ))
    }
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn sha256(bytes: &[u8]) -> String {
    teal_utils::sha256_hex(bytes)
}

fn catalog_digest(packages: &[RuntimePackage]) -> String {
    let mut bytes = b"teal-catalog-v2\0".to_vec();
    for package in packages {
        hash_field(&mut bytes, &package.name);
        hash_field(&mut bytes, &package.version);
        hash_field(&mut bytes, &package.digest);
    }
    sha256(&bytes)
}

fn catalog_digest_for_runtime_packages(packages: &[RuntimePackage]) -> String {
    let mut packages = packages.to_vec();
    packages.sort_by(|left, right| left.name.cmp(&right.name));
    catalog_digest(&packages)
}

fn hash_field(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend_from_slice(&(value.len() as u64).to_le_bytes());
    bytes.extend_from_slice(value.as_bytes());
}

#[cfg(test)]
mod tests;
