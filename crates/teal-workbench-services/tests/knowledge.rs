//! 工作台分层知识装配、刷新与逐层状态的回归测试。

use teal_agent::{ActionContext, AllowAllPermissions, DenyAllPermissions, KnowledgeProvider};
use teal_knowledge::{KnowledgeScope, LAYER_CACHE_DIRECTORY, LayerOptions};
use teal_workbench_services::WorkbenchKnowledge;

/// 一次知识刷新使用的调用上下文。
fn context() -> ActionContext {
    ActionContext {
        run_id: "teal-bench/knowledge".into(),
        task_id: "knowledge".into(),
        generation: 0,
        policy_version: String::new(),
    }
}

#[tokio::test]
async fn refresh_builds_layers_and_reports_per_layer_status() {
    let fixture = tempfile::tempdir().expect("fixture root");
    let user_root = fixture.path().join("user-knowledge");
    let project_root = fixture.path().join("workspace/.teal/knowledge");
    std::fs::create_dir_all(&user_root).unwrap();
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::write(
        user_root.join("user.md"),
        "# User note\n\nzqxuserzqx body\n",
    )
    .unwrap();
    std::fs::write(
        project_root.join("workspace.md"),
        "# Workspace note\n\nzqxworkzqx body\n",
    )
    .unwrap();

    let knowledge = WorkbenchKnowledge::official()
        .expect("embedded knowledge should decode")
        .with_layers(
            Some(user_root.clone()),
            Some(project_root.clone()),
            LayerOptions::default(),
        )
        .expect("layers should open");

    let statuses = knowledge
        .refresh(&AllowAllPermissions, context(), false)
        .await
        .expect("startup refresh should succeed");
    assert_eq!(statuses.len(), 2);
    assert!(
        !user_root.join(LAYER_CACHE_DIRECTORY).exists()
            && !project_root.join(LAYER_CACHE_DIRECTORY).exists(),
        "a read-only refresh must not write the derived cache"
    );
    for scope in [KnowledgeScope::User, KnowledgeScope::Project] {
        let status = knowledge.status(scope);
        assert!(status.configured(), "{} must be configured", scope.name());
        assert_eq!(status.files, 1);
        assert!(status.bytes > 0);
        assert!(status.error.is_none());
        assert!(!status.cached);
    }

    let written = knowledge
        .refresh(&AllowAllPermissions, context(), true)
        .await
        .expect("explicit refresh should write the cache");
    assert_eq!(written.len(), 2);
    for (scope, root) in [
        (KnowledgeScope::User, &user_root),
        (KnowledgeScope::Project, &project_root),
    ] {
        assert!(
            root.join(LAYER_CACHE_DIRECTORY)
                .join("manifest.json")
                .is_file(),
            "{} cache manifest",
            scope.name()
        );
    }

    let cached = knowledge
        .refresh(&AllowAllPermissions, context(), true)
        .await
        .expect("a second refresh should reuse the cache");
    assert!(cached.iter().all(|status| status.cached));
    assert_eq!(
        knowledge.status(KnowledgeScope::User).files,
        1,
        "the status must follow the refreshed layer"
    );

    let hits = knowledge
        .search("zqxuserzqx", Some(KnowledgeScope::User), 5)
        .expect("user search");
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].scope, KnowledgeScope::User);
    let workspace = knowledge
        .search("zqxworkzqx", Some(KnowledgeScope::Project), 5)
        .expect("workspace search");
    assert_eq!(workspace.len(), 1);
    assert_eq!(workspace[0].scope, KnowledgeScope::Project);
    assert_eq!(
        knowledge.read(&hits[0].id, 4096).expect("user entry").title,
        "User note"
    );
    assert!(
        knowledge
            .search("zqxuserzqx", Some(KnowledgeScope::Official), 5)
            .expect("official search")
            .is_empty()
    );
    assert!(
        knowledge
            .overview(4096)
            .text
            .contains("<teal_home>/knowledge")
    );
}

#[tokio::test]
async fn workspace_layer_documents_require_read_authorization() {
    let fixture = tempfile::tempdir().expect("fixture root");
    let project_root = fixture.path().join("workspace/.teal/knowledge");
    std::fs::create_dir_all(&project_root).unwrap();
    std::fs::write(
        project_root.join("workspace.md"),
        "# Workspace note\n\nzqxdenyzqx body\n",
    )
    .unwrap();

    let knowledge = WorkbenchKnowledge::official()
        .expect("embedded knowledge should decode")
        .with_layers(None, Some(project_root), LayerOptions::default())
        .expect("layer should open");
    let denied = knowledge
        .refresh(&DenyAllPermissions, context(), false)
        .await;
    assert!(
        denied.is_err(),
        "a denied workspace document must fail the refresh"
    );
    let status = knowledge.status(KnowledgeScope::Project);
    assert!(status.configured());
    assert_eq!(status.files, 0, "a denied refresh must keep the old layer");
    assert!(status.error.is_some(), "the failure must be reported");
    assert!(
        knowledge
            .search("zqxdenyzqx", Some(KnowledgeScope::Project), 5)
            .expect("search stays available")
            .is_empty()
    );
}

#[tokio::test]
async fn unused_layers_are_skipped_instead_of_failing() {
    let knowledge = WorkbenchKnowledge::official()
        .expect("embedded knowledge should decode")
        .with_layers(None, None, LayerOptions::default())
        .expect("no layers should still be valid");
    let statuses = knowledge
        .refresh(&AllowAllPermissions, context(), true)
        .await
        .expect("an empty configuration should refresh nothing");
    assert!(statuses.is_empty());
    assert!(!knowledge.status(KnowledgeScope::User).configured());
    assert!(!knowledge.status(KnowledgeScope::Project).configured());
    assert!(
        knowledge
            .refresh_layer(KnowledgeScope::User, &AllowAllPermissions, context(), false)
            .await
            .is_err(),
        "refreshing an unconfigured layer must fail"
    );
}
