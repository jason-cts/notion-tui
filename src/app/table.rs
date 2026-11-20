use super::*;
use crate::service::ColumnReplacement;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TableMenuKind {
    Insert,
    Delete,
}

pub struct TableMenu {
    pub kind: TableMenuKind,
    pub selected: usize,
}

impl TableMenu {
    pub fn options(&self) -> &'static [&'static str] {
        match self.kind {
            TableMenuKind::Insert => &[
                "Insert row above",
                "Insert row below",
                "Insert column left",
                "Insert column right",
            ],
            TableMenuKind::Delete => &["Delete row", "Delete column"],
        }
    }
}

impl App {
    pub fn table_info(&self) -> Option<TableInfo> {
        match self.location()?.kind {
            LocationKind::Table(info) => Some(info),
            _ => None,
        }
    }

    pub fn table_columns(&self) -> usize {
        self.table_info().and_then(|table| table.width).map_or_else(
            || {
                self.blocks
                    .iter()
                    .filter_map(|row| row.table_cells.as_ref().map(Vec::len))
                    .max()
                    .unwrap_or(0)
            },
            |width| width.get(),
        )
    }

    pub(super) fn simple_table_key(&mut self, key: KeyEvent) -> bool {
        if self.focus != Focus::Content || self.table_info().is_none() {
            return false;
        }
        match key.code {
            KeyCode::Left => self.table_column = self.table_column.saturating_sub(1),
            KeyCode::Right => {
                self.table_column =
                    (self.table_column + 1).min(self.table_columns().saturating_sub(1));
            }
            KeyCode::Char('a' | 'd') => {
                if !self.loading_content {
                    let kind = if key.code == KeyCode::Char('a') {
                        TableMenuKind::Insert
                    } else {
                        TableMenuKind::Delete
                    };
                    self.table_menu = Some(TableMenu { kind, selected: 0 });
                }
            }
            KeyCode::Char('A') | KeyCode::Enter => {}
            _ => return false,
        }
        true
    }

    pub(super) fn table_menu_key(&mut self, key: KeyEvent) -> bool {
        let Some(menu) = &mut self.table_menu else {
            return false;
        };
        match key.code {
            KeyCode::Up => menu.selected = menu.selected.saturating_sub(1),
            KeyCode::Down => menu.selected = (menu.selected + 1).min(menu.options().len() - 1),
            KeyCode::Esc => self.table_menu = None,
            KeyCode::Enter => {
                let menu = self.table_menu.take().unwrap();
                match (menu.kind, menu.selected) {
                    (TableMenuKind::Insert, 0) => self.start_insert(true),
                    (TableMenuKind::Insert, 1) => self.start_insert(false),
                    (TableMenuKind::Insert, index) => self.prepare_column(ColumnChange::Insert(
                        self.table_column + usize::from(index == 3),
                    )),
                    (TableMenuKind::Delete, 0) => self.prepare_delete(),
                    (TableMenuKind::Delete, _) => {
                        self.prepare_column(ColumnChange::Delete(self.table_column))
                    }
                }
            }
            _ => {}
        }
        true
    }

    pub(super) fn start_insert(&mut self, above: bool) {
        if self.loading_content {
            return;
        }
        let Some(location) = self.location() else {
            return;
        };
        let position = if self.blocks.is_empty() {
            InsertPosition::End
        } else if let Some(selected) = self
            .blocks
            .get(self.block_selected)
            .filter(|b| !b.id.is_empty())
        {
            if above {
                match self.block_selected.checked_sub(1) {
                    None => InsertPosition::Start,
                    Some(index) if !self.blocks[index].id.is_empty() => {
                        InsertPosition::After(self.blocks[index].id.clone())
                    }
                    _ => {
                        self.fail("Select a block before inserting");
                        return;
                    }
                }
            } else {
                InsertPosition::After(selected.id.clone())
            }
        } else {
            self.fail("Select a block before inserting");
            return;
        };
        let parent = location.id.clone();
        let target = match location.kind {
            LocationKind::Table(table) => {
                let Some(width) = table.width else {
                    self.fail("Table column count is unavailable");
                    return;
                };
                Target::AppendTableRow {
                    parent,
                    position,
                    width,
                }
            }
            _ => Target::Append { parent, position },
        };
        self.editor = Some(Editor::new(target));
    }

    pub(super) fn prepare_delete(&mut self) {
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
        } else if self.table_info().is_some() {
            self.fail("Select a row to delete");
        }
    }

    fn prepare_column(&mut self, change: ColumnChange) {
        let Some(location) = self.location() else {
            return;
        };
        if self.uncertain_tables.contains(&location.id) {
            self.fail("A previous replacement may exist. Check the original and its neighboring blocks in Notion before changing columns again");
        } else if self.blocks.is_empty() {
            self.fail("Insert a row before changing columns");
        } else if matches!(change, ColumnChange::Delete(_)) && self.table_columns() <= 1 {
            self.fail("A simple table needs at least one column");
        } else {
            self.pending_column = Some(change);
            self.confirm = Some(Confirm::ChangeColumn);
        }
    }

    pub(super) fn change_column(&mut self) {
        let Some(change) = self.pending_column.take() else {
            return;
        };
        let (Some(location), Some(table), Some(parent)) = (
            self.location(),
            self.table_info(),
            self.path.iter().rev().nth(1),
        ) else {
            return;
        };
        let mut request = ColumnReplacement {
            original: location.id.clone(),
            parent: parent.id.clone(),
            table,
            rows: self.blocks.clone(),
            change,
            copy_may_exist: false,
        };
        self.saving = true;
        self.error = false;
        self.status = "Replacing table…".into();
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = service
                .replace_table_column(&mut request)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::TableReplaced {
                original: request.original,
                change,
                copied: request.copy_may_exist,
                result,
            });
        });
    }

    pub(super) fn finish_table_replacement(
        &mut self,
        original: String,
        change: ColumnChange,
        copied: bool,
        result: Result<String, String>,
    ) {
        self.saving = false;
        match result {
            Ok(id) => {
                if let Some(parent) = self.path.iter().rev().nth(1)
                    && let Some(selection) = self.parent_selections.get_mut(&parent.id)
                    && selection.id == original
                {
                    selection.id = id.clone();
                }
                if let Some(location) = self.path.last_mut()
                    && location.id == original
                    && let LocationKind::Table(mut table) = location.kind
                    && let Some(width) = table.width
                {
                    table.width = change.width_after(width);
                    location.kind = LocationKind::Table(table);
                    location.id = id.clone();
                    self.table_column = change.index();
                    self.selection_after_delete = Some((id, self.block_selected));
                    self.load();
                }
            }
            Err(error) => {
                if copied {
                    self.uncertain_tables.insert(original);
                }
                self.fail(error);
            }
        }
    }
}
