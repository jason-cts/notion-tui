use super::*;

/// Kept by the app across failures, including OAuth renewal. Never replay an
/// append whose result is unknown; a known copy can resume at verification.
#[derive(Clone, Debug)]
pub enum RelocationState {
    Ready,
    Uncertain,
    Copied { id: String, payload: Value },
}

#[derive(Clone, Debug)]
pub struct Relocation {
    pub original: BlockItem,
    pub source_parent: String,
    pub destination: String,
    pub position: InsertPosition,
    pub state: RelocationState,
}

pub(super) fn same_id(left: &str, right: &str) -> bool {
    left.replace('-', "")
        .eq_ignore_ascii_case(&right.replace('-', ""))
}

pub(super) fn parent_id(value: &Value) -> Option<&str> {
    let parent = &value["parent"];
    match parent["type"].as_str()? {
        "page_id" => parent["page_id"].as_str(),
        "block_id" => parent["block_id"].as_str(),
        _ => None,
    }
}

pub(super) fn trashed(value: &Value) -> bool {
    value["in_trash"] == true || value["archived"] == true
}

// Build only supported request fields, using the fresh response so block color
// and to-do state survive. Refuse unknown body fields rather than dropping them.
fn leaf_payload(value: &Value) -> Result<Value> {
    let block = BlockItem::from_value(value);
    if !block.relocatable() || value["has_children"] != false || trashed(value) {
        bail!(
            "Only leaf paragraphs, bullets, numbered items, and to-dos can be indented/outdented"
        );
    }
    let body = value[&block.block_type]
        .as_object()
        .context("Missing block content")?;
    if body
        .keys()
        .any(|key| !matches!(key.as_str(), "rich_text" | "color" | "checked"))
    {
        bail!("This block has unsupported content; indent/outdent would lose data");
    }
    let mut fields = json!({"rich_text": block.input.unwrap().normalized()});
    fields["color"] = body.get("color").cloned().unwrap_or(json!("default"));
    if block.kind == Some(Kind::Todo) {
        fields["checked"] = json!(
            body.get("checked")
                .and_then(Value::as_bool)
                .context("Missing to-do state")?
        );
    }
    Ok(json!({"object":"block", "type": block.block_type, block.block_type: fields}))
}

