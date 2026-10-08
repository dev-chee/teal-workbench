//! 工作区图片通过既有路径准备、审批和 Host 工具桥读取。
use super::*;
use std::io::Read;
use base64::Engine;

pub(super) fn register_image_host(
    registry: &mut teal::HostRegistry,
    tools: Arc<WorkspaceTools>,
) -> Result<(), teal::HostRegistryError> {
    let preparation = tools.clone();
    registry.register(
        HostFunctionDefinition::blocking_with_context(
            "std.bench.workspace",
            "read_image",
            teal::HostCallSignature::new([RuntimeType::String], RuntimeType::Map)
                .expect("image host signature")
                .task(),
            "editor.read",
            ExternalCallRecovery::Journaled,
            move |_, arguments| {
                let path = tools.resolve_prepared(&string_argument(arguments, 0, "path")?, true)?;
                let mut bytes = Vec::new();
                std::fs::File::open(path)?
                    .take(8 * 1024 * 1024 + 1)
                    .read_to_end(&mut bytes)?;
                let image = teal_model::ImageContent::new(bytes).map_err(host_error)?;
                image_value(&image)
            },
        )
        .with_preparer(move |arguments| {
            let prepared = preparation.prepare(WorkspaceOperation::WorkspaceReadFile, arguments)?;
            let path = string_argument(&prepared.arguments, 0, "path")?;
            let metadata = std::fs::metadata(path)?;
            if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
                return Err(host_error(
                    "Image must be a regular file no larger than 8 MiB",
                ));
            }
            Ok(prepared)
        })
        .with_tool_export(HostToolExport {
            name: "workspace.read_image".into(),
            description: "Read an authorized local PNG or JPEG image; no remote downloads".into(),
            parameters: vec![HostToolParameter {
                name: "path".into(),
                description: "Workspace image path".into(),
            }],
            effect: HostToolEffect::ReadOnly,
        })
        .with_tool_image(image_export()),
    )
}
pub(super) fn image_export() -> teal::HostToolImage {
    teal::HostToolImage {
        data_field: "data".into(),
        media_type_field: "media_type".into(),
    }
}
pub(super) fn image_value(image: &teal_model::ImageContent) -> Result<HostValue, HostError> {
    Ok(HostValue::Map(vec![
        (
            HostValue::String("data".into()),
            HostValue::String(
                base64::engine::general_purpose::STANDARD
                    .encode(image.bytes().map_err(host_error)?),
            ),
        ),
        (
            HostValue::String("media_type".into()),
            HostValue::String(image.media_type.clone()),
        ),
        (
            HostValue::String("width".into()),
            HostValue::String(image.width.to_string()),
        ),
        (
            HostValue::String("height".into()),
            HostValue::String(image.height.to_string()),
        ),
        (
            HostValue::String("digest".into()),
            HostValue::String(image.digest.clone()),
        ),
    ]))
}
