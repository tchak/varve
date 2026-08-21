//! `/settings/security`, derived from this module's name: the
//! security tab of the settings area — the active-sessions list and
//! the per-session revocation POST.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    icon::{IconData, icon, iconify::iconify_icon},
    router::{
        content::Form,
        error::{RouterErrorExt, SeeOther, bad_request, see_other},
        href, page, route,
    },
    session,
    view::{attributes, view},
};

use crate::{
    auth::{account, encode_token_hash},
    components::{
        badge::{BadgeVariant, badge},
        button::{ButtonSize, ButtonVariant, button},
        card::{card, card_content, card_header, card_title},
    },
    db,
    i18n::{t, t_args},
    ua,
};

use super::{Tab, settings_shell};

/// A per-session revocation submission: the session row to destroy.
#[derive(Deserialize)]
struct Revocation {
    session_id: String,
}

/// One session row, fully localized and formatted ahead of the view.
struct Row {
    id: String,
    current: bool,
    /// The brand icon for the parsed browser family, or the generic
    /// globe.
    icon: IconData,
    /// The display title: `"Chrome 129 · macOS"` from [`ua::describe`],
    /// or the localized unknown-browser fallback.
    title: String,
    /// The raw stored user-agent string, carried on the title span's
    /// `title=` attribute as a tooltip — the forensic detail the
    /// parsed title summarizes. Absent for sessions recorded without
    /// one.
    user_agent: Option<String>,
    details: String,
}

/// Composes the row title from a parsed [`ua::Browser`]: family, then
/// the major version, then the OS family, absent parts omitted —
/// `"Chrome 129 · macOS"`, `"Safari · Mac OS X"`, `"Firefox 130"`.
/// Proper nouns all the way down, so the composition is
/// locale-neutral and happens outside MF2.
fn browser_title(browser: &ua::Browser) -> String {
    let mut title = browser.family.clone();
    if let Some(major) = &browser.major {
        title.push(' ');
        title.push_str(major);
    }
    if let Some(os) = &browser.os {
        title.push_str(" · ");
        title.push_str(os);
    }
    title
}

/// The brand icon for a parsed browser family, keyed on the uap-core
/// family name (which spells variants as e.g. `"Chrome Mobile iOS"`,
/// `"Mobile Safari"`, `"Edge Mobile"`, `"Opera Mini"` — hence the
/// substring matches). Anything unrecognized — including the
/// unknown-browser fallback row — gets the generic feather globe.
/// Each `iconify_icon!` id resolves against the staged set at compile
/// time (`build.rs`), so a mistyped id fails the build.
fn browser_icon(family: &str) -> IconData {
    if family.contains("Chrome") || family == "Chromium" {
        iconify_icon!("simple-icons:googlechrome")
    } else if family.contains("Firefox") {
        iconify_icon!("simple-icons:firefox")
    } else if family.contains("Safari") {
        iconify_icon!("simple-icons:safari")
    } else if family.contains("Edge") {
        iconify_icon!("simple-icons:microsoftedge")
    } else if family.contains("Opera") {
        iconify_icon!("simple-icons:opera")
    } else {
        iconify_icon!("feather:globe")
    }
}

/// The id of the session row backing *this* request, when the
/// presented token resolves to one — the row [`page`] marks as the
/// current session and [`submit`] treats as a full sign-out.
async fn current_session_id(cx: &Cx, db: &mut toasty::Db) -> Result<Option<uuid::Uuid>> {
    let Some(hash) = session::token_hash(cx).await? else {
        return Ok(None);
    };
    let row =
        platform_core::find_live_session(db, &encode_token_hash(&hash), jiff::Timestamp::now())
            .await?;
    Ok(row.map(|row| row.id))
}

