//! The case-file catalog slice (G.13): `createCaseFile`, the
//! viewer-scoped `caseFiles` connection, the `caseFile(id)` lookup
//! and `procedure.caseFiles`. Cells and `submitCaseFile` join with
//! the kernel edge.

// The derives resolve their markers through `schema` in scope.
use crate::schema;

/// Variables of [`CaseFileQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CaseFileVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { caseFile(id: $id) { … } }`; `None` for an
/// absent or invisible case file (G.6 — visible to participants and
/// the owning organization's members).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "CaseFileVariables")]
pub struct CaseFileQuery {
    #[arguments(id: $id)]
    pub case_file: Option<CaseFile>,
}

/// The full case file.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct CaseFile {
    pub id: cynic::Id,
    pub state: CaseFileState,
    pub created_at: jiff::Timestamp,
    pub updated_at: jiff::Timestamp,
    pub procedure: ProcedureRef,
    pub participants: Vec<CaseFileParticipant>,
}

/// The lifecycle state union (G.2 rule 5); one member until the
/// checkpoint machine lands.
#[derive(cynic::InlineFragments, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "CaseFileState")]
pub enum CaseFileState {
    Draft(CaseFileDraftState),
    /// A state this client predates.
    #[cynic(fallback)]
    Unknown,
}

/// Being filled, never submitted.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "CaseFileDraftState")]
pub struct CaseFileDraftState {
    pub created_at: jiff::Timestamp,
}

/// The bare state, as list rows carry it.
#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
#[cynic(graphql_type = "CaseFileStateValue")]
pub enum CaseFileStateValue {
    Draft,
}

/// A participation link: the account plus when it joined.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct CaseFileParticipant {
    pub account: AccountRef,
    pub joined_at: jiff::Timestamp,
}

/// An account as participants name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct AccountRef {
    pub id: cynic::Id,
    pub name: String,
    pub email: String,
}

/// A procedure as case files name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct ProcedureRef {
    pub id: cynic::Id,
    pub title: String,
    pub organization: OrganizationRef,
}

/// An organization as ancestors name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct OrganizationRef {
    pub id: cynic::Id,
    pub name: String,
}

/// Variables of [`CaseFilesQuery`] and [`ProcedureCaseFilesQuery`]:
/// the forward-only page (G.13).
#[derive(cynic::QueryVariables, Debug)]
pub struct CaseFilesVariables {
    pub first: Option<i32>,
    pub after: Option<String>,
}

/// `query($first: Int, $after: String) { caseFiles(first: $first,
/// after: $after) { … } }`: the case files the viewer can see —
/// today, participates in — newest first.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "CaseFilesVariables")]
pub struct CaseFilesQuery {
    #[arguments(first: $first, after: $after)]
    pub case_files: CaseFileConnection,
}

/// One page of case files.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct CaseFileConnection {
    pub edges: Vec<CaseFileEdge>,
    pub page_info: PageInfo,
}

/// One row of a connection page.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct CaseFileEdge {
    pub cursor: String,
    pub node: CaseFileRef,
}

/// Forward-only paging facts.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct PageInfo {
    pub has_next_page: bool,
    pub end_cursor: Option<String>,
}

/// A case file as lists name it.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
pub struct CaseFileRef {
    pub id: cynic::Id,
    pub state: CaseFileStateValue,
    pub created_at: jiff::Timestamp,
    pub procedure: ProcedureRef,
}

/// Variables of [`ProcedureCaseFilesQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct ProcedureCaseFilesVariables {
    pub id: cynic::Id,
    pub first: Option<i32>,
    pub after: Option<String>,
}

/// `query($id: ID!, $first: Int, $after: String) { procedure(id: $id)
/// { caseFiles(first: $first, after: $after) { … } } }`: all of an
/// administered procedure's case files, newest first.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "ProcedureCaseFilesVariables")]
pub struct ProcedureCaseFilesQuery {
    #[arguments(id: $id)]
    pub procedure: Option<ProcedureCaseFiles>,
}

/// The one field this query reads off the full procedure.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Procedure", variables = "ProcedureCaseFilesVariables")]
pub struct ProcedureCaseFiles {
    #[arguments(first: $first, after: $after)]
    pub case_files: CaseFileConnection,
}

