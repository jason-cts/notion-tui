use std::num::NonZeroUsize;

use super::{
    relocation::{parent_id, same_id, trashed},
    *,
};
use crate::model::TableInfo;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnChange {
    Insert(usize),
    Delete(usize),
}

impl ColumnChange {
    pub fn index(self) -> usize {
        match self {
            Self::Insert(index) | Self::Delete(index) => index,
        }
    }

    pub fn width_after(self, width: NonZeroUsize) -> Option<NonZeroUsize> {
        match self {
            Self::Insert(index) if index <= width.get() => {
                width.get().checked_add(1).and_then(NonZeroUsize::new)
            }
            Self::Delete(index) if index < width.get() => NonZeroUsize::new(width.get() - 1),
            _ => None,
        }
    }
}

pub struct ColumnReplacement {
    pub original: String,
    pub parent: String,
    pub table: TableInfo,
    pub rows: Vec<BlockItem>,
    pub change: ColumnChange,
    pub copy_may_exist: bool,
}

fn writable_rows(rows: &[BlockItem], width: usize) -> Result<Vec<Vec<RichText>>> {
    if rows.is_empty() {
        bail!("A replacement table requires at least one row");
    }
    rows.iter()
        .map(|row| {
            let cells = row
                .table_cells
                .as_ref()
                .filter(|cells| cells.len() == width)
                .context("Table has a row with a different column count")?;
            if row.children || !row.editable_table_row() {
                bail!(
                    "Table has content that cannot be copied safely, such as mentions or equations"
                );
            }
            Ok(cells
                .iter()
                .map(|cell| RichText(cell.input.clone().unwrap()).normalized())
                .collect())
        })
        .collect()
}

fn row_batches(rows: &[Vec<RichText>]) -> Result<Vec<Vec<Value>>> {
    // Leave room for the JSON wrapper and the parent table metadata.
    const MAX_BATCH_BYTES: usize = 490_000;
    let mut batches = Vec::new();
    let mut batch = Vec::new();
    let mut bytes = 0;
    for cells in rows {
        let row = json!({"object":"block","type":"table_row","table_row":{"cells":cells}});
        let size = serde_json::to_vec(&row)?.len() + 1;
        if size > MAX_BATCH_BYTES {
            bail!("A table row exceeds Notion’s maximum request size");
        }
        if batch.len() == 100 || bytes + size > MAX_BATCH_BYTES {
            batches.push(std::mem::take(&mut batch));
            bytes = 0;
        }
        batch.push(row);
        bytes += size;
    }
    if !batch.is_empty() {
        batches.push(batch);
    }
    Ok(batches)
}

fn same_rows(snapshot: &[BlockItem], fresh: &[BlockItem]) -> bool {
    snapshot.len() == fresh.len()
        && snapshot.iter().zip(fresh).all(|(before, after)| {
            same_id(&before.id, &after.id)
                && !before.modified.is_empty()
                && before.modified == after.modified
        })
}

impl ApiService {
    async fn checked_table(&self, id: &str, parent: &str, expected: TableInfo) -> Result<Value> {
        let table = self.raw_block(id).await?;
        if trashed(&table)
            || parent_id(&table).is_none_or(|id| !same_id(id, parent))
            || BlockItem::from_value(&table).table != Some(expected)
        {
            bail!("The table or its parent changed in Notion");
        }
        Ok(table)
    }

    pub(super) async fn replace_column(&self, request: &mut ColumnReplacement) -> Result<String> {
        if request.copy_may_exist {
            bail!("A previous replacement may exist. Check Notion before continuing");
        }
        let width = request
            .table
            .width
            .context("Table column count is unavailable")?;
        let new_width = request
            .change
            .width_after(width)
            .context("Invalid column change")?;
        let old_rows = writable_rows(&request.rows, width.get())?;
        let table = self
            .checked_table(&request.original, &request.parent, request.table)
            .await?;
        let fresh = self.children(&request.original).await?;
        if !same_rows(&request.rows, &fresh) || writable_rows(&fresh, width.get())? != old_rows {
            bail!("The table rows changed in Notion. Refresh before changing columns");
        }
        let mut new_rows = old_rows.clone();
        for cells in &mut new_rows {
            match request.change {
                ColumnChange::Insert(index) => cells.insert(index, RichText::default()),
                ColumnChange::Delete(index) => {
                    cells.remove(index);
                }
            }
        }
        let mut batches = row_batches(&new_rows)?.into_iter();
        let first = batches
            .next()
            .context("A replacement table requires at least one row")?;
        let payload = json!({
            "position":{"type":"after_block","after_block":{"id":request.original}},
            "children":[{"object":"block","type":"table","table":{
                "table_width":new_width.get(),"has_column_header":request.table.has_column_header,
                "has_row_header":request.table.has_row_header,"children":first
            }}]
        });
        request.copy_may_exist = true;
        let response = self.http.patch(format!("{}/blocks/{}/children", self.base_url, request.parent))
            .json(&payload).send().await
            .context("Replacement response unavailable. Original retained. Inspect Notion before retrying")?;
        if matches!(
            response.status().as_u16(),
            400 | 401 | 403 | 404 | 409 | 413 | 429
        ) {
            request.copy_may_exist = false;
        }
        let response = self
            .write_response(response, false)
            .await
            .context("Could not create replacement. Original retained")?;
        let id = response["results"][0]["id"]
            .as_str()
            .filter(|id| !id.is_empty() && !same_id(id, &request.original))
            .context(
                "Replacement ID unavailable. Original retained. Inspect Notion before retrying",
            )?;

        let finish = async {
            for batch in batches {
                self.patch(
                    &format!("/blocks/{id}/children"),
                    json!({"children": batch}),
                )
                .await?;
            }
            self.checked_table(
                id,
                &request.parent,
                TableInfo {
                    width: Some(new_width),
                    ..request.table
                },
            )
            .await?;
            if writable_rows(&self.children(id).await?, new_width.get())? != new_rows {
                bail!("Replacement did not preserve the expected cell contents");
            }
            let latest = self
                .checked_table(&request.original, &request.parent, request.table)
                .await?;
            let rows = self.children(&request.original).await?;
            if latest["last_edited_time"] != table["last_edited_time"]
                || !same_rows(&fresh, &rows)
                || writable_rows(&rows, width.get())? != old_rows
            {
                bail!("Original table changed during replacement");
            }
            self.send_write(
                self.http
                    .delete(format!("{}/blocks/{}", self.base_url, request.original)),
                true,
            )
            .await
        };
        finish.await.with_context(|| format!("Replacement {id} may be incomplete or the original may remain. Check both tables in Notion"))?;
        Ok(id.to_owned())
    }
}
