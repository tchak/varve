//! `/settings/security/tokens`, derived from this module's name: API
//! token creation ([`submit`]); revocation is the [`revoke`]
//! submodule (`/settings/security/tokens/revoke`). Both sit under the
//! `/settings` gate, so a principal is guaranteed.

pub(super) mod revoke;

use platform_core::CreateApiTokenError;
use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{content::Form, error::bad_request, page},
    view::view,
};

use crate::{auth::require_account, db, i18n::t};

use super::{TokensForm, security_cards};

/// A creation submission: the token's name.
#[derive(Deserialize)]
pub(super) struct Creation {
    name: String,
}

/// Creates a token and re-renders the security tab with the secret
/// shown **once** — this response is the only place it ever appears
/// in plaintext; the row keeps a hash (`platform_core::api_token`).
/// A page, not a redirect: carrying the secret across a 303 would
/// need a flash store, and a one-shot render is the simpler, safer
/// shape (a refresh re-submits the form, creating another token,
/// which the browser's resubmission prompt makes deliberate).
///
/// A blank name re-renders with the error in the field (and creates
/// nothing); a name over `MAX_API_TOKEN_NAME_CHARS` is a 400 — the
/// input's `maxlength` keeps browsers from sending one, so it can
/// only come from a forged request.
#[page(POST)]
pub async fn submit(cx: &Cx, Form(input): Form<Creation>) -> Result {
    let account_id = require_account(cx).await?.id;
    let mut db = db(cx);
    match platform_core::create_api_token(&mut db, account_id, &input.name, jiff::Timestamp::now())
        .await
    {
        Ok(issued) => {
            let form = TokensForm {
                issued: Some((issued.token.name, issued.secret)),
                ..TokensForm::default()
            };
            view! { security_cards(tokens_form: form) }
        }
        Err(CreateApiTokenError::EmptyName) => {
            let error = t(cx, "settings.security.tokens.error.name-required").await?;
            let form = TokensForm {
                name: String::new(),
                name_error: Some(error),
                issued: None,
            };
            view! { security_cards(tokens_form: form) }
        }
        Err(CreateApiTokenError::NameTooLong) => {
            Err(bad_request("the token name is too long").into())
        }
        Err(error) => Err(error.into()),
    }
}
