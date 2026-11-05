#![cfg(feature = "postgres")]

//! Live PostgreSQL parity test for every StorageBackend surface touched by
//! the web domains.
//!
//! Set `OXO_FLOW_TEST_POSTGRES_URL` to a disposable database to run it:
//!
//! ```text
//! OXO_FLOW_TEST_POSTGRES_URL=postgres://postgres:postgres@127.0.0.1:5432/oxo_test
//! cargo test -p oxo-flow-web --features postgres --test postgres_storage_contract
//! ```

use oxo_flow_web::infra::db::models;
use oxo_flow_web::infra::db::postgres::PostgresBackend;
use oxo_flow_web::infra::db::{Pagination, StorageBackend};
use uuid::Uuid;

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

#[tokio::test]
async fn storage_backend_contract_round_trips_on_postgres() {
    let Some(url) = std::env::var("OXO_FLOW_TEST_POSTGRES_URL").ok() else {
        eprintln!("skipping PostgreSQL contract test: OXO_FLOW_TEST_POSTGRES_URL is unset");
        return;
    };

    let backend = PostgresBackend::new(&url)
        .await
        .expect("connect to disposable PostgreSQL database");
    backend.init().await.expect("initialize PostgreSQL schema");

    let suffix = Uuid::new_v4().simple().to_string();
    let user = backend
        .create_user(&format!("pg_contract_{suffix}"), "admin", Some("hash"))
        .await
        .expect("create user");
    let other_user = backend
        .create_user(&format!("pg_contract_other_{suffix}"), "user", Some("hash"))
        .await
        .expect("create second user");
    let timestamp = now();

    let session = format!("session-{suffix}");
    let session_expires = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
    backend
        .create_session(&user.id, &session, &session_expires)
        .await
        .expect("create session");
    assert_eq!(
        backend
            .get_session(&session)
            .await
            .expect("get session")
            .expect("session exists")
            .user_id,
        user.id
    );

    let key = models::ApiKeyRow {
        id: Uuid::new_v4().to_string(),
        user_id: user.id.clone(),
        name: "contract-key".to_string(),
        key_hash: format!("hash-{suffix}"),
        created_at: timestamp.clone(),
        last_used_at: None,
        revoked: 0,
    };
    backend.create_api_key(&key).await.expect("create API key");
    assert_eq!(
        backend
            .resolve_api_key(&key.key_hash, &timestamp)
            .await
            .expect("resolve API key"),
        Some((user.id.clone(), "admin".to_string()))
    );

    backend
        .store_oauth_state(&format!("state-{suffix}"), &timestamp)
        .await
        .expect("store OAuth state");
    let oauth_state = format!("state-{suffix}");
    let (first, second) = tokio::join!(
        backend.consume_oauth_state(&oauth_state),
        backend.consume_oauth_state(&oauth_state)
    );
    let consumed = usize::from(first.expect("first OAuth consume").is_some())
        + usize::from(second.expect("second OAuth consume").is_some());
    assert_eq!(
        consumed, 1,
        "a CSRF state must be consumed by exactly one callback"
    );
    assert!(
        backend
            .consume_oauth_state(&oauth_state)
            .await
            .expect("consume OAuth state twice")
            .is_none()
    );

    backend
        .record_audit_log(
            &user.id,
            "contract",
            "/postgres",
            "success",
            Some("{\"ok\":true}"),
        )
        .await
        .expect("record audit log");
    let (logs, total) = backend
        .list_audit_logs(&timestamp, 10, 0)
        .await
        .expect("list audit logs");
    assert!(total >= 1 && logs.iter().any(|row| row.user_id == user.id));
    assert!(
        backend
            .list_recent_audit_logs(10)
            .await
            .expect("list recent audit logs")
            .iter()
            .any(|row| row.user_id == user.id)
    );

    backend
        .save_webhook_config(
            "https://example.invalid/hook",
            Some("secret"),
            true,
            "[\"run.completed\"]",
            "sha256-keyed",
        )
        .await
        .expect("save webhook config");
    let webhook = backend
        .get_webhook_config()
        .await
        .expect("get webhook config")
        .expect("webhook row exists");
    assert_eq!(webhook.url, "https://example.invalid/hook");
    assert_eq!(webhook.enabled, 1);

    let chat_session = format!("chat-{suffix}");
    assert!(
        backend
            .save_chat_user_message(&chat_session, &user.id, "Contract", "hello")
            .await
            .expect("save user chat message")
    );
    assert!(
        backend
            .save_chat_assistant_message(&chat_session, &user.id, "world")
            .await
            .expect("save assistant chat message")
    );
    assert!(
        !backend
            .is_chat_session_writable(&chat_session, &other_user.id)
            .await
            .expect("check chat ownership")
    );
    assert!(
        !backend
            .save_chat_user_message(&chat_session, &other_user.id, "Hijack", "blocked")
            .await
            .expect("reject cross-user chat write")
    );
    assert!(
        backend
            .list_chat_sessions(&user.id, 10)
            .await
            .expect("list chat sessions")
            .iter()
            .any(|row| row.id == chat_session)
    );
    for trial in 0..16 {
        let contested_chat = format!("contested-chat-{trial}-{suffix}");
        let (first, second) = tokio::join!(
            backend.save_chat_user_message(&contested_chat, &user.id, "First", "hello"),
            backend.save_chat_user_message(&contested_chat, &other_user.id, "Second", "hello")
        );
        let accepted = usize::from(first.expect("first chat writer"))
            + usize::from(second.expect("second chat writer"));
        assert_eq!(
            accepted, 1,
            "a chat session id must be claimed by exactly one concurrent writer"
        );
    }

    let pipeline = models::PipelineRow {
        id: Uuid::new_v4().to_string(),
        user_id: user.id.clone(),
        name: "PostgreSQL contract".to_string(),
        version: "1.0.0".to_string(),
        toml_content: "[workflow]\nname = \"contract\"\n".to_string(),
        rules_count: 1,
        forked_from: None,
        visibility: "private".to_string(),
        created_at: timestamp.clone(),
        updated_at: timestamp.clone(),
    };
    let pipeline = backend
        .save_pipeline(&pipeline)
        .await
        .expect("save pipeline");
    backend
        .record_pipeline_revision(&pipeline.id, &user.id, "1.0.0", &pipeline.toml_content)
        .await
        .expect("record pipeline revision");
    let revisions = backend
        .list_pipeline_revisions(&pipeline.id, 10)
        .await
        .expect("list pipeline revisions");
    assert_eq!(revisions.len(), 1);
    assert!(
        backend
            .get_pipeline_revision(&pipeline.id, &revisions[0].id)
            .await
            .expect("get pipeline revision")
            .is_some()
    );
    assert!(
        backend
            .search_pipelines("PostgreSQL", Some(&user.id), true, 10)
            .await
            .expect("search pipelines")
            .iter()
            .any(|row| row.id == pipeline.id)
    );
    assert!(
        backend
            .list_pipelines_visible(&user.id, false, 10)
            .await
            .expect("list visible pipelines")
            .iter()
            .any(|row| row.id == pipeline.id)
    );

    let run = models::RunRow {
        id: Uuid::new_v4().to_string(),
        user_id: user.id.clone(),
        pipeline_id: Some(pipeline.id.clone()),
        pipeline_snapshot: pipeline.toml_content.clone(),
        workflow_name: Some("contract".to_string()),
        status: "queued".to_string(),
        phase: "parsing".to_string(),
        dry_run: false,
        pid: None,
        workdir: Some(format!("/tmp/oxo-pg-contract-{suffix}")),
        config_overrides: None,
        started_at: None,
        finished_at: None,
        created_at: timestamp.clone(),
    };
    assert!(
        backend
            .create_run_if_workdir_idle(&run)
            .await
            .expect("claim run workdir")
    );
    assert!(
        backend
            .mark_run_running(&run.id, &timestamp)
            .await
            .expect("mark run running")
    );
    backend
        .upsert_run_node(&models::RunNodeRow {
            run_id: run.id.clone(),
            rule_name: "contract".to_string(),
            status: "completed".to_string(),
            exit_code: Some(0),
            started_at: Some(timestamp.clone()),
            finished_at: Some(timestamp.clone()),
        })
        .await
        .expect("upsert run node");
    assert_eq!(
        backend
            .list_run_nodes(&run.id)
            .await
            .expect("list run nodes")
            .len(),
        1
    );
    let page = backend
        .list_runs(
            &user.id,
            Pagination {
                page: 1,
                per_page: 10,
            },
        )
        .await
        .expect("list runs");
    assert!(page.items.iter().any(|row| row.id == run.id));
    let cursor = backend
        .list_runs_cursor(&models::RunFilters {
            user_id: Some(user.id.clone()),
            status: Some("running".to_string()),
            q: Some("contract".to_string()),
            cursor: None,
            limit: 10,
        })
        .await
        .expect("list runs with cursor");
    assert!(cursor.items.iter().any(|row| row.id == run.id));

    let cluster = models::ClusterRow {
        id: Uuid::new_v4().to_string(),
        name: format!("contract-{suffix}"),
        ssh_host: "localhost".to_string(),
        ssh_port: 22,
        ssh_user: None,
        ssh_key: None,
        scheduler: Some("slurm".to_string()),
        remote_dir: None,
        enabled: true,
        created_at: timestamp.clone(),
    };
    backend.save_cluster(&cluster).await.expect("save cluster");
    assert!(backend.get_cluster(&cluster.id).await.unwrap().is_some());
    assert!(backend.delete_cluster(&cluster.id).await.unwrap());

    let ai_config = models::AiProviderConfigRow {
        id: Uuid::new_v4().to_string(),
        user_id: Some(user.id.clone()),
        provider: "openai".to_string(),
        api_key: Some("secret".to_string()),
        api_url: Some("https://api.example.invalid/v1".to_string()),
        model: Some("contract".to_string()),
        search_enabled: 1,
        monitor_enabled: 1,
        auto_retry_enabled: 0,
        max_correction_rounds: 2,
        created_at: timestamp.clone(),
        updated_at: timestamp.clone(),
    };
    backend
        .save_ai_provider_config(&ai_config, false)
        .await
        .expect("save user AI config");
    backend
        .save_ai_provider_config(&ai_config, true)
        .await
        .expect("preserve user AI key");
    let server_ai_config = models::AiProviderConfigRow {
        id: Uuid::new_v4().to_string(),
        user_id: None,
        ..ai_config.clone()
    };
    backend
        .save_ai_provider_config(&server_ai_config, false)
        .await
        .expect("insert server AI config");
    backend
        .save_ai_provider_config(&server_ai_config, true)
        .await
        .expect("update server AI config");

    backend.delete_user(&user.id).await.expect("delete user");
    backend
        .delete_user(&other_user.id)
        .await
        .expect("delete second user");
    assert!(backend.get_session(&session).await.unwrap().is_none());
    assert!(
        backend
            .resolve_api_key(&key.key_hash, &timestamp)
            .await
            .unwrap()
            .is_none()
    );
}
