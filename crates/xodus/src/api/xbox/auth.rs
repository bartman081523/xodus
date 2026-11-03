use crate::models::xbox::{
    UserAuthProperties, UserAuthRequest, XstsPropertyBag, XstsRequest, XstsResponse,
};

pub async fn authenticate_xbox_user(
    client: &reqwest::Client,
    mut rps_ticket: String,
) -> reqwest::Result<XstsResponse> {
    if let Some(stripped) = rps_ticket.strip_prefix("d=") {
        rps_ticket = stripped.to_string();
    }

    let body = UserAuthRequest {
        relying_party: "http://auth.xboxlive.com".to_string(),
        token_type: "JWT".to_string(),
        properties: UserAuthProperties {
            auth_method: "RPS".to_string(),
            site_name: "user.auth.xboxlive.com".to_string(),
            rps_ticket,
        },
    };

    let resp = client
        .post("https://user.auth.xboxlive.com/user/authenticate")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-xbl-contract-version", "1")
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    resp.json().await
}

pub async fn request_xsts_token(
    client: &reqwest::Client,
    token: String,
    relying_party: &str,
) -> reqwest::Result<XstsResponse> {
    let body = XstsRequest {
        relying_party: Some(relying_party.to_string()),
        token_type: Some("JWT".to_string()),
        properties: XstsPropertyBag {
            user_tokens: Some(vec![token]),
            sandbox_id: Some("RETAIL".to_string()),
            delegation_token: None,
            service_token: None,
        },
    };

    let resp = client
        .post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .header("Content-Type", "application/json")
        .header("Accept", "application/json")
        .header("x-xbl-contract-version", "1")
        .json(&body)
        .send()
        .await?
        .error_for_status()?;

    resp.json().await
}

pub fn get_xsts_auth_header(xsts: XstsResponse) -> String {
    let uhs = xsts.user_hash().expect("XSTS response missing xui claim");
    format!("XBL3.0 x={uhs};{}", xsts.token)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_xsts_auth_header_formatting() {
        let json = r#"{
            "NotAfter": "2026-11-04T12:00:00Z",
            "Token": "test_xsts_jwt_token",
            "DisplayClaims": {
                "xui": [
                    {
                        "uhs": "2533274812345678",
                        "gtg": "EuleBruecke7237"
                    }
                ]
            }
        }"#;
        let resp: XstsResponse = serde_json::from_str(json).unwrap();
        assert_eq!(
            get_xsts_auth_header(resp),
            "XBL3.0 x=2533274812345678;test_xsts_jwt_token"
        );
    }
}
