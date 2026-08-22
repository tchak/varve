//! `/settings/security/tokens/revoke`, derived from this module's
//! name: per-token revocation.

use serde::Deserialize;
use topcoat::{
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, bad_request, see_other},
        href, route,
    },
};

use crate::{
    auth::account,
    db,
    pages::settings::{security, signin_location},
};

/// A revocation submission: the token row to destroy.
#[derive(Deserialize)]
struct Revocation {
    token_id: String,
}

/// Revokes one token and lands back on the security tab. The destroy
/// is the *scoped* [`platform_core::destroy_api_token`]: the
/// authenticated account's id is part of the delete predicate, so a
/// forged `token_id` belonging to another account destroys nothing
/// and still answers the same 303.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(input): Form<Revocation>) -> topcoat::Result<SeeOther> {
    // A `#[route]`, so the settings layout's redirect does not
    // apply: answer the anonymous case the same way explicitly.
    let Some(account) = account(cx).await? else {
        return Ok(see_other(signin_location(cx)));
    };
    let account_id = account.id;
    let token_id: uuid::Uuid = input
        .token_id
        .parse()
        .map_err(|_| bad_request("token_id is not a UUID"))?;
    let mut db = db(cx);
    platform_core::destroy_api_token(&mut db, account_id, token_id).await?;
    Ok(see_other(href!(security::page).resolve(cx)))
}
