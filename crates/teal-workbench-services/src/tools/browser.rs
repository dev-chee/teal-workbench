//! 受管浏览器 Host 工具注册与准备边界。

use super::*;

pub(super) fn register_browser_hosts(
    registry: &mut teal::HostRegistry,
    browser: teal_browser::BrowserManager,
) -> Result<(), teal::HostRegistryError> {
    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "screenshot",
            teal::HostCallSignature::new(
                [RuntimeType::String, RuntimeType::String, RuntimeType::Int],
                RuntimeType::Map,
            )
            .expect("screenshot signature"),
            "browser.inspect",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    let page = string_argument(&arguments, 1, "page")?;
                    let generation = u64::try_from(int_argument(&arguments, 2, "generation")?)
                        .map_err(host_error)?;
                    let bytes = manager
                        .screenshot(&session, &page, generation)
                        .await
                        .map_err(browser_host_error)?;
                    super::media::image_value(
                        &teal_model::ImageContent::new(bytes).map_err(host_error)?,
                    )
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.screenshot", arguments))
        .with_tool_export(browser_export(
            "browser.screenshot",
            "Capture the managed page viewport as a PNG image",
            &["session", "page", "generation"],
            HostToolEffect::ReadOnly,
        ))
        .with_tool_image(super::media::image_export()),
    )?;

    let start_preparer = browser.clone();
    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "start",
            teal::HostCallSignature::new([RuntimeType::Bool], RuntimeType::Map)
                .expect("Static browser Host signature is valid"),
            "browser.navigate",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let visible = bool_argument(&arguments, 0, "visible")?;
                    let session = manager.start(visible).await.map_err(browser_host_error)?;
                    Ok(HostValue::Map(vec![(
                        HostValue::String("session".into()),
                        HostValue::String(session.0),
                    )]))
                })
            },
        )
        .with_preparer(move |arguments| {
            browser_preparation(Some(&start_preparer), "browser.start", arguments)
        })
        .with_tool_export(browser_export(
            "browser.start",
            "Start a managed Edge or Chrome session with an isolated temporary profile",
            &["visible"],
            HostToolEffect::Mutating,
        )),
    )?;

    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "open",
            teal::HostCallSignature::new(
                [RuntimeType::String, RuntimeType::String],
                RuntimeType::Map,
            )
            .expect("Static browser Host signature is valid"),
            "browser.navigate",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    let url = string_argument(&arguments, 1, "url")?;
                    manager
                        .allow_network(&session, &url)
                        .await
                        .map_err(browser_host_error)?;
                    manager
                        .open(&session, &url)
                        .await
                        .map(browser_page_value)
                        .map_err(browser_host_error)
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.open", arguments))
        .with_tool_export(browser_export(
            "browser.open",
            "Open an HTTP or HTTPS page in a managed browser session",
            &["session", "url"],
            HostToolEffect::Mutating,
        )),
    )?;

    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "allow_network",
            teal::HostCallSignature::new(
                [RuntimeType::String, RuntimeType::String],
                RuntimeType::None,
            )
            .expect("Static browser Host signature is valid"),
            "browser.navigate",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    let url = string_argument(&arguments, 1, "url")?;
                    manager
                        .allow_network(&session, &url)
                        .await
                        .map_err(browser_host_error)?;
                    Ok(HostValue::None)
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.allow_network", arguments))
        .with_tool_export(browser_export(
            "browser.allow_network",
            "Approve one exact HTTP or HTTPS URL for a managed session before retrying navigation",
            &["session", "url"],
            HostToolEffect::Mutating,
        )),
    )?;

    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "snapshot",
            teal::HostCallSignature::new(
                [RuntimeType::String, RuntimeType::String],
                RuntimeType::String,
            )
            .expect("Static browser Host signature is valid"),
            "browser.inspect",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    let page = string_argument(&arguments, 1, "page")?;
                    let snapshot = manager
                        .snapshot(&session, &page)
                        .await
                        .map_err(browser_host_error)?;
                    serde_json::to_string(&snapshot)
                        .map(HostValue::String)
                        .map_err(host_error)
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.snapshot", arguments))
        .with_tool_export(browser_export(
            "browser.snapshot",
            "Read the accessibility tree and return a new page generation",
            &["session", "page"],
            HostToolEffect::ReadOnly,
        )),
    )?;

    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "click",
            teal::HostCallSignature::new(
                [
                    RuntimeType::String,
                    RuntimeType::String,
                    RuntimeType::Int,
                    RuntimeType::Int,
                ],
                RuntimeType::None,
            )
            .expect("Static browser Host signature is valid"),
            "browser.interact",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    manager
                        .click(
                            &session,
                            &string_argument(&arguments, 1, "page")?,
                            i64::from(int_argument(&arguments, 2, "backend_node_id")?),
                            u64::try_from(int_argument(&arguments, 3, "generation")?)
                                .map_err(host_error)?,
                        )
                        .await
                        .map_err(browser_host_error)?;
                    Ok(HostValue::None)
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.click", arguments))
        .with_tool_export(browser_export(
            "browser.click",
            "Click a backend DOM node from the current accessibility snapshot",
            &["session", "page", "backend_node_id", "generation"],
            HostToolEffect::Mutating,
        )),
    )?;

    let manager = browser.clone();
    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "fill",
            teal::HostCallSignature::new(
                [
                    RuntimeType::String,
                    RuntimeType::String,
                    RuntimeType::Int,
                    RuntimeType::Int,
                    RuntimeType::String,
                ],
                RuntimeType::None,
            )
            .expect("Static browser Host signature is valid"),
            "browser.interact",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = manager.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    manager
                        .fill(
                            &session,
                            &string_argument(&arguments, 1, "page")?,
                            i64::from(int_argument(&arguments, 2, "backend_node_id")?),
                            u64::try_from(int_argument(&arguments, 3, "generation")?)
                                .map_err(host_error)?,
                            &string_argument(&arguments, 4, "text")?,
                        )
                        .await
                        .map_err(browser_host_error)?;
                    Ok(HostValue::None)
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.fill", arguments))
        .with_tool_export(browser_export(
            "browser.fill",
            "Focus a backend DOM node and insert non-secret text",
            &["session", "page", "backend_node_id", "generation", "text"],
            HostToolEffect::Mutating,
        )),
    )?;

    registry.register(
        HostFunctionDefinition::asynchronous(
            "std.bench.browser",
            "close",
            teal::HostCallSignature::new([RuntimeType::String], RuntimeType::None)
                .expect("Static browser Host signature is valid"),
            "browser.interact",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let manager = browser.clone();
                Box::pin(async move {
                    let session =
                        teal_browser::BrowserSessionId(string_argument(&arguments, 0, "session")?);
                    manager.close(&session).await.map_err(browser_host_error)?;
                    Ok(HostValue::None)
                })
            },
        )
        .with_preparer(|arguments| browser_preparation(None, "browser.close", arguments))
        .with_tool_export(browser_export(
            "browser.close",
            "Close a managed browser session and remove its temporary profile",
            &["session"],
            HostToolEffect::Mutating,
        )),
    )?;
    Ok(())
}

