// SPDX-License-Identifier: Apache-2.0

//! An in-memory `InboxRepository` for the HTTP route tests. It returns what a test sets and
//! records each event that the route inserts.

use std::sync::Mutex;

use async_trait::async_trait;
use paigasus_agent_host_core::ids::EventId;
use paigasus_agent_host_core::inbox::{Inbox, NewInboxEvent};
use paigasus_agent_host_core::ports::{InboxRepository, InsertOutcome, RepositoryError};
use uuid::Uuid;

/// What the insert answers.
#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub enum InsertAnswer {
    /// `Inserted` with the id of the event that the route sent.
    Echo,
    /// `Duplicate` with this stored id.
    Duplicate(EventId),
    /// A store error.
    Fail,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
pub struct FakeInbox {
    inbox: Option<Inbox>,
    lookup_fails: bool,
    insert: InsertAnswer,
    lookups: Mutex<Vec<Uuid>>,
    inserted: Mutex<Vec<NewInboxEvent>>,
}

#[allow(dead_code, reason = "each test binary compiles this file and uses a part of it")]
impl FakeInbox {
    fn new(inbox: Option<Inbox>) -> Self {
        FakeInbox {
            inbox,
            lookup_fails: false,
            insert: InsertAnswer::Echo,
            lookups: Mutex::new(Vec::new()),
            inserted: Mutex::new(Vec::new()),
        }
    }

    /// A repository that knows this inbox.
    pub fn with(inbox: Inbox) -> Self {
        FakeInbox::new(Some(inbox))
    }

    /// A repository with no inbox.
    pub fn empty() -> Self {
        FakeInbox::new(None)
    }

    /// The lookup returns a store error.
    pub fn failing_lookup(mut self) -> Self {
        self.lookup_fails = true;
        self
    }

    /// Set the answer of the insert.
    pub fn answering(mut self, insert: InsertAnswer) -> Self {
        self.insert = insert;
        self
    }

    /// The ids that the route looked up.
    pub fn lookups(&self) -> Vec<Uuid> {
        self.lookups.lock().unwrap().clone()
    }

    /// The events that the route inserted.
    pub fn inserted(&self) -> Vec<NewInboxEvent> {
        self.inserted.lock().unwrap().clone()
    }
}

#[async_trait]
impl InboxRepository for FakeInbox {
    async fn get_by_uuid(&self, id: Uuid) -> Result<Option<Inbox>, RepositoryError> {
        self.lookups.lock().unwrap().push(id);
        if self.lookup_fails {
            return Err(RepositoryError::Store("fake lookup failure".to_string()));
        }
        Ok(self.inbox.clone())
    }

    async fn insert_event(&self, event: NewInboxEvent) -> Result<InsertOutcome, RepositoryError> {
        self.inserted.lock().unwrap().push(event.clone());
        match &self.insert {
            InsertAnswer::Echo => Ok(InsertOutcome::Inserted { id: event.id }),
            InsertAnswer::Duplicate(id) => Ok(InsertOutcome::Duplicate { id: *id, payload_matches: true }),
            InsertAnswer::Fail => Err(RepositoryError::Store("fake insert failure".to_string())),
        }
    }
}
