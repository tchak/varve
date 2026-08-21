//! `/settings/account`, derived from this module's name: the account
//! tab of the settings area — the editable "User profile" card
//! (display name and language preference, saved by [`submit`]) and
//! the read-only "Email address" card.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, bad_request},
        href, page,
    },
    view::{attributes, component, view},
};

use crate::{
    auth::account,
    components::{
        button::button,
        card::{card, card_content, card_footer, card_header, card_title},
        field::field,
        label::label,
        select::select,
    },
    db,
    i18n::{SUPPORTED_LOCALES, request_locale, t},
};

use super::{Tab, settings_shell};

/// A profile submission: the display name and the locale preference
/// picked in the form.
#[derive(Deserialize)]
struct ProfileUpdate {
    name: String,
    locale: String,
}

/// The "User profile" card: one form — the name field and the
/// language select — submitting to [`submit`], with the save button
/// in the card footer. `name` and `locale` are the values the form
/// shows (the stored ones on GET, the submitted ones on a failed
/// save); `name_error` fills the name field's error slot.
///
/// The language options are exactly [`SUPPORTED_LOCALES`], labeled
/// by endonym (each language named in itself — the `locale.*`
/// strings, identical in both catalogs), so the select can only ever
/// submit a locale the platform serves.
#[component]
async fn profile_card(cx: &Cx, name: String, locale: String, name_error: Option<String>) -> Result {
    let profile_title = t(cx, "settings.account.profile.title")?;
    let name_label = t(cx, "form.name")?;
    let language_label = t(cx, "form.language")?;
    let save_label = t(cx, "settings.account.profile.save")?;
    let mut options = Vec::with_capacity(SUPPORTED_LOCALES.len());
    for supported in SUPPORTED_LOCALES {
        options.push((*supported, t(cx, &format!("locale.{supported}"))?));
    }
    // The card is a gapped column of sections; `contents` keeps the
    // form transparent to that layout while it wraps both the fields
    // and the footer's submit button.
    view! {
        card(
            card_header(card_title((profile_title)))
            <form method="post" action=(href!(submit)) class="contents">
                card_content(
                    <div class="flex flex-col gap-4">
                        field(
                            id: "account-name",
                            label: name_label,
                            error: name_error,
                            attrs: attributes! {
                                type="text"
                                name="name"
                                value=(name.as_str())
                                required=""
                                autocomplete="name"
                            }
                        )
                        <div class="flex flex-col gap-2">
                            label(
                                attrs: attributes! { for="account-locale" },
                                (language_label)
                            )
                            select(
                                attrs: attributes! { id="account-locale" name="locale" },
                                for (id, endonym) in &options {
                                    <option value=(*id) selected=(*id == locale.as_str())>
                                        (endonym.as_str())
                                    </option>
                                }
                            )
                        </div>
                    </div>
                )
                card_footer(button(attrs: attributes! { type="submit" }, (save_label)))
            </form>
        )
    }
}

/// The account tab's cards: the [`profile_card`] form and the
/// read-only email card (the address as prose — no input, nothing to
/// save). Shared by the GET page and [`submit`]'s failed-save
/// re-render.
#[component]
async fn account_cards(
    cx: &Cx,
    name: String,
    locale: String,
    name_error: Option<String>,
) -> Result {
    // The settings gate (`super::gate`) already turned anonymous
    // requests away; a missing account here is a wiring defect, not
    // a user state.
    let email = account(cx).ok_or_unauthorized()?.email.clone();
    let email_title = t(cx, "settings.account.email.title")?;
    view! {
        settings_shell(
            active: Tab::Account,
            <div class="flex flex-col gap-6">
                profile_card(name: name, locale: locale, name_error: name_error)
                card(
                    card_header(card_title((email_title)))
                    card_content(<p class="text-sm">(email.as_str())</p>)
                )
            </div>
        )
    }
}

/// The account tab. The form shows the stored name, and the request
/// locale as the selected language: [`request_locale`] is already
/// "the account's stored preference when supported, else the
/// `Accept-Language` negotiation, else English" (the root layer's
/// `resolve_locale`), which is exactly the value the select should
/// present — no second resolution here.
#[page]
pub async fn page(cx: &Cx) -> Result {
    let account = account(cx).ok_or_unauthorized()?;
    let name = account.name.clone();
    let locale = request_locale(cx).to_string();
    view! { account_cards(name: name, locale: locale, name_error: None) }
}

/// Saves the profile. The name is trimmed; an empty result re-renders
/// the form with a localized message in the name field's error slot
/// (and saves nothing). Success stores name and locale through
/// [`platform_core::update_profile`] and answers 303 back to
/// [`page`] — which then renders in the *new* locale by the existing
/// resolution order alone (the follow-up request reloads the account
/// and its stored preference wins), nothing here switches languages.
///
/// A locale outside [`SUPPORTED_LOCALES`] is a 400: the select only
/// ever offers the supported set, so an unsupported value cannot come
/// from the form — only from a forged request, which deserves a bad
/// request, not a friendly re-render.
#[page(POST)]
async fn submit(cx: &Cx, Form(input): Form<ProfileUpdate>) -> Result {
    let account_id = account(cx).ok_or_unauthorized()?.id;
    if !SUPPORTED_LOCALES.contains(&input.locale.as_str()) {
        return Err(bad_request("locale is not supported").into());
    }
    let name = input.name.trim();
    if name.is_empty() {
        let error = t(cx, "settings.account.profile.error.name-required")?;
        return view! {
            account_cards(
                name: String::new(),
                locale: input.locale,
                name_error: Some(error)
            )
        };
    }
    let mut db = db(cx);
    platform_core::update_profile(&mut db, account_id, name, Some(&input.locale)).await?;
    super::super::redirect_to(cx, href!(page).resolve(cx)).await
}
