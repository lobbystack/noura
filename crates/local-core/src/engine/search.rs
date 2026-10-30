//! Index-backed queries: object lists and full-text search.

use super::*;

impl WorkspaceEngine {
    pub fn query_objects(&self, object_type: Option<&str>) -> Result<Vec<WorkspaceObject>> {
        self.index
            .lock()
            .map_err(|_| lock_error("object_query"))?
            .query_objects(object_type)
    }

    /// Objects matching exact-value filters, filtered in SQL.
    pub fn query_objects_filtered(
        &self,
        filter: &crate::ObjectFilter,
    ) -> Result<Vec<WorkspaceObject>> {
        self.index
            .lock()
            .map_err(|_| lock_error("object_query"))?
            .query_objects_filtered(filter)
    }

    /// Body-free, bounded object summaries for overview screens.
    pub fn query_object_summaries(
        &self,
        query: &crate::ObjectSummaryQuery,
    ) -> Result<Vec<crate::ObjectSummary>> {
        if let Some(bound) = &query.date_on_or_before
            && !bound
                .value
                .as_str()
                .is_some_and(workspace_format::is_civil_date)
        {
            return Err(CoreError::validation(
                "invalid_date",
                "dateOnOrBefore must hold a YYYY-MM-DD date",
                "object_summaries",
            ));
        }
        self.index
            .lock()
            .map_err(|_| lock_error("object_summaries"))?
            .query_object_summaries(query)
    }

    /// Every object of `object_type` the index holds, without bodies, in
    /// the order of their paths, for a check over a whole type such as a
    /// plugin's schema. Files that share an ID are left out.
    pub fn object_summaries_of_type(&self, object_type: &str) -> Result<Vec<crate::ObjectSummary>> {
        self.index
            .lock()
            .map_err(|_| lock_error("object_summaries"))?
            .object_summaries_of_type(object_type)
    }

    pub fn search(&self, input: &SearchInput) -> Result<Vec<SearchResult>> {
        self.index
            .lock()
            .map_err(|_| lock_error("search_query"))?
            .search(input)
    }

    /// The object types the index holds, sorted, for a search's type
    /// filter.
    pub fn object_types(&self) -> Result<Vec<String>> {
        self.index
            .lock()
            .map_err(|_| lock_error("object_types"))?
            .object_types()
    }

    /// The paths of the `limit` notes whose files changed last, newest
    /// first. Notes are Markdown files other than managed objects of
    /// another type, such as tasks.
    pub fn recent_notes(&self, limit: usize) -> Result<Vec<String>> {
        self.index
            .lock()
            .map_err(|_| lock_error("recent_notes"))?
            .recent_notes(limit)
    }
}