fn browser_host_error(error: teal_browser::BrowserError) -> HostError {
    if matches!(
        error,
        teal_browser::BrowserError::AuditUnknown(_)
            | teal_browser::BrowserError::NetworkAuditUnknown(_)
    ) {
        HostError::UnknownEffect(error.to_string())
    } else {
        host_error(error)
    }
}

pub(super) fn browser_preparation(
    browser: Option<&teal_browser::BrowserManager>,
    name: &str,
    arguments: &[HostValue],
) -> Result<HostCallPreparation, HostError> {
    let mut arguments = arguments.to_vec();
    if matches!(name, "browser.open" | "browser.allow_network") {
        arguments[1] =
            HostValue::String(normalized_web_url(&string_argument(&arguments, 1, "url")?)?);
    }
    let identity = if name == "browser.start" {
        let executable = browser
            .ok_or_else(|| host_error("managed browser preparer is unavailable"))?
            .executable()
            .map_err(host_error)?;
        format!(
            "managed/{}",
            executable.to_string_lossy().replace('\\', "/")
        )
    } else {
        let mut parts = vec![string_argument(&arguments, 0, "session")?];
        if arguments.len() > 1 && !matches!(name, "browser.open" | "browser.allow_network") {
            parts.push(string_argument(&arguments, 1, "page")?);
        }
        if matches!(name, "browser.click" | "browser.fill") {
            parts.push(int_argument(&arguments, 3, "generation")?.to_string());
        }
        if name == "browser.screenshot" {
            parts.push(int_argument(&arguments, 2, "generation")?.to_string());
        }
        parts.join("/")
    };
    let access = if matches!(name, "browser.snapshot" | "browser.screenshot") {
        ResourceAccess::Read
    } else {
        ResourceAccess::Write
    };
    let mut resources = vec![HostResourceRequest {
        access,
        resource: HostResource::Application {
            namespace: "browser".into(),
            identity,
        },
    }];
    if matches!(name, "browser.open" | "browser.allow_network") {
        resources.push(HostResourceRequest {
            access: ResourceAccess::Network,
            resource: HostResource::Network(string_argument(&arguments, 1, "url")?),
        });
    }
    Ok(HostCallPreparation::new(arguments, resources))
}

pub(super) fn normalized_web_url(input: &str) -> Result<String, HostError> {
    let mut parsed = url::Url::parse(input).map_err(host_error)?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return Err(host_error("URL must use HTTP or HTTPS"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(host_error("URL credentials are not allowed"));
    }
    parsed.set_fragment(None);
    Ok(parsed.to_string())
}

fn browser_export(
    name: &str,
    description: &str,
    parameters: &[&str],
    effect: HostToolEffect,
) -> HostToolExport {
    HostToolExport {
        name: name.into(),
        description: description.into(),
        parameters: parameters
            .iter()
            .map(|name| HostToolParameter {
                name: (*name).into(),
                description: format!("Browser {name}"),
            })
            .collect(),
        effect,
    }
}

fn browser_page_value(page: teal_browser::BrowserPage) -> HostValue {
    HostValue::Map(vec![
        (
            HostValue::String("session".into()),
            HostValue::String(page.session.0),
        ),
        (
            HostValue::String("page".into()),
            HostValue::String(page.page_id),
        ),
        (HostValue::String("url".into()), HostValue::String(page.url)),
        (
            HostValue::String("title".into()),
            HostValue::String(page.title),
        ),
        (
            HostValue::String("generation".into()),
            HostValue::Int(i32::try_from(page.generation).unwrap_or(i32::MAX)),
        ),
    ])
}
