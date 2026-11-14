//! The socket's side of feedback (`crate::feedback`): the form is checked
//! here, the hub instance adds where the player stands (at a Scribe or a
//! Kafra), then the report is rate limited per account and stored.

use super::{Conn, Out};
use crate::db::feedback_repo::{Author, FeedbackRepo};
use crate::feedback::{self, Draft, FeedbackError};
use crate::world::{HubCommand, ServerMsg};
use tracing::{error, info};

fn refused(e: FeedbackError) -> Out {
    Out::Msg(ServerMsg::FeedbackResult { id: 0, code: e.code().into(), message: e.to_string() })
}

impl Conn {
    /// `FEEDBACK` from the client: checked, then to the hub (town only).
    pub(super) fn on_feedback(&mut self, draft: Draft) -> Vec<Out> {
        let draft = match draft.validate() {
            Ok(d) => d,
            Err(e) => return vec![refused(e)],
        };
        if !self.in_hub() {
            return vec![refused(FeedbackError::TooFar)];
        }
        self.forward(HubCommand::Feedback { user_id: self.user_id(), draft: Box::new(draft) })
    }

    /// The hub placed the report: rate limit, store, answer.
    pub(super) async fn store_feedback(&mut self, draft: Draft, place: feedback::Place) -> Vec<Out> {
        let author = self.session.character.read().await.as_ref().map(|c| Author {
            user_id: self.user_id(),
            character_id: c.id,
            character_name: c.name.clone(),
        });
        let Some(author) = author else {
            return vec![super::hub_error("no_character", "select a character first")];
        };
        let pool = &self.state.pool;
        let rate = match FeedbackRepo::recent(pool, author.user_id).await {
            Ok((day, ago)) => feedback::check_rate(day, ago),
            Err(e) => {
                error!("Feedback rate check failed for user {}: {e}", author.user_id);
                Err(FeedbackError::Storage)
            }
        };
        if let Err(e) = rate {
            return vec![refused(e)];
        }
        match FeedbackRepo::insert(pool, &author, &draft, Some(&place)).await {
            Ok(id) => {
                info!("Feedback #{id} ({} / {}) from user {}: {}", draft.kind, draft.priority, author.user_id, draft.title);
                let message = format!("Thank you! Your report #{id} is in the Scribe's ledger.");
                vec![Out::Msg(ServerMsg::FeedbackResult { id: id as u32, code: String::new(), message })]
            }
            Err(e) => {
                error!("Feedback insert failed for user {}: {e}", author.user_id);
                vec![refused(FeedbackError::Storage)]
            }
        }
    }
}
