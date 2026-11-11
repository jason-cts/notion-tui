use super::*;
use crate::rich_text::external_links;

impl App {
    pub fn database(&self) -> Option<&DatabaseState> {
        let location = self.location()?;
        (location.kind == LocationKind::Database)
            .then(|| self.databases.get(&location.id))
            .flatten()
    }
    pub fn content_type(&self, block: &BlockItem) -> String {
        if block.block_type != "child_database" {
            return block.block_type.clone();
        }
        let layout = self
            .databases
            .get(&block.id)
            .map(|db| {
                db.view()
                    .map(|v| v.view_type.as_str())
                    .unwrap_or(if db.error.is_some() {
                        "unavailable"
                    } else {
                        "…"
                    })
            })
            .unwrap_or("…");
        format!("child_database ({layout})")
    }
    pub(super) fn ensure_database_metadata(&mut self, force: bool) {
        let id =
            if let Some(location) = self.location().filter(|l| l.kind == LocationKind::Database) {
                Some(location.id.clone())
            } else {
                self.blocks
                    .get(self.block_selected)
                    .filter(|b| b.block_type == "child_database")
                    .map(|b| b.id.clone())
            };
        let Some(id) = id else { return };
        let db = self.databases.entry(id.clone()).or_default();
        if !force
            && (db.meta.is_some() || db.error.is_some() || self.metadata_pending.contains(&id))
        {
            return;
        }
        db.error = None;
        self.metadata_pending.insert(id.clone());
        let generation = self.metadata_generations.entry(id.clone()).or_default();
        *generation += 1;
        let generation = *generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = service.database(&id).await.map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::DatabaseMeta {
                id,
                generation,
                result,
            });
        });
    }
    pub(super) fn load_database(&mut self, refresh: bool) {
        self.content_generation += 1;
        let id = self.location().unwrap().id.clone();
        self.databases.entry(id.clone()).or_default().generation = self.content_generation;
        self.loading_content = true;
        self.status = "Loading database…".into();
        self.error = false;
        if refresh {
            self.ensure_database_metadata(true);
            return;
        }
        if self.databases.get(&id).is_none_or(|db| db.meta.is_none()) {
            if let Some(error) = self.databases.get(&id).and_then(|db| db.error.clone()) {
                self.loading_content = false;
                self.fail(error);
            } else {
                self.ensure_database_metadata(false);
            }
            return;
        }
        let db = self.databases.get_mut(&id).unwrap();
        let Some(view) = db.view().cloned() else {
            db.error = Some("No saved views are available for this database".into());
            self.loading_content = false;
            return;
        };
        if !supported(&view) {
            db.error = Some(format!(
                "{} views are not supported. Use [ / ] to choose another view",
                view.view_type
            ));
            db.data = None;
            self.loading_content = false;
            return;
        }
        db.progress = 0;
        db.error = None;
        let generation = db.generation;
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let progress_sender = sender.clone();
            let progress_id = id.clone();
            let progress = std::sync::Arc::new(move |count| {
                let _ = progress_sender.send(Message::DatabaseProgress {
                    id: progress_id.clone(),
                    generation,
                    count,
                });
            });
            let result = service
                .database_rows(&view, progress)
                .await
                .map(Box::new)
                .map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::DatabaseRows {
                id,
                generation,
                result,
            });
        });
    }
    pub(super) fn receive_database_meta(
        &mut self,
        id: String,
        generation: u64,
        result: Result<DatabaseMeta, String>,
    ) {
        if self.metadata_generations.get(&id) != Some(&generation) {
            return;
        }
        self.metadata_pending.remove(&id);
        let active = self
            .location()
            .is_some_and(|l| l.kind == LocationKind::Database && l.id == id);
        let result = result.and_then(|meta| {
            if meta.id == id {
                Ok(meta)
            } else {
                Err("Database metadata returned a different ID".into())
            }
        });
        let db = self.databases.entry(id).or_default();
        match result {
            Ok(meta) => {
                let old_view = db.view().map(|v| v.id.clone());
                db.view_index = old_view
                    .and_then(|id| meta.views.iter().position(|v| v.id == id))
                    .unwrap_or_else(|| meta.views.iter().position(supported).unwrap_or(0));
                if active
                    && !meta.title.trim().is_empty()
                    && !meta.title.eq_ignore_ascii_case("Untitled")
                {
                    self.path.last_mut().unwrap().title = meta.title.clone();
                }
                db.meta = Some(meta);
                db.error = None;
                if active {
                    self.load_database(false);
                }
            }
            Err(error) => {
                db.error = Some(error.clone());
                if active {
                    self.loading_content = false;
                    self.fail(error);
                }
            }
        }
    }
    pub(super) fn receive_database_rows(
        &mut self,
        id: String,
        generation: u64,
        result: Result<DatabaseData, String>,
    ) {
        let active = self
            .location()
            .is_some_and(|l| l.kind == LocationKind::Database && l.id == id);
        let Some(db) = self.databases.get_mut(&id) else {
            return;
        };
        if db.generation != generation {
            return;
        }
        if active {
            self.loading_content = false;
        }
        match result {
            Ok(data) => {
                let selected = db.selected_row().map(|r| r.id.clone());
                let old_group = db.data.as_ref().and_then(|d| {
                    d.groups(rusticnotion::chrono::Local::now().date_naive())
                        .get(db.column)
                        .map(|g| g.key.clone())
                });
                db.selected = selected
                    .and_then(|id| data.rows.iter().position(|r| r.id == id))
                    .unwrap_or(0);
                let groups = data.groups(rusticnotion::chrono::Local::now().date_naive());
                db.column = old_group
                    .and_then(|key| {
                        groups.iter().position(|g| {
                            g.key == key && g.cards.iter().any(|c| c.row == db.selected)
                        })
                    })
                    .or_else(|| {
                        groups
                            .iter()
                            .position(|g| g.cards.iter().any(|c| c.row == db.selected))
                    })
                    .unwrap_or(0);
                db.card = groups
                    .get(db.column)
                    .and_then(|g| g.cards.iter().position(|c| c.row == db.selected))
                    .unwrap_or(0);
                if active {
                    self.status = format!(
                        "{} rows · [ / ] views · Enter open · v details",
                        data.rows.len()
                    );
                    self.error = false;
                }
                db.data = Some(data);
                db.error = None;
            }
            Err(error) => {
                db.error = Some(error.clone());
                if active {
                    self.fail(error);
                }
            }
        }
    }
    pub(super) fn database_key(&mut self, key: KeyEvent) -> bool {
        if self.focus != Focus::Content || self.database().is_none() {
            return false;
        }
        let id = self.location().unwrap().id.clone();
        match key.code {
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let db = self.databases.get_mut(&id).unwrap();
                let len = db.meta.as_ref().map_or(0, |m| m.views.len());
                if len > 0 {
                    db.view_index = if key.code == KeyCode::Char(']') {
                        (db.view_index + 1) % len
                    } else {
                        (db.view_index + len - 1) % len
                    };
                    db.selected = 0;
                    db.column = 0;
                    db.card = 0;
                    db.horizontal = 0;
                    db.data = None;
                    self.load_database(false);
                }
            }
            KeyCode::Char('r') => self.load_database(true),
            KeyCode::Char('e' | 'a' | 'n' | ' ') => self
                .fail("Database browsing is read-only. Enter opens a page for editing its content"),
            KeyCode::Char('v' | 'o') => {
                let detail = self
                    .database()
                    .and_then(|db| Some(db.data.as_ref()?.detail(db.selected_row()?)));
                if let Some(detail) = detail {
                    if key.code == KeyCode::Char('v') {
                        self.viewer = Some((detail, 0));
                    } else {
                        let links = external_links(&detail.rich_text);
                        match links.len() {
                            0 => self.fail("No HTTP or HTTPS links in this row"),
                            1 => self.pending_link = Some(links[0].1.clone()),
                            _ => self.link_picker = Some(LinkPicker { links, selected: 0 }),
                        }
                    }
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.databases.get_mut(&id).unwrap().move_vertical(true);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.databases.get_mut(&id).unwrap().move_vertical(false);
            }
            KeyCode::Char('h') => {
                self.databases.get_mut(&id).unwrap().move_horizontal(false);
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.databases.get_mut(&id).unwrap().move_horizontal(true);
            }
            KeyCode::Home | KeyCode::Char('g') => {
                self.databases.get_mut(&id).unwrap().move_edge(false);
            }
            KeyCode::End | KeyCode::Char('G') => {
                let db = self.databases.get_mut(&id).unwrap();
                db.move_edge(true);
            }
            KeyCode::Enter => {
                if self.loading_content {
                    return true;
                }
                if let Some(row) = self
                    .database()
                    .and_then(DatabaseState::selected_row)
                    .cloned()
                {
                    self.path.push(Location {
                        id: row.id.clone(),
                        title: row.title,
                        kind: LocationKind::Page,
                        page_id: row.id,
                        list_depth: 0,
                    });
                    self.load();
                }
            }
            _ => return false,
        }
        true
    }
}
