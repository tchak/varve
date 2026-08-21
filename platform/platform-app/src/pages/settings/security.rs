//! `/settings/security`, derived from this module's name: the
//! security tab of the settings area — the active-sessions list and
//! the per-session revocation POST.

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
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
    user_agent: String,
    details: String,
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
/// newest first — user agent and IP (each falling back to a
/// localized "unknown", so sessions recorded before the metadata
/// columns existed still render a full row), the created/expires
/// dates, a "current session" badge on the row whose token hash
/// matches this request's, and a per-session revocation POST. Each
/// row carries `data-current="true|false"` so tests and styling can
/// address the current row without parsing the badge text.
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

    let mut rows = Vec::with_capacity(sessions.len());
    for session in &sessions {
        let created = t_args(
            cx,
            "settings.security.sessions.created",
            &super::super::one_arg("date", &session.created_at.strftime("%Y-%m-%d").to_string()),
        )?;
        let expires = t_args(
            cx,
            "settings.security.sessions.expires",
            &super::super::one_arg("date", &session.expires_at.strftime("%Y-%m-%d").to_string()),
        )?;
        let ip = session.ip.as_deref().unwrap_or(&unknown);
        rows.push(Row {
            id: session.id.to_string(),
            current: Some(session.id) == current_id,
            user_agent: session.user_agent.as_deref().unwrap_or(&unknown).to_owned(),
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
                                class="flex flex-col gap-2 border-b border-border py-4 \
                                       first:pt-0 last:border-b-0 last:pb-0 sm:flex-row \
                                       sm:items-center sm:justify-between"
                            >
                                <div class="flex min-w-0 flex-col gap-1">
                                    <span class="truncate text-sm font-medium">
                                        (row.user_agent.as_str())
                                    </span>
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
