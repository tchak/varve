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
    router::{content::Form, error::bad_request, href, page},
    view::{View, ViewExt, view},
};

use crate::{auth::require_account, db, flash, i18n::t, pages::redirect_to};

use super::{ISSUED_FLASH, IssuedFlash, TokensForm, security_cards};

/// A creation submission: the token's name.
#[derive(Deserialize)]
pub(super) struct Creation {
    name: String,
}

/// Creates a token, parks the secret in a one-shot private cookie
/// ([`flash`]), and answers 303 to the security tab, which shows the
/// secret **once** as it consumes the flash — post/redirect/get, so a
/// refresh of the landing page neither re-submits the form nor shows
/// the secret again. The row keeps a hash only
/// (`platform_core::api_token`); the encrypted cookie is the secret's
/// only other transit, and it is gone after one GET.
///
/// A blank name re-renders with the error in the field (and creates
/// nothing); a name over `MAX_API_TOKEN_NAME_CHARS` is a 400 — the
/// input's `maxlength` keeps browsers from sending one, so it can
/// only come from a forged request.
#[page(POST)]
pub async fn submit(cx: &Cx, Form(input): Form<Creation>) -> Result<impl View> {
    let account_id = require_account(cx).await?.id;
    let mut db = db(cx);
    match platform_core::create_api_token(&mut db, account_id, &input.name, jiff::Timestamp::now())
        .await
    {
        Ok(issued) => {
            flash::set(
                cx,
                ISSUED_FLASH,
                IssuedFlash {
                    name: issued.token.name,
                    secret: issued.secret,
                },
            )?;
            redirect_to(cx, href!(super::page).resolve(cx))
        }
        Err(CreateApiTokenError::EmptyName) => {
            let error = t(cx, "settings.security.tokens.error.name-required").await?;
            let form = TokensForm {
                name: String::new(),
                name_error: Some(error),
                issued: None,
            };
            Ok(view! { security_cards(tokens_form: form) }.boxed())
        }
        Err(CreateApiTokenError::NameTooLong) => {
            Err(bad_request("the token name is too long").into())
        }
        Err(error) => Err(error.into()),
    }
}
