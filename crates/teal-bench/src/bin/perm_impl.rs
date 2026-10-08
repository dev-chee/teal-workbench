pub(crate) fn execute_permissions_command(command: crate::PermissionsCommand) -> i32 {
    let workspace = match std::env::current_dir() {
        Ok(workspace) => workspace,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let paths = match teal_config::ConfigPaths::discover(Some(&workspace)) {
        Ok(paths) => paths,
        Err(error) => {
            eprintln!("{error}");
            return 1;
        }
    };
    let result = match command {
        crate::PermissionsCommand::Show { json } => {
            teal_config::load_declarations_paths(paths.clone())
                .map(|effective| effective.workspace_permissions())
                .and_then(|profile| {
                    if json {
                        println!(
                            "{}",
                            crate::output::json("perm.show", &profile).map_err(|error| {
                                teal_config::ConfigError::invalid(
                                    teal_config::ConfigFile::Config,
                                    error.to_string(),
                                )
                            })?
                        );
                    } else {
                        println!(
                            "preset={:?} generation={} rules={}",
                            profile.preset,
                            profile.generation,
                            profile.rules.len()
                        );
                        for rule in profile.rules {
                            println!("{}\t{:?}\t{:?}", rule.operation, rule.effect, rule.resource);
                        }
                    }
                    Ok(())
                })
        }
        crate::PermissionsCommand::Preset(preset) => {
            teal_config::set_permission_preset(&paths, preset)
        }
        crate::PermissionsCommand::Grant(file) => (|| {
            let document = teal_config::read_permission_file(&file)?;
            teal_config::add_permission_rules(&paths, document.rules)?;
            Ok(())
        })(),
        crate::PermissionsCommand::Explain { request, json } => (|| {
            let text = std::fs::read_to_string(&request).map_err(|source| {
                teal_config::ConfigError::Io {
                    path: request.clone(),
                    source,
                }
            })?;
            let request_value: teal_security::PermissionRequest =
                toml::from_str(&text).map_err(|error| teal_config::ConfigError::Toml {
                    path: request.clone(),
                    message: error.to_string(),
                })?;
            let profile =
                teal_config::load_declarations_paths(paths.clone())?.workspace_permissions();
            let upper = request_value.resources.iter().cloned().fold(
                teal_security::ExecutionGrant::deny_all("teal-perm/explain/v1"),
                |grant, resource| {
                    grant.allow(teal_security::PermissionRule {
                        subject: request_value.subject.clone(),
                        operation: request_value.operation.clone(),
                        resource,
                        effect: teal_security::PermissionEffect::Allow,
                    })
                },
            );
            let session = teal_security::PermissionSession::new(
                profile.preset,
                profile.generation,
                upper,
                profile.rules,
            );
            let (decision, origin) = match session.evaluate(request_value).map_err(|error| {
                teal_config::ConfigError::invalid(
                    teal_config::ConfigFile::Config,
                    error.to_string(),
                )
            })? {
                teal_security::PermissionDecision::Allow(permit) => (
                    "allow",
                    match permit.origin() {
                        teal_security::RuleOrigin::Grant => "grant",
                        teal_security::RuleOrigin::User => "user",
                        teal_security::RuleOrigin::Task => "task",
                        teal_security::RuleOrigin::Preset => "preset",
                    },
                ),
                teal_security::PermissionDecision::Ask(_) => ("ask", "default-or-ask-rule"),
                teal_security::PermissionDecision::Deny { origin, .. } => (
                    "deny",
                    match origin {
                        teal_security::RuleOrigin::Grant => "grant",
                        teal_security::RuleOrigin::User => "user",
                        teal_security::RuleOrigin::Task => "task",
                        teal_security::RuleOrigin::Preset => "preset",
                    },
                ),
            };
            if json {
                println!(
                    "{}",
                    crate::output::json(
                        "perm.explain",
                        serde_json::json!({"decision": decision, "origin": origin}),
                    )
                    .map_err(|error| {
                        teal_config::ConfigError::invalid(
                            teal_config::ConfigFile::Config,
                            error.to_string(),
                        )
                    })?
                );
            } else {
                println!("decision={decision} origin={origin}");
            }
            Ok(())
        })(),
        crate::PermissionsCommand::Operations { json } => {
            let entries = teal_host::ExtensionCatalog::builtin()
                .iter()
                .flat_map(|extension| {
                    extension.operations.iter().map(|operation| {
                        serde_json::json!({
                            "operation": operation.to_string(),
                            "extension": extension.id.to_string(),
                            "contract_version": extension.contract_version,
                        })
                    })
                })
                .collect::<Vec<_>>();
            if json {
                println!(
                    "{}",
                    crate::output::json("perm.operations", &entries).unwrap()
                );
            } else {
                for extension in teal_host::ExtensionCatalog::builtin().iter() {
                    for operation in &extension.operations {
                        println!("{:<28} {}", operation, extension.id);
                    }
                }
            }
            Ok(())
        }
        crate::PermissionsCommand::Revoke => teal_config::revoke_workspace_permissions(&paths),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        return 1;
    }
    0
}
