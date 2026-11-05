//! `/api/jsh`: signing in to Just Superhuman from the phone. The device flow runs on this computer
//! ([`bro_core::jsh`]); the phone shows the code and the link, and the computer's window shows the same
//! state. The token never leaves this computer: nothing here serializes it.

use crate::rest::api_error;
use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use bro_core::jsh::{self, Snapshot};
use serde_json::{Value, json};

/// The bridge's JSON for a snapshot, camelCase throughout.
///
/// `{ signedIn, account?: { user: { name, email }, organization: { name, slug },
///    credits: { available, granted, monthlyAllowance, resetsAt }, claude: { available, reason },
///    host?: { url, status } }, pending?: { userCode, verificationUri, verificationUriComplete, expiresAt },
///    error? }`. Times are milliseconds since the epoch.
pub(crate) fn view(snap: &Snapshot) -> Value {
    let mut out = json!({ "signedIn": snap.signed_in });
    if let Some(a) = snap.account.as_ref().filter(|_| snap.signed_in) {
        let mut account = json!({
            "user": { "name": a.user.name, "email": a.user.email },
            "organization": { "name": a.organization.name, "slug": a.organization.slug },
            "credits": {
                "available": a.credits.available,
                "granted": a.credits.granted,
                "monthlyAllowance": a.credits.monthly_allowance,
                "resetsAt": a.credits.resets_at,
            },
            "claude": { "available": a.claude.available, "reason": a.claude.reason },
        });
        if let Some(host) = &a.host {
            account["host"] = json!({ "url": host.url, "status": host.status });
        }
        out["account"] = account;
    }
    if let Some(p) = &snap.pending {
        out["pending"] = json!({
            "userCode": p.user_code,
            "verificationUri": p.verification_uri,
            "verificationUriComplete": p.verification_uri_complete,
            "expiresAt": p.expires_at,
        });
    }
    if let Some(e) = &snap.error {
        out["error"] = json!(e);
    }
    out
}

fn busy() -> Response {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, "Just Superhuman's sign-in stopped unexpectedly.")
}

/// `GET /api/jsh`: signed in or not, the account's credits, a sign-in waiting for approval, the last problem.
pub(crate) async fn get() -> Response {
    match tokio::task::spawn_blocking(|| jsh::login().snapshot()).await {
        Ok(snap) => Json(view(&snap)).into_response(),
        Err(_) => busy(),
    }
}

/// `POST /api/jsh/login`: start the device flow on this computer and poll it in the background. A second call
/// while one is waiting returns that one. Signed in already: the same answer as GET, no new flow. When Just
/// Superhuman can't be reached: 502 with the same shape (`error` says why, `message` repeats it for clients that
/// read that).
pub(crate) async fn login() -> Response {
    let started = tokio::task::spawn_blocking(|| {
        let me = jsh::login();
        if jsh::signed_in() {
            return Ok(());
        }
        me.begin().map(|_| ())
    })
    .await;
    match started {
        Ok(Ok(())) => get().await,
        Ok(Err(e)) => {
            let mut body = view(&jsh::login().snapshot());
            let text = format!("{e:#}");
            body["error"] = json!(text);
            body["message"] = json!(text);
            (StatusCode::BAD_GATEWAY, Json(body)).into_response()
        }
        Err(_) => busy(),
    }
}

/// `POST /api/jsh/logout`: revoke this computer's token and forget it. Always `{ "signedIn": false }`;
/// `warning` is added when Just Superhuman couldn't be reached to revoke it.
pub(crate) async fn logout() -> Response {
    match tokio::task::spawn_blocking(|| jsh::login().sign_out()).await {
        Ok(out) => {
            let mut body = json!({ "signedIn": false });
            if !out.revoked {
                body["warning"] = json!("Signed out on this computer, but Just Superhuman couldn't be reached to revoke the device. Revoke it under Account, Devices on console.justsuperhuman.com.");
            }
            Json(body).into_response()
        }
        Err(_) => busy(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bro_core::jsh::{Account, Credits, Host, Organization, Pending, User};

    #[test]
    fn view_is_camel_case_and_has_no_token() {
        let account = Account {
            user: User { name: "Sam Rivera".into(), email: "sam@example.com".into() },
            organization: Organization { name: "Rivera Studio".into(), slug: "rivera-studio".into() },
            credits: Credits { available: 1840, granted: 1840, monthly_allowance: 0, resets_at: Some(1795000000000) },
            host: Some(Host { url: "https://rivera-studio.justsuperhuman.com".into(), status: "ready".into() }),
            ..Default::default()
        };
        let pending = Pending { user_code: "WDJB-MJHT".into(), verification_uri: "https://x/connect".into(), verification_uri_complete: "https://x/connect?code=WDJB-MJHT".into(), expires_at: 5 };
        let body = view(&Snapshot { signed_in: true, account: Some(account.clone()), pending: Some(pending.clone()), error: Some("nope".into()), ..Default::default() });
        assert_eq!(body["signedIn"], true);
        assert_eq!(body["account"]["user"], json!({ "name": "Sam Rivera", "email": "sam@example.com" }));
        assert_eq!(body["account"]["organization"], json!({ "name": "Rivera Studio", "slug": "rivera-studio" }));
        assert_eq!(body["account"]["credits"], json!({ "available": 1840, "granted": 1840, "monthlyAllowance": 0, "resetsAt": 1795000000000i64 }));
        assert_eq!(body["account"]["host"]["status"], "ready");
        assert_eq!(body["pending"]["userCode"], "WDJB-MJHT");
        assert_eq!(body["pending"]["verificationUriComplete"], "https://x/connect?code=WDJB-MJHT");
        assert_eq!(body["pending"]["expiresAt"], 5);
        assert_eq!(body["error"], "nope");
        assert!(!body.to_string().contains("jsh_user"));
        // signed out: a stale cache is not shown; no host, no pending, no error keys at all
        let out = view(&Snapshot { signed_in: false, account: Some(account), ..Default::default() });
        assert_eq!(out, json!({ "signedIn": false }));
    }
}
