//! 桌面可访问性 Host 工具注册与准备边界。

use super::*;

pub(super) fn register_computer_hosts(
    registry: &mut teal::HostRegistry,
    computer: Arc<dyn teal_computer::ComputerBackend>,
) -> Result<(), teal::HostRegistryError> {
    let backend = Arc::clone(&computer);
    registry.register(
        HostFunctionDefinition::blocking(
            "std.bench.computer",
            "list_windows",
            teal::HostCallSignature::new([], RuntimeType::Array)
                .expect("Static computer Host signature is valid")
                .task(),
            "computer.inspect",
            ExternalCallRecovery::Journaled,
            move |_| {
                backend
                    .list_windows()
                    .map_err(host_error)?
                    .into_iter()
                    .map(|window| {
                        serde_json::to_string(&window)
                            .map(HostValue::String)
                            .map_err(host_error)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(HostValue::Array)
            },
        )
        .with_preparer(|arguments| computer_preparation("computer.list_windows", arguments))
        .with_tool_export(computer_export(
            "computer.list_windows",
            "List visible top-level windows with generation-bound identities",
            &[],
            HostToolEffect::ReadOnly,
        )),
    )?;

    let backend = Arc::clone(&computer);
    registry.register(
        HostFunctionDefinition::blocking(
            "std.bench.computer",
            "snapshot",
            teal::HostCallSignature::new(
                [RuntimeType::Int, RuntimeType::String, RuntimeType::Int],
                RuntimeType::String,
            )
            .expect("Static computer Host signature is valid")
            .task(),
            "computer.inspect",
            ExternalCallRecovery::Journaled,
            move |arguments| {
                serde_json::to_string(
                    &backend
                        .snapshot(&computer_window_argument(arguments)?)
                        .map_err(host_error)?,
                )
                .map(HostValue::String)
                .map_err(host_error)
            },
        )
        .with_preparer(|arguments| computer_preparation("computer.snapshot", arguments))
        .with_tool_export(computer_export(
            "computer.snapshot",
            "Read a bounded UI Automation tree and return a new window generation",
            &["process_id", "window_handle", "generation"],
            HostToolEffect::ReadOnly,
        )),
    )?;

    register_computer_action(
        registry,
        Arc::clone(&computer),
        "invoke",
        "computer.invoke",
        "Invoke a semantic UI Automation action on a non-password element",
        "computer.interact",
        None,
        |backend, window, runtime_id, _| backend.invoke(window, runtime_id),
    )?;
    register_computer_action(
        registry,
        Arc::clone(&computer),
        "set_value",
        "computer.set_value",
        "Set a non-password element through the UI Automation Value pattern",
        "computer.interact",
        Some("value"),
        |backend, window, runtime_id, value| backend.set_value(window, runtime_id, value),
    )?;
    register_computer_action(
        registry,
        Arc::clone(&computer),
        "raw_click",
        "computer.raw_click",
        "Explicitly click the center of a non-password element using raw input",
        "computer.raw_input",
        None,
        |backend, window, runtime_id, _| backend.raw_click(window, runtime_id),
    )?;
    register_computer_action(
        registry,
        computer,
        "raw_keys",
        "computer.raw_keys",
        "Explicitly focus a non-password element and send raw keys",
        "computer.raw_input",
        Some("keys"),
        |backend, window, runtime_id, value| backend.raw_keys(window, runtime_id, value),
    )?;
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "Computer Host and tool contracts stay explicit at registration"
)]
fn register_computer_action<F>(
    registry: &mut teal::HostRegistry,
    computer: Arc<dyn teal_computer::ComputerBackend>,
    function: &'static str,
    tool_name: &'static str,
    description: &'static str,
    capability: &'static str,
    value_parameter: Option<&'static str>,
    callback: F,
) -> Result<(), teal::HostRegistryError>
where
    F: Fn(
            &dyn teal_computer::ComputerBackend,
            &teal_computer::ComputerWindow,
            &str,
            &str,
        ) -> Result<(), teal_computer::ComputerError>
        + Send
        + Sync
        + 'static,
{
    let argument_types = if value_parameter.is_some() {
        vec![
            RuntimeType::Int,
            RuntimeType::String,
            RuntimeType::Int,
            RuntimeType::String,
            RuntimeType::String,
        ]
    } else {
        vec![
            RuntimeType::Int,
            RuntimeType::String,
            RuntimeType::Int,
            RuntimeType::String,
        ]
    };
    let mut parameters = vec!["process_id", "window_handle", "generation", "runtime_id"];
    parameters.extend(value_parameter);
    registry.register(
        HostFunctionDefinition::blocking(
            "std.bench.computer",
            function,
            teal::HostCallSignature::new(argument_types, RuntimeType::None)
                .expect("Static computer Host signature is valid")
                .task(),
            capability,
            ExternalCallRecovery::Journaled,
            move |arguments| {
                let window = computer_window_argument(arguments)?;
                let runtime_id = string_argument(arguments, 3, "runtime_id")?;
                let value = if let Some(name) = value_parameter {
                    string_argument(arguments, 4, name)?
                } else {
                    String::new()
                };
                callback(computer.as_ref(), &window, &runtime_id, &value)
                    .map_err(computer_host_error)?;
                Ok(HostValue::None)
            },
        )
        .with_preparer(move |arguments| computer_preparation(tool_name, arguments))
        .with_tool_export(computer_export(
            tool_name,
            description,
            &parameters,
            HostToolEffect::Mutating,
        )),
    )
}

fn computer_host_error(error: teal_computer::ComputerError) -> HostError {
    if matches!(
        error,
        teal_computer::ComputerError::UnknownEffect(_)
            | teal_computer::ComputerError::AuditUnknown(_)
    ) {
        HostError::UnknownEffect(error.to_string())
    } else {
        host_error(error)
    }
}

pub(super) fn computer_preparation(
    name: &str,
    arguments: &[HostValue],
) -> Result<HostCallPreparation, HostError> {
    if name == "computer.list_windows" {
        return Ok(HostCallPreparation::new(
            arguments.to_vec(),
            vec![HostResourceRequest {
                access: ResourceAccess::Read,
                resource: HostResource::Application {
                    namespace: "computer".into(),
                    identity: "windows".into(),
                },
            }],
        ));
    }
    let mut arguments = arguments.to_vec();
    let window = computer_window_argument(&arguments)?;
    arguments[1] = HostValue::String(window.window_handle.clone());
    let mut identity = format!(
        "{}/{}/{}",
        window.process_id, window.window_handle, window.generation
    );
    if arguments.len() > 3 {
        let runtime_id = normalized_runtime_id(&string_argument(&arguments, 3, "runtime_id")?)?;
        arguments[3] = HostValue::String(runtime_id.clone());
        identity.push('/');
        identity.push_str(&runtime_id);
    }
    Ok(HostCallPreparation::new(
        arguments,
        vec![HostResourceRequest {
            access: if name == "computer.snapshot" {
                ResourceAccess::Read
            } else {
                ResourceAccess::Write
            },
            resource: HostResource::Application {
                namespace: "computer".into(),
                identity,
            },
        }],
    ))
}

fn computer_window_argument(
    arguments: &[HostValue],
) -> Result<teal_computer::ComputerWindow, HostError> {
    let process_id = u32::try_from(int_argument(arguments, 0, "process_id")?)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| host_error("process_id must be a positive integer"))?;
    let generation = u64::try_from(int_argument(arguments, 2, "generation")?)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| host_error("generation must be a positive integer"))?;
    let window_handle = string_argument(arguments, 1, "window_handle")?
        .parse::<isize>()
        .ok()
        .filter(|value| *value != 0)
        .map(|value| value.to_string())
        .ok_or_else(|| host_error("window_handle must be a nonzero decimal integer"))?;
    Ok(teal_computer::ComputerWindow {
        process_id,
        window_handle,
        title: String::new(),
        generation,
    })
}

fn normalized_runtime_id(value: &str) -> Result<String, HostError> {
    if value.is_empty() {
        return Err(host_error("runtime_id must not be empty"));
    }
    value
        .split('.')
        .map(|part| {
            part.parse::<i32>()
                .map(|value| value.to_string())
                .map_err(|_| host_error("runtime_id must contain dot-separated integers"))
        })
        .collect::<Result<Vec<_>, _>>()
        .map(|parts| parts.join("."))
}

fn computer_export(
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
                description: format!("Computer {name}"),
            })
            .collect(),
        effect,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_conversion_preserves_unknown_computer_effects() {
        assert!(matches!(
            computer_host_error(teal_computer::ComputerError::UnknownEffect("lost".into())),
            HostError::UnknownEffect(message) if message.contains("lost")
        ));
        assert!(matches!(
            computer_host_error(teal_computer::ComputerError::AuditUnknown(
                teal_security::AuditError::Host,
            )),
            HostError::UnknownEffect(message) if message.contains("audit result unavailable")
        ));
        assert!(matches!(
            computer_host_error(teal_computer::ComputerError::Invalid("stale".into())),
            HostError::Message { .. }
        ));
    }
}
