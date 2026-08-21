//! `/signout`, derived from this module's name.

use topcoat::{
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, route,
    },
};

use crate::auth::sign_out;

/// Ends the session (idempotent) and returns home.
#[route(POST)]
pub async fn submit(cx: &Cx) -> topcoat::Result<SeeOther> {
    sign_out(cx).await?;
    Ok(see_other(href!(super::home).resolve(cx)))
}
