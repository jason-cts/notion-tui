use super::*;
use crate::service::{Relocation, RelocationState};

#[derive(Clone, Debug)]
pub struct PendingRelocation {
    pub request: Relocation,
    pub path: Vec<Location>,
    pub indent: bool,
}

#[cfg(test)]
mod tests;

impl App {
    pub(super) fn prepare_relocation(&mut self, indent: bool) {
        if self.loading_content
            || self
                .location()
                .is_none_or(|l| l.kind == LocationKind::Database)
        {
            return;
        }
        let Some(block) = self.blocks.get(self.block_selected).cloned() else {
            return;
        };
        if let Some(pending) = self.relocation_recovery.get(&block.id) {
            if matches!(pending.request.state, RelocationState::Uncertain) {
                self.fail("Previous copy result is unknown. Inspect Notion and resolve the copy manually; indent/outdent will not create another copy in this session");
                return;
            }
            self.pending_relocation = Some(pending.clone());
            self.confirm = Some(Confirm::Relocate);
            return;
        }
        if !block.relocatable() {
            self.fail("Indent/outdent supports leaf paragraphs, bullets, numbered items, and to-dos without mentions or equations");
            return;
        }
        let source_parent = self.location().unwrap().id.clone();
        let mut destination_path = self.path.clone();
        let (destination, position) = if indent {
            let Some(previous) = self
                .block_selected
                .checked_sub(1)
                .and_then(|i| self.blocks.get(i))
            else {
                self.fail("The first sibling cannot be indented");
                return;
            };
            if previous.id.is_empty() || !previous.accepts_nested_blocks() {
                self.fail("The previous sibling cannot contain nested blocks");
                return;
            }
            let location = self.location().unwrap();
            destination_path.push(Location {
                id: previous.id.clone(),
                title: previous.text.clone(),
                kind: LocationKind::Blocks,
                page_id: location.page_id.clone(),
                list_depth: location.list_depth
                    + usize::from(matches!(
                        previous.kind,
                        Some(Kind::Bullet | Kind::Number | Kind::Todo)
                    )),
            });
            (previous.id.clone(), InsertPosition::End)
        } else {
            if self.location().unwrap().kind != LocationKind::Blocks || self.path.len() < 2 {
                self.fail("Top-level blocks cannot be outdented");
                return;
            }
            destination_path.pop();
            let grandparent = destination_path.last().unwrap();
            if grandparent.kind == LocationKind::Database {
                self.fail("Blocks cannot be outdented into a database");
                return;
            }
            (
                grandparent.id.clone(),
                InsertPosition::After(source_parent.clone()),
            )
        };
        self.pending_relocation = Some(PendingRelocation {
            request: Relocation {
                original: block,
                source_parent,
                destination,
                position,
                state: RelocationState::Ready,
            },
            path: destination_path,
            indent,
        });
        self.confirm = Some(Confirm::Relocate);
    }

    pub(super) fn relocate_block(&mut self) {
        let Some(mut pending) = self.pending_relocation.take() else {
            return;
        };
        self.saving = true;
        self.error = false;
        self.status = if pending.indent {
            "Indenting…"
        } else {
            "Outdenting…"
        }
        .into();
        let service = self.service.clone();
        let sender = self.sender.clone();
        tokio::spawn(async move {
            let result = service
                .relocate(&mut pending.request)
                .await
                .map_err(|e| format!("{e:#}"));
            let _ = sender.send(Message::Relocated {
                pending: Box::new(pending),
                result,
            });
        });
    }

    pub(super) fn receive_relocation(
        &mut self,
        pending: PendingRelocation,
        result: Result<String, String>,
    ) {
        self.saving = false;
        match result {
            Ok(id) => {
                self.relocation_recovery
                    .remove(&pending.request.original.id);
                self.path = pending.path;
                self.focus = Focus::Content;
                self.selection_after_update = Some((pending.request.destination, id));
                self.load();
            }
            Err(error) => {
                if !matches!(pending.request.state, RelocationState::Ready) {
                    self.relocation_recovery
                        .insert(pending.request.original.id.clone(), pending);
                }
                self.fail(error);
            }
        }
    }
}
