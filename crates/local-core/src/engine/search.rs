//! Index-backed queries: object lists, full-text search, and the calendar.

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
        if let Some(date) = &query.due_on_or_before
            && date.parse::<jiff::civil::Date>().is_err()
        {
            return Err(CoreError::validation(
                "invalid_date",
                "dueOnOrBefore must be a YYYY-MM-DD date",
                "object_summaries",
            ));
        }
        self.index
            .lock()
            .map_err(|_| lock_error("object_summaries"))?
            .query_object_summaries(query)
    }

    pub fn search(&self, input: &SearchInput) -> Result<Vec<SearchResult>> {
        self.index
            .lock()
            .map_err(|_| lock_error("search_query"))?
            .search(input)
    }

    pub fn calendar(&self, start: &str, end: &str) -> Result<Vec<CalendarEntry>> {
        self.index
            .lock()
            .map_err(|_| lock_error("calendar_query"))?
            .calendar(start, end)
    }
}
