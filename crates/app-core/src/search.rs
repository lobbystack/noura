//! The search service: full-text search over `local-core`'s index. What
//! you type is plain text, never query syntax. [`Searcher`] runs searches
//! and lists the index's types off the UI thread.

use std::sync::{
    Arc,
    mpsc::{Receiver, Sender, TryRecvError, channel},
};

use local_core::{CoreError, SearchInput, SearchResult};

use crate::{
    AppError, EntryKind, Workspace,
    files::{parent_of, shown_name},
};

/// At most this many hits come back for one query.
pub const SEARCH_LIMIT: u32 = 50;

/// What to search for.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SearchQuery {
    /// The words to find. Every word must match, a partial word matches
    /// the start of a word, and accents don't matter.
    pub text: String,
    /// Only objects of this type, one of [`Workspace::search_types`].
    pub object_type: Option<String>,
    /// Only files in this workspace folder and its subfolders, written as
    /// the file tree shows it, such as `Notes/Ideas`. Empty for the whole
    /// workspace.
    pub folder: String,
    /// Only notes whose `tags` property holds this tag, as a list or a
    /// single value. A leading `#` and the case don't matter. Empty for
    /// any note.
    pub tag: String,
}

/// One file that matches a search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub relative_path: String,
    /// The file's title: its name, without `.md` for Markdown files.
    pub title: String,
    /// The managed object's type, such as `note` or `task`.
    pub object_type: Option<String>,
    /// The folder that holds the file, or `""` at the top of the workspace.
    pub folder: String,
    /// The text around the match in the file's body.
    pub snippet: String,
}

impl Workspace {
    /// The files that match `query`, best match first: a match in the
    /// title comes before a match in the body alone.
    pub fn search(&self, query: &SearchQuery) -> Result<Vec<SearchHit>, AppError> {
        self.search_page(query, 0, SEARCH_LIMIT as usize)
    }

    /// The files [`Workspace::search`] finds, skipping the first `offset`,
    /// at most `limit` of them.
    pub fn search_page(
        &self,
        query: &SearchQuery,
        offset: usize,
        limit: usize,
    ) -> Result<Vec<SearchHit>, AppError> {
        let input = search_input(query, limit);
        let offset = u32::try_from(offset).unwrap_or(u32::MAX);
        let results = self
            .engine()
            .search_page(&input, offset)
            .map_err(search_error)?;
        Ok(results.into_iter().map(hit).collect())
    }

    /// How many files match `query` in all.
    pub fn search_count(&self, query: &SearchQuery) -> Result<usize, AppError> {
        let count = self
            .engine()
            .search_count(&search_input(query, 0))
            .map_err(search_error)?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }

    /// The object types the index holds, sorted, for the type filter.
    pub fn search_types(&self) -> Result<Vec<String>, AppError> {
        self.engine().object_types().map_err(search_error)
    }
}

fn search_input(query: &SearchQuery, limit: usize) -> SearchInput {
    SearchInput {
        query: query.text.clone(),
        object_type: query.object_type.clone(),
        folder: Some(query.folder.clone()),
        tag: Some(query.tag.clone()),
        limit: Some(u32::try_from(limit).unwrap_or(u32::MAX)),
    }
}

/// How many hits one page of search's contents holds.
pub const SEARCH_PAGE: usize = 50;

/// What a search shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchState {
    /// Nothing typed yet.
    Idle,
    /// The search runs in the background.
    Searching,
    /// The hits, best first. None means no file matches.
    Done(Vec<SearchHit>),
    /// The search couldn't run: the folder filter is outside the
    /// workspace, or the index failed.
    Failed(AppError),
}

/// What the search thread works on.
enum Job {
    /// List the object types the index holds, for the type filter.
    Types,
    /// Find a page of the files that match a query, skipping the first
    /// `offset`. The first page's search counts every match afterwards.
    Search { query: SearchQuery, offset: usize },
    /// Count every file that matches a query.
    Count(SearchQuery),
}

/// A job to run, numbered so its answer can be matched to it.
struct Request {
    number: u64,
    workspace: Arc<Workspace>,
    job: Job,
}

impl Request {
    /// Run the job. The workspace is let go before the answer goes back,
    /// so closing it never waits on the search thread.
    fn run(self) -> Answer {
        let Self {
            number,
            workspace,
            job,
        } = self;
        match job {
            Job::Types => Answer::Types {
                number,
                result: workspace.search_types(),
            },
            Job::Search { query, offset } => Answer::Hits {
                number,
                offset,
                result: workspace.search_page(&query, offset, SEARCH_PAGE),
            },
            Job::Count(query) => Answer::Count {
                number,
                result: workspace.search_count(&query),
            },
        }
    }

