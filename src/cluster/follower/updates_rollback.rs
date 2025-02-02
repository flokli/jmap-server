/*
 * Copyright (c) 2020-2022, Stalwart Labs Ltd.
 *
 * This file is part of the Stalwart JMAP Server.
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU Affero General Public License as
 * published by the Free Software Foundation, either version 3 of
 * the License, or (at your option) any later version.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE. See the
 * GNU Affero General Public License for more details.
 * in the LICENSE file at the top-level directory of this distribution.
 * You should have received a copy of the GNU Affero General Public License
 * along with this program.  If not, see <http://www.gnu.org/licenses/>.
 *
 * You can be released from the requirements of the AGPLv3 license by
 * purchasing a commercial license. Please contact licensing@stalw.art
 * for more details.
*/

use super::rpc::Response;
use super::State;
use crate::cluster::log::changes_merge::MergedChanges;
use crate::cluster::log::update_apply::RaftStoreApplyUpdate;
use crate::cluster::log::AppendEntriesResponse;
use crate::cluster::log::Update;
use crate::JMAPServer;
use store::core::collection::Collection;
use store::tracing::{debug, error};
use store::write::batch::WriteBatch;
use store::{AccountId, Store};

impl<T> JMAPServer<T>
where
    T: for<'x> Store<'x> + 'static,
{
    pub async fn handle_rollback_updates(
        &self,
        mut account_id: AccountId,
        mut collection: Collection,
        mut changes: MergedChanges,
        mut updates: Vec<Update>,
    ) -> Option<(State, Response)> {
        loop {
            // Thread collection does not contain any actual records,
            // it exists solely for change tracking.
            if let Collection::Thread = collection {
                changes.inserts.clear();
                changes.updates.clear();
                changes.deletes.clear();
            }

            if !changes.inserts.is_empty() {
                let inserts = std::mem::take(&mut changes.inserts);
                let store = self.store.clone();
                if let Err(err) = self
                    .spawn_worker(move || {
                        let mut batch = WriteBatch::new(account_id);
                        for delete_id in inserts {
                            store.delete_document(&mut batch, collection, delete_id)?;
                        }

                        store.write(batch)
                    })
                    .await
                {
                    error!("Failed to delete documents: {:?}", err);
                    return None;
                }
            }

            if !updates.is_empty() {
                match self.apply_rollback_updates(updates).await {
                    Ok(is_done) => {
                        if is_done {
                            changes.updates.clear();
                            changes.deletes.clear();
                        } else {
                            return (
                                State::Rollback {
                                    account_id,
                                    collection,
                                    changes,
                                },
                                Response::AppendEntries(AppendEntriesResponse::Continue),
                            )
                                .into();
                        }
                    }
                    Err(err) => {
                        debug!("Failed to update store: {:?}", err);
                        return None;
                    }
                }
                updates = vec![];
            }

            if !changes.deletes.is_empty() || !changes.updates.is_empty() {
                let serialized_changes = match changes.serialize() {
                    Some(changes) => changes,
                    None => {
                        error!("Failed to serialize bitmap.");
                        return None;
                    }
                };

                return (
                    State::Rollback {
                        account_id,
                        collection,
                        changes,
                    },
                    Response::AppendEntries(AppendEntriesResponse::Update {
                        account_id,
                        collection,
                        changes: serialized_changes,
                        is_rollback: true,
                    }),
                )
                    .into();
            } else {
                if let Err(err) = self.remove_rollback_change(account_id, collection).await {
                    error!("Failed to remove rollback change key: {:?}", err);
                    return None;
                }

                match self.next_rollback_change().await {
                    Ok(Some((next_account_id, next_collection, next_changes))) => {
                        account_id = next_account_id;
                        collection = next_collection;
                        changes = next_changes;
                        continue;
                    }
                    Ok(None) => {
                        return (
                            State::default(),
                            Response::AppendEntries(AppendEntriesResponse::Match {
                                match_log: match self.get_last_log().await {
                                    Ok(Some(last_log)) => last_log,
                                    Ok(None) => {
                                        error!("Unexpected error: Last log not found.");
                                        return None;
                                    }
                                    Err(err) => {
                                        debug!("Failed to get prev raft id: {:?}", err);
                                        return None;
                                    }
                                },
                            }),
                        )
                            .into();
                    }
                    Err(err) => {
                        error!("Failed to obtain rollback changes: {:?}", err);
                        return None;
                    }
                }
            }
        }
    }
}
