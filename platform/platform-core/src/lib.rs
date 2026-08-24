//! Platform crate (design/platform.md P.3): Toasty models for platform-owned
//! data and the use-case services over them — each use case will
//! compose one `varve-service` operation with its platform side
//! effects in exactly one place.
//!
//! **Current scope: the P0 auth foundations and the catalog shell** (P.8, outside-in
//! ordering — the platform shell before the kernel edge). That means
//! the account model with credential verification ([`account`]), the
//! server side of browser sessions ([`session`]), the database
//! bootstrap ([`db`]), and the minimal [`Principal`] both transports
//! resolve to (P.7). The rest of the P.3 inventory — procedure
//! catalog, team membership, messages, API tokens, webhook
//! subscriptions, notification outbox — arrives with later phases
//! (P1–P3), as do the kernel-facing use-case services. The first
//! kernel edge is the **procedure draft** ([`procedure`] +
//! [`tree_edit`]): the authored tree (P.4) edited in memory and
//! parked as platform JSON on the catalog row until publication —
//! which derives the kernel schema from it ([`tree::Tree::schema`])
//! and is where `varve-service` will eventually plug in.
//!
//! Deliberate boundaries:
//!
//! - No web framework here. `topcoat` (sessions, cookies, routing)
//!   lives in `platform-app`; this crate only exposes the storage
//!   primitives its session layer adapts to (token *hashes*, never
//!   raw tokens — see [`session`]).
//! - No permission model. Authorization reduces to surface assignment
//!   in the kernel (DESIGN §2.9); the platform roles and party ids
//!   join [`Principal`] with kernel integration (P.7).

#![forbid(unsafe_code)]

pub mod account;
pub mod api_token;
pub mod db;
pub mod organization;
pub mod principal;
pub mod procedure;
pub mod procedure_event;
pub mod procedure_state;
pub mod publish;
pub mod session;
pub mod surfaces;
pub mod team;
pub mod tree;
pub mod tree_edit;

pub use account::{
    Account, AuthError, RegisterError, find_account, register, update_profile, verify_credentials,
};
pub use api_token::{
    API_TOKEN_LIFETIME_MONTHS, ApiToken, CreateApiTokenError, IssuedToken,
    MAX_API_TOKEN_NAME_CHARS, api_token_lifetime, create_api_token, destroy_api_token,
    find_live_api_token, list_live_api_tokens, sweep_expired_api_tokens,
};
pub use db::{MIGRATIONS, connect, connect_with};
pub use organization::{
    CreateOrganizationError, Member, Organization, OrganizationMembership, add_organization_member,
    count_organization_members, count_organization_procedures, count_organization_teams,
    create_organization, create_organization_for, find_organization, find_organization_by_slug,
    is_organization_member, list_account_organizations, list_organization_members, normalize_slug,
    remove_organization_member,
};
pub use principal::Principal;
pub use procedure::{
    LifecycleError, Procedure, RevisionDraft, RevisionDraftError, TreeBytes, close_procedure,
    create_procedure, current_state, discard_revision_draft, edit_revision_draft, find_procedure,
    find_procedure_with_revision_draft, list_account_procedures, list_organization_procedures,
    reopen_procedure, revision_draft_tree,
};
pub use procedure_event::{
    FactsBytes, ProcedureEvent, ProcedureEventKind, PublishedFacts, list_procedure_events,
};
pub use procedure_state::{CorruptState, ProcedureState, ProcedureStateValue, TransitionError};
pub use publish::{
    PublishProcedureError, PublishProcedureOutcome, SharedExecutor, draft_report, publish_procedure,
};
pub use session::{
    DEFAULT_SESSION_TTL, MAX_USER_AGENT_CHARS, Session, create_session, delete_account_sessions,
    delete_session, destroy_session, find_live_session, list_live_sessions, sweep_expired,
};
pub use surfaces::{APPLICANT_SURFACE, REVIEWER_SURFACE, SurfacePair, compile_surfaces};
pub use team::{
    Team, TeamMembership, add_team_member, create_team, find_team, is_team_member,
    list_account_teams, list_organization_teams, list_team_members, remove_team_member,
};
pub use tree::{
    Audience, Tree, TreeColumn, TreeDecodeError, TreeElement, TreeGroup, TreeNote, TreeSection,
};
pub use tree_edit::{
    ColumnPatch, EditError, ElementId, GroupPatch, NotePatch, Parent, Placement, SectionPatch,
    add_element, effective_audience, list_capable, move_element, new_column_id, new_group_id,
    new_node_id, new_option_id, remove_element, update_column, update_group, update_note,
    update_section,
};
