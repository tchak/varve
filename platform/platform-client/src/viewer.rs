//! `viewer`: the account the request executes as.

use crate::schema;

/// `query { viewer { … } }`.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query")]
pub struct ViewerQuery {
    pub viewer: Viewer,
}

/// The P0 account core.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct Viewer {
    pub account_id: cynic::Id,
    pub email: String,
    pub locale: Option<String>,
}