    /// The count of every match that follows this request's first page.
    fn count(&self) -> Option<Self> {
        match &self.job {
            Job::Search { query, offset: 0 } => Some(Self {
                number: self.number,
                workspace: self.workspace.clone(),
                job: Job::Count(query.clone()),
            }),
            _ => None,
        }
    }

    /// The answer when the search thread couldn't start.
    fn unavailable(&self) -> Answer {
        let result = AppError::new("search_unavailable", "Search isn't available right now.");
        match self.job {
            Job::Types => Answer::Types {
                number: self.number,
                result: Err(result),
            },
            Job::Search { offset, .. } => Answer::Hits {
                number: self.number,
                offset,
                result: Err(result),
            },
            Job::Count(_) => Answer::Count {
                number: self.number,
                result: Err(result),
            },
        }
    }
}

enum Answer {
    Types {
        number: u64,
        result: Result<Vec<String>, AppError>,
    },
    Hits {
        number: u64,
        offset: usize,
        result: Result<Vec<SearchHit>, AppError>,
    },
    Count {
        number: u64,
        result: Result<usize, AppError>,
    },
}

/// The answers [`Searcher::take`] found for the newest jobs.
#[derive(Default)]
pub(crate) struct Answers {
    /// The object types the index holds.
    pub(crate) types: Option<Result<Vec<String>, AppError>>,
    /// The newest search's page of hits, and how many hits come before
    /// it.
    pub(crate) hits: Option<(usize, Result<Vec<SearchHit>, AppError>)>,
    /// How many files match the newest search in all.
    pub(crate) count: Option<Result<usize, AppError>>,
}

/// Runs searches, and lists the types the index holds, on a background
/// thread, so the UI never waits on the index. A search asked for while
/// another runs replaces any search still waiting, so the thread works on
/// the newest query, and only the newest search's answer is taken.
pub(crate) struct Searcher {
    requests: Option<Sender<Request>>,
    answers: Receiver<Answer>,
    answer_sender: Sender<Answer>,
    /// Called on the background thread once an answer arrives.
    answered: Arc<dyn Fn() + Send + Sync>,
    /// The number the next job gets. Numbers start at 1.
    next: u64,
    /// The number of the search whose answer is wanted, or 0 for none.
    wanted_hits: u64,
    /// The number of the search whose count is wanted, or 0 for none.
    wanted_count: u64,
    /// The number of the type list that is wanted, or 0 for none.
    wanted_types: u64,
}

impl Searcher {
    /// A searcher that calls `answered` from its thread after each answer.
    pub(crate) fn new(answered: impl Fn() + Send + Sync + 'static) -> Self {
        let (answer_sender, answers) = channel();
        Self {
            requests: None,
            answers,
            answer_sender,
            answered: Arc::new(answered),
            next: 1,
            wanted_hits: 0,
            wanted_count: 0,
            wanted_types: 0,
        }
    }

    /// Forget the searches and type lists asked for so far: their answers
    /// are dropped.
    pub(crate) fn cancel(&mut self) {
        self.wanted_hits = 0;
        self.wanted_count = 0;
        self.wanted_types = 0;
    }

    /// Forget the searches asked for so far, but not the type list.
    pub(crate) fn cancel_search(&mut self) {
        self.wanted_hits = 0;
        self.wanted_count = 0;
    }

    /// List the object types `workspace`'s index holds in the background.
    /// [`Searcher::take`] returns them.
    pub(crate) fn list_types(&mut self, workspace: Arc<Workspace>) {
        self.wanted_types = self.send(workspace, Job::Types);
    }

    /// Search `workspace` for `query` in the background, in place of any
    /// search asked for before. [`Searcher::take`] returns the answer.
    pub(crate) fn start(&mut self, workspace: Arc<Workspace>, query: SearchQuery) {
        let offset = 0;
        self.wanted_hits = self.send(workspace, Job::Search { query, offset });
        self.wanted_count = self.wanted_hits;
    }

    /// Search `workspace` for the page of `query`'s hits after the first
    /// `offset`, as you reach the end of the ones that show. The count
    /// the search started with stays wanted.
    pub(crate) fn more(&mut self, workspace: Arc<Workspace>, query: SearchQuery, offset: usize) {
        self.wanted_hits = self.send(workspace, Job::Search { query, offset });
    }

