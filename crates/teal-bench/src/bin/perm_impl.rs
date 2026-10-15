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
            teal_security::load_declarations_paths(paths.clone())
                .and_then(|effective| teal_bench::config::permissions(&effective))
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
            teal_security::set_permission_preset(&paths, preset)
        }
        crate::PermissionsCommand::Grant(file) => (|| {
            let document = teal_security::read_permission_file(&file)?;
            teal_security::add_permission_rules(&paths, document.rules)?;
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
            let profile = teal_bench::config::permissions(
                &teal_security::load_declarations_paths(paths.clone())?,
            )?;
            let upper = request_value.resources.iter().cloned().fold(
                teal_security::ExecutionGrant::deny_all("teal-perm/explain/v1"),
                |grant, resource| {
                    grant.allow(teal_security::PermissionRule {
                        subject: request_value.subject.clone(),
                        operation: request_value.operation.clone(),
                        resource,
                        effect: teal_security::PermissionEffect::Allow,
                        arguments_digest: request_value.arguments_digest.clone(),
                        source_digest: request_value.source_digest.clone(),
                        resource_versions: request_value.resource_versions.clone(),
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
            let entries = teal_security::STANDARD_PERMISSION_OPERATIONS
                .iter()
                .map(|operation| serde_json::json!({"operation":operation}))
                .collect::<Vec<_>>();
            if json {
                println!(
                    "{}",
                    crate::output::json("perm.operations", &entries).unwrap()
                );
            } else {
                for operation in teal_security::STANDARD_PERMISSION_OPERATIONS {
                    println!("{operation}");
                }
            }
            Ok(())
        }
        crate::PermissionsCommand::Revoke => teal_security::revoke_workspace_permissions(&paths),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        return 1;
    }
    0
}
