use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForkResponse {
    pub forked_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareRequest {
    pub visibility: String,
    pub expires_in_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareResponse {
    pub share_url: String,
    pub access_token: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportRequest {
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportResponse {
    pub pipeline_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fork_response_roundtrip() {
        let resp = ForkResponse {
            forked_id: "p2".into(),
            name: "forked-pipeline".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ForkResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.forked_id, resp.forked_id);
    }

    #[test]
    fn test_share_request_roundtrip() {
        let req = ShareRequest {
            visibility: "public".into(),
            expires_in_days: Some(7),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ShareRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.visibility, req.visibility);
    }

    #[test]
    fn test_share_response_roundtrip() {
        let resp = ShareResponse {
            share_url: "https://example.com/share/abc".into(),
            access_token: "token123".into(),
            expires_at: Some("2024-01-08".into()),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ShareResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.access_token, resp.access_token);
    }

    #[test]
    fn test_import_roundtrip() {
        let req = ImportRequest {
            url: "https://example.com/pipeline".into(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ImportRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back.url, req.url);

        let resp = ImportResponse {
            pipeline_id: "p_imported".into(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let back: ImportResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pipeline_id, resp.pipeline_id);
    }
}
