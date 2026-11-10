use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use crate::{
    editor::{Buffer, Editor, Target},
    model::{BlockItem, BlockLabel, Kind, PageItem, block_labels},
    service::{Service, Write},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Focus {
    Pages,
    Content,
}

#[derive(Clone, Debug)]
pub struct Location {
    pub id: String,
    pub title: String,
    pub is_page: bool,
    pub page_id: String,
    pub list_depth: usize,
}

pub enum Message {
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Formatting,
    Discard,
    Quit,
}

pub struct LinkPicker {
    pub links: Vec<(String, String)>,
    pub selected: usize,
}

pub struct App {
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
    selection_after_insert: Option<(String, String)>,
    service: Service,
    sender: mpsc::UnboundedSender<Message>,
}

impl App {
    pub fn new(service: Service, sender: mpsc::UnboundedSender<Message>, demo: bool) -> Self {
        Self {
            service,
            sender,
            demo,
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
            selection_after_insert: None,
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
        self.loading_content = true;
        self.blocks.clear();
        self.block_selected = 0;
        let generation = self.content_generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = async {
                if location.is_page {
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
        self.path = vec![Location {
            page_id: page.id.clone(),
            id: page.id,
            title: page.title,
            is_page: true,
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

    pub fn receive(&mut self, message: Message) {
        match message {
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
                        if let Some((parent, after)) = self.selection_after_insert.take()
                            && parent == location.id
                        {
                            self.block_selected = blocks
                                .iter()
                                .position(|block| block.id == after)
                                .map_or(0, |index| (index + 1).min(blocks.len().saturating_sub(1)));
                        }
                        if let Some(last) = self.path.last_mut() {
                            *last = location;
                        }
                        self.blocks = blocks;
                        self.error = false;
                        self.status = format!(
                            "{} blocks · e edit · a append · n child page",
                            self.blocks.len()
                        );
                    }
                    Err(error) => self.fail(error),
                }
            }
            Message::Saved(result) => {
                self.saving = false;
                match result {
                    Ok(created) => {
                        self.selection_after_insert = self.editor.as_ref().and_then(|editor| {
                            if let Target::Append {
                                parent,
                                after: Some(after),
                            } = &editor.target
                            {
                                Some((parent.clone(), after.clone()))
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
            if matches!(editor.target, Target::Create(_)) {
                editor.buffer.insert(&text.replace(['\n', '\r'], " "));
            } else {
                editor.buffer.insert(text);
            }
        } else if let Some(search) = &mut self.search {
            search.insert(&text.replace(['\n', '\r'], " "));
        }
    }

    pub fn key(&mut self, key: KeyEvent) {
        if self.saving {
            return;
        }
        if let Some(confirm) = self.confirm {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.confirm = None;
                    match confirm {
                        Confirm::Formatting => {
                            if let Some(e) = &mut self.editor {
                                e.confirmed = true;
                            }
                            self.save_editor();
                        }
                        Confirm::Discard => self.editor = None,
                        Confirm::Quit => self.quit = true,
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => self.confirm = None,
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
                KeyCode::Down | KeyCode::Char('j') => {
                    picker.selected = (picker.selected + 1) % picker.links.len()
                }
                KeyCode::Up | KeyCode::Char('k') => {
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
                KeyCode::Down | KeyCode::Char('j') => *scroll = scroll.saturating_add(1),
                KeyCode::Up | KeyCode::Char('k') => *scroll = scroll.saturating_sub(1),
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
                KeyCode::Tab => self.editor.as_mut().unwrap().cycle_kind(),
                _ => {
                    let editor = self.editor.as_mut().unwrap();
                    let multiline = !matches!(editor.target, Target::Create(_));
                    editor.buffer.key(key, multiline);
                }
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
        match key.code {
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
                    self.load();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_selection(true),
            KeyCode::Up | KeyCode::Char('k') => self.move_selection(false),
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
            KeyCode::Backspace | KeyCode::Left => {
                if self.focus == Focus::Content && self.path.len() > 1 {
                    self.path.pop();
                    self.load();
                } else {
                    self.focus = Focus::Pages;
                }
            }
            KeyCode::Char('e') if self.focus == Focus::Content => {
                if let Some(block) = self.blocks.get(self.block_selected).cloned() {
                    if block.protected {
                        self.fail("Mentions and equations are read-only");
                    } else if block.kind.is_none() || block.id.is_empty() {
                        self.fail("This block type is read-only");
                    } else {
                        self.editor = Some(Editor::new(Target::Edit(block)));
                    }
                }
            }
            KeyCode::Char('a') if self.focus == Focus::Content => {
                if self.loading_content {
                    return;
                }
                if let Some(location) = self.location() {
                    let after = self
                        .blocks
                        .get(self.block_selected)
                        .map(|block| block.id.clone());
                    if (!self.blocks.is_empty() && after.is_none()) || after.as_deref() == Some("")
                    {
                        self.fail("Select a block before inserting");
                        return;
                    }
                    self.editor = Some(Editor::new(Target::Append {
                        parent: location.id.clone(),
                        after,
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
        } else if let Some(block) = self.blocks.get(self.block_selected).cloned() {
            if block.child_page {
                let page_id = block.id.clone();
                self.path.push(Location {
                    id: block.id,
                    title: block.text,
                    is_page: true,
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
                    is_page: false,
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
        if let Target::Edit(block) = &editor.target
            && block.styled
            && !editor.confirmed
            && editor.buffer.text != block.text
        {
            self.confirm = Some(Confirm::Formatting);
            return;
        }
        if let Target::Edit(block) = &editor.target
            && editor.buffer.text == block.text
        {
            self.editor = None;
            self.status = "No changes".into();
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
