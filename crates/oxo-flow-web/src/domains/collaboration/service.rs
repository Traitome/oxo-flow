use crate::domains::collaboration::types::*;
use crate::infra::db::StorageBackend;
use uuid::Uuid;

/// Fork a pipeline — full copy into user's workspace.
pub async fn fork_pipeline(
    db: &dyn StorageBackend,
    pipeline_id: &str,
    new_owner_id: &str,
) -> Result<ForkResponse, String> {
    let source = db
        .get_pipeline(pipeline_id)
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or_else(|| "Source pipeline not found".to_string())?;

    let forked_id = Uuid::new_v4().to_string();
    let forked_name = format!("{} (fork)", source.name);

    // Create a new pipeline row with the forked content
    use crate::infra::db::models::PipelineRow;
    let new_pipeline = PipelineRow {
        id: forked_id.clone(),
        user_id: new_owner_id.to_string(),
        name: forked_name.clone(),
        version: source.version,
        toml_content: source.toml_content,
        rules_count: source.rules_count,
        forked_from: Some(pipeline_id.to_string()),
        visibility: "private".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    };

    db.save_pipeline(&new_pipeline)
        .await
        .map_err(|e| format!("Failed to save fork: {e}"))?;

    Ok(ForkResponse {
        forked_id,
        name: forked_name,
    })
}

/// Share a pipeline by creating a share link.
pub async fn share_pipeline(
    db: &dyn StorageBackend,
    pipeline_id: &str,
    owner_id: &str,
    visibility: &str,
    expires_in_days: Option<u32>,
) -> Result<ShareResponse, String> {
    let _pipeline = db
        .get_pipeline(pipeline_id)
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or_else(|| "Pipeline not found".to_string())?;

    let token = Uuid::new_v4().to_string();
    let share_id = Uuid::new_v4().to_string();

    let expires_at = expires_in_days.map(|days| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let expiry = now + (days as u64 * 86400);
        expiry.to_string()
    });

    use crate::infra::db::models::ShareRow;
    let share = ShareRow {
        id: share_id,
        pipeline_id: pipeline_id.to_string(),
        owner_id: owner_id.to_string(),
        token: token.clone(),
        visibility: visibility.to_string(),
        expires_at,
        created_at: String::new(),
    };

    db.create_share(&share)
        .await
        .map_err(|e| format!("Failed to create share: {e}"))?;

    Ok(ShareResponse {
        share_url: format!("oxo+https://localhost:8777/share/{token}"),
        access_token: token,
        expires_at: share.expires_at,
    })
}

/// Import a pipeline from a share URL.
pub async fn import_pipeline(
    db: &dyn StorageBackend,
    url: &str,
    importer_id: &str,
) -> Result<ImportResponse, String> {
    // Parse oxo+https:// URL format
    let token = url
        .strip_prefix("oxo+https://")
        .or_else(|| url.strip_prefix("oxo+http://"))
        .and_then(|rest| rest.rsplit('/').next())
        .ok_or_else(|| {
            "Invalid share URL format. Use: oxo+https://host/share/<token>".to_string()
        })?;

    let share = db
        .get_share_by_token(token)
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or_else(|| "Share link not found or expired".to_string())?;

    // Check expiry
    if let Some(ref expiry) = share.expires_at {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .to_string();
        if expiry < &now {
            return Err("Share link has expired".to_string());
        }
    }

    let source = db
        .get_pipeline(&share.pipeline_id)
        .await
        .map_err(|e| format!("DB error: {e}"))?
        .ok_or_else(|| "Source pipeline no longer exists".to_string())?;

    // Import as a new pipeline owned by the importer
    let imported_id = Uuid::new_v4().to_string();
    use crate::infra::db::models::PipelineRow;
    let imported = PipelineRow {
        id: imported_id.clone(),
        user_id: importer_id.to_string(),
        name: format!("{} (imported)", source.name),
        version: source.version,
        toml_content: source.toml_content,
        rules_count: source.rules_count,
        forked_from: Some(share.pipeline_id),
        visibility: "private".to_string(),
        created_at: String::new(),
        updated_at: String::new(),
    };

    db.save_pipeline(&imported)
        .await
        .map_err(|e| format!("Failed to save import: {e}"))?;

    Ok(ImportResponse {
        pipeline_id: imported_id,
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_parse_share_url() {
        let url = "oxo+https://lab.org/share/abc123-def456";
        let token = url
            .strip_prefix("oxo+https://")
            .unwrap()
            .rsplit('/')
            .next()
            .unwrap();
        assert_eq!(token, "abc123-def456");
    }

    #[test]
    fn test_parse_share_url_invalid() {
        let result = "just-a-string".strip_prefix("oxo+https://");
        assert!(result.is_none());
    }
}
