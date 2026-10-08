#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ChangeSetInput {
    pub(crate) files: Vec<ChangeSetFileInput>,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub(crate) struct ChangeSetFileInput {
    pub(crate) path: String,
    pub(crate) expected_sha256: Option<String>,
    pub(crate) new_sha256: Option<String>,
    pub(crate) patch: String,
}