impl ApiService {
    pub(super) async fn raw_block(&self, id: &str) -> Result<Value> {
        let response = self
            .http
            .get(format!("{}/blocks/{id}", self.base_url))
            .send()
            .await?;
        if response.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(AuthError::Unauthorized.into());
        }
        Ok(response.error_for_status()?.json().await?)
    }

    async fn check_relocation_source(&self, movement: &Relocation) -> Result<Value> {
        let fresh = self.raw_block(&movement.original.id).await?;
        if fresh["last_edited_time"].as_str() != Some(&movement.original.modified)
            || fresh["type"].as_str() != Some(&movement.original.block_type)
            || parent_id(&fresh).is_none_or(|id| !same_id(id, &movement.source_parent))
        {
            bail!(
                "Conflict: block or parent changed in Notion; refresh before indenting/outdenting"
            );
        }
        leaf_payload(&fresh)?;
        Ok(fresh)
    }

    async fn check_relocation_destination(&self, movement: &Relocation) -> Result<()> {
        match &movement.position {
            InsertPosition::End => {
                let target = self.raw_block(&movement.destination).await?;
                if trashed(&target)
                    || !BlockItem::from_value(&target).accepts_nested_blocks()
                    || parent_id(&target).is_none_or(|id| !same_id(id, &movement.source_parent))
                {
                    bail!(
                        "The previous sibling can no longer contain this block; refresh before indenting"
                    );
                }
                let siblings = self.children(&movement.source_parent).await?;
                let index = siblings
                    .iter()
                    .position(|b| same_id(&b.id, &movement.original.id));
                if index
                    .and_then(|index| index.checked_sub(1))
                    .and_then(|index| siblings.get(index))
                    .is_none_or(|b| !same_id(&b.id, &movement.destination))
                {
                    bail!("Conflict: sibling order changed in Notion; refresh before indenting");
                }
            }
            InsertPosition::After(anchor) if same_id(anchor, &movement.source_parent) => {
                let parent = self.raw_block(anchor).await?;
                if trashed(&parent)
                    || !BlockItem::from_value(&parent).accepts_nested_blocks()
                    || parent_id(&parent).is_none_or(|id| !same_id(id, &movement.destination))
                {
                    bail!(
                        "Conflict: parent moved or cannot be outdented from; refresh before outdenting"
                    );
                }
                if parent["parent"]["type"] == "block_id" {
                    let target = self.raw_block(&movement.destination).await?;
                    if trashed(&target) || !BlockItem::from_value(&target).accepts_nested_blocks() {
                        bail!("The grandparent cannot contain this block");
                    }
                }
            }
            _ => bail!("Invalid indent/outdent destination"),
        }
        Ok(())
    }

    pub(super) async fn relocate_block(&self, movement: &mut Relocation) -> Result<String> {
        if matches!(movement.state, RelocationState::Uncertain) {
            bail!(
                "Copy result is unknown for block {}. Inspect Notion and resolve any copy manually before trying another move; no request was replayed",
                movement.original.id
            );
        }
        if !movement.original.relocatable()
            || movement.source_parent.is_empty()
            || movement.destination.is_empty()
            || same_id(&movement.original.id, &movement.destination)
        {
            bail!("This block cannot be indented/outdented");
        }
        if matches!(movement.state, RelocationState::Ready) {
            let original = self.check_relocation_source(movement).await?;
            self.check_relocation_destination(movement).await?;
            let payload = leaf_payload(&original)?;
            let mut append = json!({"children": [payload.clone()]});
            if let InsertPosition::After(id) = &movement.position {
                append["position"] = json!({"type":"after_block", "after_block":{"id":id}});
            }
            movement.state = RelocationState::Uncertain;
            let response = self
                .http
                .patch(format!(
                    "{}/blocks/{}/children",
                    self.base_url, movement.destination
                ))
                .json(&append)
                .send()
                .await
                .context(
                    "Copy response unavailable; original retained. Inspect Notion before retrying",
                )?;
            // A rejected request can be explicitly retried. A successful but
            // unreadable response remains uncertain and must never be replayed.
            if matches!(
                response.status().as_u16(),
                400 | 401 | 403 | 404 | 409 | 413 | 429
            ) {
                movement.state = RelocationState::Ready;
            }
            let response = self
                .write_response(response, false)
                .await
                .context("Could not create replacement; original retained")?;
            let id = response["results"][0]["id"].as_str().filter(|id| !id.is_empty())
                .context("Copy response has no block ID; original retained. Inspect Notion before retrying")?.to_owned();
            if same_id(&id, &movement.original.id) {
                bail!("Invalid replacement ID; original retained. Inspect Notion before retrying");
            }
            movement.state = RelocationState::Copied { id, payload };
        }
        let RelocationState::Copied { id, payload } = &movement.state else {
            unreachable!()
        };
        let id = id.clone();
        let result = async {
            let replacement = self.raw_block(&id).await?;
            if parent_id(&replacement).is_none_or(|p| !same_id(p, &movement.destination))
                || leaf_payload(&replacement)? != *payload
            {
                bail!("Replacement changed; original was not trashed");
            }
            let original = self.raw_block(&movement.original.id).await?;
            // A lost delete response is safe to resolve without another append.
            if trashed(&original) {
                return Ok(id.clone());
            }
            self.check_relocation_destination(movement).await?;
            let fresh = self.check_relocation_source(movement).await?;
            if leaf_payload(&fresh)? != *payload {
                bail!("Original content changed; original was not trashed");
            }
            self.send_write(
                self.http
                    .delete(format!("{}/blocks/{}", self.base_url, movement.original.id)),
                true,
            )
            .await?;
            Ok::<_, anyhow::Error>(id.clone())
        }
        .await;
        result.with_context(|| format!("Replacement {id} exists; original {} may remain. Select the original and repeat >/< to resume without another copy", movement.original.id))
    }
}

#[cfg(test)]
mod tests;