/// `createCaseFile` input.
#[derive(cynic::InputObject, Debug, Clone)]
pub struct CreateCaseFileInput {
    pub procedure_id: cynic::Id,
}

/// Variables of [`CreateCaseFile`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CreateCaseFileVariables {
    pub input: CreateCaseFileInput,
}

/// `mutation($input: CreateCaseFileInput!) { createCaseFile(input: $input) { … } }`;
/// the viewer becomes the first participant. Fails with
/// `INVALID_TRANSITION` on a closed procedure, `FORBIDDEN` on a
/// never-published or missing one (G.13).
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "CreateCaseFileVariables")]
pub struct CreateCaseFile {
    #[arguments(input: $input)]
    pub create_case_file: CaseFile,
}

#[cfg(test)]
mod tests {
    use super::*;
    use cynic::{MutationBuilder, QueryBuilder};

    #[test]
    fn builds_the_connection_with_its_page_arguments() {
        let operation = CaseFilesQuery::build(CaseFilesVariables {
            first: Some(2),
            after: Some("abc".into()),
        });
        let document = serde_json::to_value(&operation).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(
            query.contains("caseFiles(first: $first, after: $after)"),
            "{query}"
        );
        assert!(query.contains("pageInfo"), "{query}");
        assert_eq!(document["variables"]["first"], 2);
        assert_eq!(document["variables"]["after"], "abc");
    }

    #[test]
    fn builds_the_creation_with_its_input() {
        let operation = CreateCaseFile::build(CreateCaseFileVariables {
            input: CreateCaseFileInput {
                procedure_id: cynic::Id::new("abc"),
            },
        });
        let document = serde_json::to_value(&operation).unwrap();
        let query = document["query"].as_str().unwrap();
        assert!(query.starts_with("mutation"), "{query}");
        assert!(query.contains("createCaseFile(input: $input)"), "{query}");
        assert_eq!(document["variables"]["input"]["procedureId"], "abc");
    }
}

/// Variables of [`CaseFileRecordQuery`].
#[derive(cynic::QueryVariables, Debug)]
pub struct CaseFileRecordVariables {
    pub id: cynic::Id,
}

/// `query($id: ID!) { caseFile(id: $id) { cells items findings } }`
/// — the record read model (G.14: the G.12 shapes on the case
/// file), separate from [`CaseFileQuery`] so catalog reads stay
/// metadata-only.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "CaseFileRecordVariables")]
pub struct CaseFileRecordQuery {
    #[arguments(id: $id)]
    pub case_file: Option<CaseFileRecord>,
}

/// The three record fields of the full case file.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "CaseFile")]
pub struct CaseFileRecord {
    pub cells: Vec<crate::preview::Cell>,
    /// Every `many` group's ordered item list — a freshly added
    /// item's server-minted id is read here.
    pub items: Vec<crate::preview::ItemList>,
    pub findings: Vec<crate::preview::AdmissibilityFinding>,
}

/// `updateCells` input: an ordered batch of writes (G.14).
#[derive(cynic::InputObject, Debug, Clone)]
pub struct UpdateCellsInput {
    pub case_file_id: cynic::Id,
    pub writes: Vec<crate::preview::CellWriteInput>,
}

/// Variables of [`UpdateCells`].
#[derive(cynic::QueryVariables, Debug)]
pub struct UpdateCellsVariables {
    pub input: UpdateCellsInput,
}

/// `mutation($input: UpdateCellsInput!) { updateCells(input: $input)
/// { … } }` — one record-log entry, all-or-nothing; `INVALID_WRITE`
/// refuses the batch with the reason, `CONFLICT` a lost race.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Mutation", variables = "UpdateCellsVariables")]
pub struct UpdateCells {
    #[arguments(input: $input)]
    pub update_cells: UpdatedCaseFile,
}

/// What `updateCells` answers with: the record after the batch.
#[derive(cynic::QueryFragment, Debug, Clone, PartialEq, Eq)]
#[cynic(graphql_type = "CaseFile")]
pub struct UpdatedCaseFile {
    pub id: cynic::Id,
    pub cells: Vec<crate::preview::Cell>,
    pub items: Vec<crate::preview::ItemList>,
    pub findings: Vec<crate::preview::AdmissibilityFinding>,
}