/// The security tab: one card listing the account's live sessions,
/// newest first. Each row leads with the parsed browser — brand icon
/// and "Chrome 129 · macOS" title ([`ua::describe`] at render time,
/// the raw stored string kept as the title's tooltip), falling back
/// to a localized "unknown browser" when the string is absent or
/// identifies nothing — then a muted meta line (IP, falling back to
/// the localized "unknown" so pre-metadata sessions still render a
/// full row, with the created/expires dates), a "current session"
/// badge on the row whose token hash matches this request's, and a
/// per-session revocation POST. Each row carries
/// `data-current="true|false"` so tests and styling can address the
/// current row without parsing the badge text.
#[page]
pub async fn page(cx: &Cx) -> Result {
    // The settings gate (`super::gate`) already turned anonymous
    // requests away; a missing account here is a wiring defect, not
    // a user state.
    let account = account(cx).ok_or_unauthorized()?;
    let mut db = db(cx);
    let sessions =
        platform_core::list_live_sessions(&mut db, account.id, jiff::Timestamp::now()).await?;
    let current_id = current_session_id(cx, &mut db).await?;

    let sessions_title = t(cx, "settings.security.sessions.title")?;
    let current_label = t(cx, "settings.security.sessions.current")?;
    let revoke_label = t(cx, "settings.security.sessions.revoke")?;
    let unknown = t(cx, "settings.security.sessions.unknown")?;
    let unknown_browser = t(cx, "settings.security.sessions.unknown-browser")?;

    let mut rows = Vec::with_capacity(sessions.len());
    for session in &sessions {
        let created = t_args(
            cx,
            "settings.security.sessions.created",
            &super::super::one_arg("date", super::super::utc_date_arg(session.created_at)),
        )?;
        let expires = t_args(
            cx,
            "settings.security.sessions.expires",
            &super::super::one_arg("date", super::super::utc_date_arg(session.expires_at)),
        )?;
        let ip = session.ip.as_deref().unwrap_or(&unknown);
        let browser = session.user_agent.as_deref().and_then(ua::describe);
        rows.push(Row {
            id: session.id.to_string(),
            current: Some(session.id) == current_id,
            icon: browser_icon(browser.as_ref().map_or("", |browser| &browser.family)),
            title: browser
                .as_ref()
                .map_or_else(|| unknown_browser.clone(), browser_title),
            user_agent: session.user_agent.clone(),
            details: format!("{ip} · {created} · {expires}"),
        });
    }

    view! {
        settings_shell(
            active: Tab::Security,
            card(
                card_header(card_title((sessions_title)))
                card_content(
                    <ul class="flex flex-col">
                        for row in &rows {
                            <li
                                data-current=(if row.current { "true" } else { "false" })
                                class="flex items-center gap-3 border-b border-border \
                                       py-4 first:pt-0 last:border-b-0 last:pb-0"
                            >
                                <div class="flex min-w-0 flex-1 flex-col gap-1">
                                    <div class="flex min-w-0 items-center gap-2">
                                        icon(
                                            data: row.icon.clone(),
                                            attrs: attributes! {
                                                class="size-4 shrink-0 \
                                                       text-muted-foreground"
                                            }
                                        )
                                        <span
                                            class="truncate text-sm font-medium"
                                            title=(row.user_agent.as_deref())
                                        >
                                            (row.title.as_str())
                                        </span>
                                    </div>
                                    <span class="text-sm text-muted-foreground">
                                        (row.details.as_str())
                                    </span>
                                </div>
                                <div class="flex shrink-0 items-center gap-2">
                                    if row.current {
                                        badge(
                                            variant: BadgeVariant::Secondary,
                                            (current_label.as_str())
                                        )
                                    }
                                    <form method="post" action=(href!(submit))>
                                        <input
                                            type="hidden"
                                            name="session_id"
                                            value=(row.id.as_str())
                                        >
                                        button(
                                            variant: ButtonVariant::Outline,
                                            size: ButtonSize::Sm,
                                            attrs: attributes! { type="submit" },
                                            (revoke_label.as_str())
                                        )
                                    </form>
                                </div>
                            </li>
                        }
                    </ul>
                )
            )
        )
    }
}

/// Revokes one session. The destroy is the *scoped*
/// [`platform_core::destroy_session`] — the authenticated account's
/// id is part of the delete predicate, so a forged `session_id`
/// belonging to another account destroys nothing (and still answers
/// the same 303, indistinguishable from an already-revoked id).
///
/// Revoking another session lands back on the security tab; revoking
/// the *current* one is a full sign-out — topcoat drops the client
/// token (`session::stop`) and the scoped destroy deletes the row —
/// landing home like `/signout`.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(input): Form<Revocation>) -> topcoat::Result<SeeOther> {
    let account_id = account(cx).ok_or_unauthorized()?.id;
    let session_id: uuid::Uuid = input
        .session_id
        .parse()
        .map_err(|_| bad_request("session_id is not a UUID"))?;
    let mut db = db(cx);
    let revoking_current = current_session_id(cx, &mut db).await? == Some(session_id);
    if revoking_current {
        let _ = session::stop(cx).await?;
    }
    platform_core::destroy_session(&mut db, account_id, session_id).await?;
    if revoking_current {
        Ok(see_other(href!(super::super::home).resolve(cx)))
    } else {
        Ok(see_other(href!(page).resolve(cx)))
    }
}
