//! `/settings/account`, derived from this module's name: the account
//! tab of the settings area — profile information as cards.

use topcoat::{
    Result,
    context::Cx,
    router::{error::RouterErrorExt, page},
    view::view,
};

use crate::{
    auth::account,
    components::card::{card, card_content, card_header, card_title},
    i18n::t,
};

use super::{Tab, settings_shell};

/// The account tab: the "User profile" card, showing the signed-in
/// account's name and email off the row the request-state layer
/// already loaded ([`account`]). More cards join as account
/// management grows.
#[page]
pub async fn page(cx: &Cx) -> Result {
    // The settings gate (`super::gate`) already turned anonymous
    // requests away; a missing account here is a wiring defect, not
    // a user state.
    let account = account(cx).ok_or_unauthorized()?;
    let profile_title = t(cx, "settings.account.profile.title")?;
    let name_label = t(cx, "form.name")?;
    let email_label = t(cx, "form.email")?;
    view! {
        settings_shell(
            active: Tab::Account,
            card(
                card_header(card_title((profile_title)))
                card_content(
                    <dl class="flex flex-col gap-3">
                        <div class="flex flex-col gap-0.5">
                            <dt class="text-sm text-muted-foreground">(name_label)</dt>
                            <dd class="text-sm font-medium">(account.name.as_str())</dd>
                        </div>
                        <div class="flex flex-col gap-0.5">
                            <dt class="text-sm text-muted-foreground">(email_label)</dt>
                            <dd class="text-sm font-medium">
                                (account.email.as_str())
                            </dd>
                        </div>
                    </dl>
                )
            )
        )
    }
}
