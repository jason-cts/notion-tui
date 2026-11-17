use crate::database::{DatabaseData, DatabaseMeta, DatabaseState, supported};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::{HashMap, HashSet};
use tokio::sync::mpsc;

mod comments;
mod database;
pub use comments::{CommentIndicators, CommentsModal};
mod relocation;
pub use relocation::PendingRelocation;

use crate::{
    editor::{Buffer, Editor, Target},
    model::{BlockItem, BlockLabel, InsertPosition, Kind, PageItem, block_labels},
    service::{Service, Write},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Pages,
    Content,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocationKind {
    Page,
    Blocks,
    Database,
}

#[derive(Clone, Debug)]
pub struct Location {
    pub id: String,
    pub title: String,
    pub kind: LocationKind,
    pub page_id: String,
    pub list_depth: usize,
}

pub enum Message {
    CommentPresence {
        id: String,
        request: u64,
        generation: u64,
        retry_after: Option<std::time::Duration>,
        result: Result<bool, String>,
    },
    CommentsLoaded {
        id: String,
        generation: u64,
        result: Result<Vec<crate::comments::Comment>, String>,
    },
    CommentSaved {
        id: String,
        generation: u64,
        deleting: bool,
        result: Result<String, String>,
    },
    DatabaseMeta {
        id: String,
        generation: u64,
        result: Result<DatabaseMeta, String>,
    },
    DatabaseRows {
        id: String,
        generation: u64,
        result: Result<Box<DatabaseData>, String>,
    },
    DatabaseProgress {
        id: String,
        generation: u64,
        count: usize,
    },
    Browser(Result<(), String>),
    Search {
        generation: u64,
        result: Result<Vec<PageItem>, String>,
    },
    Content {
        generation: u64,
        result: Result<(Location, Vec<BlockItem>), String>,
    },
    Saved(Result<Option<PageItem>, String>),
    Deleted(Result<(), String>),
    Relocated {
        pending: Box<PendingRelocation>,
        result: Result<String, String>,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    DeleteComment,
    Discard,
    Quit,
    Delete,
    Relocate,
}

pub struct PendingDelete {
    pub block: BlockItem,
    pub parent: String,
    pub index: usize,
}

pub struct LinkPicker {
    pub links: Vec<(String, String)>,
    pub selected: usize,
}

struct BlockSelection {
    id: String,
    index: usize,
}

pub struct App {
    pub comments: Option<CommentsModal>,
    pub comment_indicators: CommentIndicators,
    comment_generation: u64,
    pub databases: HashMap<String, DatabaseState>,
    metadata_pending: HashSet<String>,
    metadata_generations: HashMap<String, u64>,
    pub pages: Vec<PageItem>,
    pub blocks: Vec<BlockItem>,
    pub page_selected: usize,
    pub block_selected: usize,
    pub focus: Focus,
    pub path: Vec<Location>,
    pub query: String,
    pub search: Option<Buffer>,
    pub editor: Option<Editor>,
    pub confirm: Option<Confirm>,
    pub pending_delete: Option<PendingDelete>,
    pub pending_relocation: Option<PendingRelocation>,
    relocation_recovery: HashMap<String, PendingRelocation>,
    pub help: bool,
    pub help_scroll: u16,
    pub viewer: Option<(BlockLabel, u16)>,
    pub link_picker: Option<LinkPicker>,
    pub pending_link: Option<String>,
    pub status: String,
    pub error: bool,
    pub loading_search: bool,
    pub loading_content: bool,
    pub saving: bool,
    pub quit: bool,
    pub demo: bool,
    search_generation: u64,
    content_generation: u64,
    parent_selections: HashMap<String, BlockSelection>,
    selection_after_back: Option<String>,
    selection_after_insert: Option<(String, InsertPosition)>,
    selection_after_update: Option<(String, String)>,
    selection_after_delete: Option<(String, usize)>,
    service: Service,
    sender: mpsc::UnboundedSender<Message>,
}

impl App {
    pub fn new(service: Service, sender: mpsc::UnboundedSender<Message>, demo: bool) -> Self {
        Self {
            comments: None,
            comment_indicators: CommentIndicators::default(),
            comment_generation: 0,
            service,
            sender,
            demo,
            databases: HashMap::new(),
            metadata_pending: HashSet::new(),
            metadata_generations: HashMap::new(),
            pages: vec![],
            blocks: vec![],
            page_selected: 0,
            block_selected: 0,
            focus: Focus::Pages,
            path: vec![],
            query: String::new(),
            search: None,
            editor: None,
            confirm: None,
            pending_delete: None,
            pending_relocation: None,
            relocation_recovery: HashMap::new(),
            help: false,
            help_scroll: 0,
            viewer: None,
            link_picker: None,
            pending_link: None,
            status: "Search pages shared with your integration".into(),
            error: false,
            loading_search: false,
            loading_content: false,
            saving: false,
            quit: false,
            search_generation: 0,
            content_generation: 0,
            parent_selections: HashMap::new(),
            selection_after_back: None,
            selection_after_insert: None,
            selection_after_update: None,
            selection_after_delete: None,
        }
    }

    fn fail(&mut self, message: impl Into<String>) {
        self.status = message.into();
        self.error = true;
    }
    pub fn location(&self) -> Option<&Location> {
        self.path.last()
    }

    pub fn block_labels(&self) -> Vec<BlockLabel> {
        block_labels(
            &self.blocks,
            self.location().map_or(0, |location| location.list_depth),
        )
    }

    pub fn start_search(&mut self) {
        self.search_generation += 1;
        self.loading_search = true;
        let generation = self.search_generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        let query = self.query.clone();
        tokio::spawn(async move {
            let result = service.search(&query).await.map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::Search { generation, result });
        });
    }

    pub fn load(&mut self) {
        let Some(mut location) = self.location().cloned() else {
            return;
        };
        self.content_generation += 1;
        self.reset_comment_checks();
        if self.selection_after_back.as_deref() != Some(location.id.as_str()) {
            self.selection_after_back = None;
        }
        if location.kind == LocationKind::Database {
            self.blocks.clear();
            self.load_database(false);
            return;
        }
        self.loading_content = true;
        self.blocks.clear();
        self.block_selected = 0;
        let generation = self.content_generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = async {
                if location.kind == LocationKind::Page {
                    location.title = service.page(&location.id).await?.title;
                }
                let blocks = service.children(&location.id).await?;
                Ok::<_, anyhow::Error>((location, blocks))
            }
            .await
            .map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::Content { generation, result });
        });
    }

    fn open_page(&mut self, page: PageItem) {
        self.selection_after_back = None;
        self.path = vec![Location {
            page_id: page.id.clone(),
            id: page.id,
            title: page.title,
            kind: LocationKind::Page,
            list_depth: 0,
        }];
        self.focus = Focus::Content;
        self.load();
    }

    fn save(&mut self, write: Write) {
        self.saving = true;
        self.error = false;
        self.status = "Saving…".into();
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = service.write(write).await.map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::Saved(result));
        });
    }

    fn delete_block(&mut self) {
        let Some(target) = &self.pending_delete else {
            return;
        };
        let original = target.block.clone();
        self.saving = true;
        self.error = false;
        self.status = "Deleting…".into();
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = service
                .write(Write::Delete { original })
                .await
                .map(|_| ())
                .map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::Deleted(result));
        });
    }

    pub fn receive(&mut self, message: Message) {
        match message {
            Message::CommentPresence {
                id,
                request,
                generation,
                retry_after,
                result,
            } => self.receive_comment_presence(id, request, generation, result, retry_after),
            Message::CommentsLoaded {
                id,
                generation,
                result,
            } => self.receive_comments(id, generation, result),
            Message::CommentSaved {
                id,
                generation,
                deleting,
                result,
            } => self.receive_comment_saved(id, generation, deleting, result),
            Message::Relocated { pending, result } => self.receive_relocation(*pending, result),
            Message::DatabaseMeta {
                id,
                generation,
                result,
            } => self.receive_database_meta(id, generation, result),
            Message::DatabaseRows {
                id,
                generation,
                result,
            } => self.receive_database_rows(id, generation, result.map(|data| *data)),
            Message::DatabaseProgress {
                id,
                generation,
                count,
            } => {
                if let Some(db) = self.databases.get_mut(&id)
                    && db.generation == generation
                {
                    db.progress = count;
                }
            }
            Message::Browser(Ok(())) => {
                self.status = "Opened link in your browser".into();
                self.error = false;
            }
            Message::Browser(Err(error)) => self.fail(error),
            Message::Search { generation, result } if generation == self.search_generation => {
                self.loading_search = false;
                match result {
                    Ok(pages) => {
                        self.pages = pages;
                        self.page_selected =
                            self.page_selected.min(self.pages.len().saturating_sub(1));
                        self.status = format!("{} pages · Enter to open", self.pages.len());
                        self.error = false;
                    }
                    Err(error) => self.fail(error),
                }
            }
            Message::Content { generation, result } if generation == self.content_generation => {
                self.loading_content = false;
                match result {
                    Ok((location, blocks)) => {
                        if self.selection_after_back.take().as_deref() == Some(location.id.as_str())
                            && let Some(selection) = self.parent_selections.get(&location.id)
                        {
                            self.block_selected = blocks
                                .iter()
                                .position(|block| block.id == selection.id)
                                .unwrap_or_else(|| {
                                    selection.index.min(blocks.len().saturating_sub(1))
                                });
                        }
                        if let Some((parent, id)) = self.selection_after_update.take()
                            && parent == location.id
                            && let Some(index) = blocks.iter().position(|block| block.id == id)
                        {
                            self.block_selected = index;
                        }
                        if let Some((parent, position)) = self.selection_after_insert.take()
                            && parent == location.id
                        {
                            self.block_selected = match position {
                                InsertPosition::Start => 0,
                                InsertPosition::After(after) => blocks
                                    .iter()
                                    .position(|block| block.id == after)
                                    .map_or(0, |index| {
                                        (index + 1).min(blocks.len().saturating_sub(1))
                                    }),
                                InsertPosition::End => blocks.len().saturating_sub(1),
                            };
                        }
                        if let Some((parent, index)) = self.selection_after_delete.take()
                            && parent == location.id
                        {
                            self.block_selected = index.min(blocks.len().saturating_sub(1));
                        }
                        if let Some(last) = self.path.last_mut() {
                            *last = location;
                        }
                        self.blocks = blocks;
                        self.ensure_database_metadata(false);
                        self.error = false;
                        self.status = format!(
                            "{} blocks · e edit · d delete · a append · n child page",
                            self.blocks.len()
                        );
                    }
                    Err(error) => self.fail(error),
                }
            }
            Message::Deleted(result) => {
                let Some(target) = self.pending_delete.take() else {
                    return;
                };
                self.saving = false;
                match result {
                    Ok(()) => {
                        self.databases.remove(&target.block.id);
                        self.metadata_pending.remove(&target.block.id);
                        *self
                            .metadata_generations
                            .entry(target.block.id)
                            .or_default() += 1;
                        self.error = false;
                        self.status = "Deleted · moved to Notion Trash".into();
                        if self
                            .location()
                            .is_some_and(|location| location.id == target.parent)
                        {
                            self.selection_after_delete = Some((target.parent, target.index));
                            self.load();
                        }
                        self.start_search();
                    }
                    Err(error) => self.fail(error),
                }
            }
            Message::Saved(result) => {
                self.saving = false;
                match result {
                    Ok(created) => {
                        let renamed = self.editor.as_ref().is_some_and(
                            |e| matches!(&e.target, Target::DatabaseCell(t) if t.kind == "title"),
                        );
                        self.selection_after_update = self.editor.as_ref().and_then(|editor| {
                            if let Target::Edit(block) = &editor.target {
                                self.location()
                                    .map(|location| (location.id.clone(), block.id.clone()))
                            } else {
                                None
                            }
                        });
                        self.selection_after_insert = self.editor.as_ref().and_then(|editor| {
                            if let Target::Append { parent, position } = &editor.target {
                                Some((parent.clone(), position.clone()))
                            } else {
                                None
                            }
                        });
                        self.editor = None;
                        self.confirm = None;
                        self.error = false;
                        self.status = "Saved".into();
                        if let Some(page) = created {
                            self.open_page(page);
                            self.start_search();
                        } else {
                            self.load();
                            if renamed {
                                self.start_search();
                            }
                        }
                    }
                    Err(error) => {
                        if let Some(editor) = &mut self.editor {
                            editor.error = Some(error.clone());
                        }
                        self.fail(error);
                    }
                }
            }
            _ => {}
        }
    }

    pub fn paste(&mut self, text: &str) {
        if self.saving || self.confirm.is_some() {
            return;
        }
        if let Some(editor) = &mut self.editor {
            editor.insert(text);
        } else if let Some(search) = &mut self.search {
            search.insert(&text.replace(['\n', '\r'], " "));
        }
    }

    pub fn key(&mut self, mut key: KeyEvent) {
        // Terminals can send BS (Ctrl-H) or DEL for the Backspace key.
        if key.code == KeyCode::Char('h') && key.modifiers == KeyModifiers::CONTROL {
            key.code = KeyCode::Backspace;
            key.modifiers = KeyModifiers::NONE;
        }
        if self.saving {
            return;
        }
        if let Some(editor) = &mut self.editor
            && editor.style_picker.is_some()
            && self.confirm.is_none()
        {
            editor.key(key);
            return;
        }
        if let Some(confirm) = self.confirm {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm = None;
                    match confirm {
                        Confirm::Discard => self.editor = None,
                        Confirm::Quit => self.quit = true,
                        Confirm::Delete => self.delete_block(),
                        Confirm::Relocate => self.relocate_block(),
                        Confirm::DeleteComment => self.delete_comment(),
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm = None;
                    if confirm == Confirm::Delete {
                        self.pending_delete = None;
                    }
                    if confirm == Confirm::Relocate {
                        self.pending_relocation = None;
                    }
                }
                _ => {}
            }
            return;
        }
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if self.editor.is_some() {
                self.confirm = Some(Confirm::Quit);
            } else {
                self.quit = true;
            }
            return;
        }
        if let Some(picker) = &mut self.link_picker {
            match key.code {
                KeyCode::Down => picker.selected = (picker.selected + 1) % picker.links.len(),
                KeyCode::Up => {
                    picker.selected =
                        (picker.selected + picker.links.len() - 1) % picker.links.len()
                }
                KeyCode::Enter => {
                    self.pending_link = Some(picker.links[picker.selected].1.clone());
                    self.link_picker = None;
                }
                KeyCode::Esc => self.link_picker = None,
                _ => {}
            }
            return;
        }
        if self.viewer.is_some() && key.code == KeyCode::Char('o') {
            self.open_links();
            return;
        }
        if self.help || self.viewer.is_some() {
            let scroll = if self.help {
                &mut self.help_scroll
            } else {
                &mut self.viewer.as_mut().unwrap().1
            };
            match key.code {
                KeyCode::Down => *scroll = scroll.saturating_add(1),
                KeyCode::Up => *scroll = scroll.saturating_sub(1),
                KeyCode::PageDown => *scroll = scroll.saturating_add(10),
                KeyCode::PageUp => *scroll = scroll.saturating_sub(10),
                KeyCode::Home | KeyCode::Char('g') => *scroll = 0,
                _ => {
                    self.help = false;
                    self.viewer = None;
                }
            }
            return;
        }
        if self.editor.is_some() {
            match key.code {
                KeyCode::Esc => self.confirm = Some(Confirm::Discard),
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.save_editor()
                }
                _ => self.editor.as_mut().unwrap().key(key),
            }
            return;
        }
        if let Some(search) = &mut self.search {
            match key.code {
                KeyCode::Esc => self.search = None,
                KeyCode::Enter => {
                    self.query = search.text.clone();
                    self.search = None;
                    self.focus = Focus::Pages;
                    self.page_selected = 0;
                    self.start_search();
                }
                _ => search.key(key, false),
            }
            return;
        }
        if self.comments_key(key) {
            return;
        }
        if self.database_key(key) {
            return;
        }
        match key.code {
            KeyCode::Char('c') if key.modifiers.is_empty() => self.open_comments(),
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('?') => {
                self.help = true;
                self.help_scroll = 0;
            }
            KeyCode::Char('v') if self.focus == Focus::Content => {
                if let Some(label) = self.block_labels().get(self.block_selected) {
                    self.viewer = Some((label.clone(), 0));
                }
            }
            KeyCode::Char('o') if self.focus == Focus::Content => self.open_links(),
            KeyCode::Tab | KeyCode::BackTab => {
                self.focus = match self.focus {
                    Focus::Pages => Focus::Content,
                    Focus::Content => Focus::Pages,
                }
            }
            KeyCode::Char('/') => self.search = Some(Buffer::new(self.query.clone())),
            KeyCode::Char('r') => {
                if self.focus == Focus::Pages {
                    self.start_search();
                } else {
                    self.invalidate_comment_indicators();
                    self.load();
                }
            }
            KeyCode::Down => self.move_selection(true),
            KeyCode::Up => self.move_selection(false),
            KeyCode::Home | KeyCode::Char('g') => {
                if self.focus == Focus::Pages {
                    self.page_selected = 0;
                } else {
                    self.block_selected = 0;
                }
            }
            KeyCode::End | KeyCode::Char('G') => {
                if self.focus == Focus::Pages {
                    self.page_selected = self.pages.len().saturating_sub(1);
                } else {
                    self.block_selected = self.blocks.len().saturating_sub(1);
                }
            }
            KeyCode::Enter => self.enter(),
            KeyCode::Backspace => {
                if self.focus == Focus::Content && self.path.len() > 1 {
                    self.path.pop();
                    self.selection_after_back = self
                        .location()
                        .filter(|location| location.kind != LocationKind::Database)
                        .map(|location| location.id.clone());
                    self.load();
                } else {
                    self.focus = Focus::Pages;
                }
            }
            KeyCode::Char('d') if self.focus == Focus::Content => {
                if !self.loading_content
                    && let Some(location) = self.location()
                    && location.kind != LocationKind::Database
                    && let Some(block) = self.blocks.get(self.block_selected)
                    && !block.id.is_empty()
                {
                    self.pending_delete = Some(PendingDelete {
                        block: block.clone(),
                        parent: location.id.clone(),
                        index: self.block_selected,
                    });
                    self.confirm = Some(Confirm::Delete);
                }
            }
            KeyCode::Char('>' | '<') if self.focus == Focus::Content => {
                self.prepare_relocation(key.code == KeyCode::Char('>'));
            }
            KeyCode::Char('e') if self.focus == Focus::Content => {
                if let Some(block) = self.blocks.get(self.block_selected).cloned() {
                    if block.protected {
                        self.fail("Mentions and equations are read-only");
                    } else if (block.kind.is_none() && !block.editable_table_row())
                        || block.id.is_empty()
                    {
                        self.fail("This block type is read-only");
                    } else {
                        self.editor = Some(Editor::new(Target::Edit(block)));
                    }
                }
            }
            KeyCode::Char('a' | 'A') if self.focus == Focus::Content => {
                if self.loading_content {
                    return;
                }
                if let Some(location) = self.location() {
                    let selected = self.blocks.get(self.block_selected);
                    if !self.blocks.is_empty() && selected.is_none_or(|block| block.id.is_empty()) {
                        self.fail("Select a block before inserting");
                        return;
                    }
                    let position = if self.blocks.is_empty() {
                        InsertPosition::End
                    } else if key.code == KeyCode::Char('A') {
                        match self.block_selected.checked_sub(1) {
                            None => InsertPosition::Start,
                            Some(index) => {
                                let previous = &self.blocks[index];
                                if previous.id.is_empty() {
                                    self.fail("Select a block before inserting");
                                    return;
                                }
                                InsertPosition::After(previous.id.clone())
                            }
                        }
                    } else {
                        InsertPosition::After(selected.unwrap().id.clone())
                    };
                    self.editor = Some(Editor::new(Target::Append {
                        parent: location.id.clone(),
                        position,
                    }));
                }
            }
            KeyCode::Char('n') if self.focus == Focus::Content => {
                if let Some(location) = self.location() {
                    self.editor = Some(Editor::new(Target::Create(location.page_id.clone())));
                }
            }
            KeyCode::Char(' ') if self.focus == Focus::Content => {
                if let Some(original) = self
                    .blocks
                    .get(self.block_selected)
                    .cloned()
                    .filter(|b| b.kind == Some(Kind::Todo))
                {
                    self.save(Write::Update {
                        original,
                        text: None,
                    });
                }
            }
            _ => {}
        }
        self.ensure_database_metadata(false);
    }

    fn open_links(&mut self) {
        let links = if let Some((label, _)) = &self.viewer {
            crate::rich_text::external_links(&label.rich_text)
        } else {
            self.blocks
                .get(self.block_selected)
                .map(|block| block.external_links())
                .unwrap_or_default()
        };
        match links.len() {
            0 => self.fail("No HTTP or HTTPS links in this block"),
            1 => self.pending_link = Some(links[0].1.clone()),
            _ => self.link_picker = Some(LinkPicker { links, selected: 0 }),
        }
    }

    fn move_selection(&mut self, down: bool) {
        let (selected, len) = match self.focus {
            Focus::Pages => (&mut self.page_selected, self.pages.len()),
            Focus::Content => (&mut self.block_selected, self.blocks.len()),
        };
        *selected = if down {
            selected.saturating_add(1).min(len.saturating_sub(1))
        } else {
            selected.saturating_sub(1)
        };
    }

    fn enter(&mut self) {
        if self.focus == Focus::Pages {
            if !self.loading_search
                && let Some(page) = self.pages.get(self.page_selected).cloned()
            {
                self.open_page(page);
            }
        } else if !self.loading_content
            && let Some(block) = self.blocks.get(self.block_selected).cloned()
        {
            if block.block_type != "child_database"
                && !block.child_page
                && (!block.children || block.id.is_empty())
            {
                return;
            }
            if let Some(location) = self.location() {
                self.parent_selections.insert(
                    location.id.clone(),
                    BlockSelection {
                        id: block.id.clone(),
                        index: self.block_selected,
                    },
                );
            }
            if block.block_type == "child_database" {
                self.path.push(Location {
                    id: block.id,
                    title: block.text,
                    kind: LocationKind::Database,
                    page_id: self
                        .location()
                        .map(|l| l.page_id.clone())
                        .unwrap_or_default(),
                    list_depth: 0,
                });
                self.load();
            } else if block.child_page {
                let page_id = block.id.clone();
                self.path.push(Location {
                    id: block.id,
                    title: block.text,
                    kind: LocationKind::Page,
                    page_id,
                    list_depth: 0,
                });
                self.load();
            } else if block.children && !block.id.is_empty() {
                let page_id = self.location().unwrap().page_id.clone();
                let list_depth = self.location().unwrap().list_depth
                    + usize::from(matches!(
                        block.kind,
                        Some(Kind::Number | Kind::Bullet | Kind::Todo)
                    ));
                self.path.push(Location {
                    id: block.id,
                    title: block.text,
                    kind: LocationKind::Blocks,
                    page_id,
                    list_depth,
                });
                self.load();
            }
        }
    }

    fn save_editor(&mut self) {
        let Some(editor) = &self.editor else {
            return;
        };
        if !editor.changed() {
            self.editor = None;
            self.status = "No changes".into();
            self.error = false;
            return;
        }
        if matches!(editor.target, Target::Comment(_)) {
            match editor.comment_write() {
                Ok(write) => self.save_comment(write),
                Err(error) => {
                    self.editor.as_mut().unwrap().error = Some(error.into());
                    self.fail(error);
                }
            }
            return;
        }
        match editor.write() {
            Ok(write) => self.save(write),
            Err(error) => {
                self.editor.as_mut().unwrap().error = Some(error.into());
                self.fail(error);
            }
        }
    }
}

#[cfg(test)]
mod tests;