    /// Hand `job` to the search thread, starting it if needed, and return
    /// the job's number.
    fn send(&mut self, workspace: Arc<Workspace>, job: Job) -> u64 {
        let number = self.next;
        self.next += 1;
        let mut request = Request {
            number,
            workspace,
            job,
        };
        if let Some(requests) = &self.requests {
            match requests.send(request) {
                Ok(()) => return number,
                // The thread stopped; start another.
                Err(returned) => request = returned.0,
            }
        }
        let (sender, receiver) = channel();
        let answers = self.answer_sender.clone();
        let answered = self.answered.clone();
        let spawned = std::thread::Builder::new()
            .name("search".into())
            .spawn(move || serve(&receiver, &answers, answered.as_ref()));
        match spawned {
            Ok(_) => {
                // The thread is waiting for this request, so the send
                // can't fail.
                let _ = sender.send(request);
                self.requests = Some(sender);
            }
            Err(error) => {
                tracing::error!(%error, "could not start the search thread");
                let _ = self.answer_sender.send(request.unavailable());
            }
        }
        number
    }

    /// The answers to the newest search and type list, once they arrive.
    /// Answers to older ones are dropped.
    pub(crate) fn take(&mut self) -> Answers {
        let mut newest = Answers::default();
        while let Ok(answer) = self.answers.try_recv() {
            match answer {
                Answer::Types { number, result } if number == self.wanted_types => {
                    newest.types = Some(result);
                }
                Answer::Hits {
                    number,
                    offset,
                    result,
                } if number == self.wanted_hits => {
                    newest.hits = Some((offset, result));
                }
                Answer::Count { number, result } if number == self.wanted_count => {
                    newest.count = Some(result);
                }
                _ => {}
            }
        }
        newest
    }
}

/// The search thread: answer the newest waiting type list, then the
/// newest waiting search, until the searcher goes away. Once a search's
/// first page is on its way and nothing waits, count every match, so the
/// first page never waits for the count. A new search drops the count
/// still to come; a later page of the same search doesn't.
fn serve(requests: &Receiver<Request>, answers: &Sender<Answer>, answered: &dyn Fn()) {
    let mut count: Option<Request> = None;
    loop {
        let first = match count.take() {
            Some(counting) => match requests.try_recv() {
                Ok(request) => {
                    count = Some(counting);
                    request
                }
                Err(TryRecvError::Empty) => {
                    if answers.send(counting.run()).is_err() {
                        return;
                    }
                    answered();
                    continue;
                }
                Err(TryRecvError::Disconnected) => return,
            },
            None => match requests.recv() {
                Ok(request) => request,
                Err(_) => return,
            },
        };
        let mut types = None;
        let mut search = None;
        let waiting = std::iter::once(first).chain(std::iter::from_fn(|| requests.try_recv().ok()));
        for request in waiting {
            match request.job {
                Job::Types => types = Some(request),
                Job::Search { offset: 0, .. } => {
                    count = None;
                    search = Some(request);
                }
                Job::Search { .. } | Job::Count(_) => search = Some(request),
            }
        }
        if let Some(request) = &search
            && let Some(counting) = request.count()
        {
            count = Some(counting);
        }
        for request in types.into_iter().chain(search) {
            if answers.send(request.run()).is_err() {
                return;
            }
            answered();
        }
    }
}

fn hit(result: SearchResult) -> SearchHit {
    let file_name = result
        .relative_path
        .rsplit('/')
        .next()
        .unwrap_or(&result.relative_path);
    // The index's title: the file's name, or a chat's own title for the
    // chat and its messages.
    let title = if result.title.trim().is_empty() {
        shown_name(file_name, EntryKind::File)
    } else {
        result.title
    };
    SearchHit {
        title,
        folder: shown_folder(&result.relative_path, result.object_type.as_deref()).to_owned(),
        object_type: result.object_type,
        snippet: result.snippet,
        relative_path: result.relative_path,
    }
}

/// The folder a hit shows: the one that holds the file, or for a chat and
/// its messages the one that holds the chat's own folder, whose name
/// repeats the chat's title.
fn shown_folder<'a>(relative_path: &'a str, object_type: Option<&str>) -> &'a str {
    let folder = parent_of(relative_path).unwrap_or("");
    let chat_folder = match object_type {
        Some("chat") => Some(folder),
        // Messages sit in the chat's folder under `messages/<date>/`.
        Some("chat-message") => parent_of(folder)
            .and_then(|messages| messages.strip_suffix("/messages"))
            .filter(|chat| !chat.is_empty()),
        _ => None,
    };
    match chat_folder {
        Some(chat) => parent_of(chat).unwrap_or(""),
        None => folder,
    }
}

/// A folder filter Noura refuses, or an index that couldn't answer. The
/// details go to the log.
fn search_error(error: CoreError) -> AppError {
    tracing::warn!(
        code = %error.code,
        operation = %error.operation,
        details = ?error.details,
        "a search failed"
    );
    match error.code.as_str() {
        "absolute_path" | "unsafe_path" | "reserved_path" => AppError::new(
            &error.code,
            "Enter a folder inside this workspace as the file tree shows it, such as Notes \
             or Notes/Ideas.",
        ),
        _ => AppError::new("search_unavailable", "Search isn't available right now."),
    }
}
