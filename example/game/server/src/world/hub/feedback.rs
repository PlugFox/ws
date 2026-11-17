//! Feedback at the Scribe or a Kafra (`crate::feedback`): the instance
//! checks that the player stands at one and adds where they are; the
//! socket task rate limits and stores the report.

use super::{HubEvent, HubInstance, Outgoing, KAFRA};
use crate::feedback::{Draft, FeedbackError, Place, FEEDBACK_NPC};
use crate::world::protocol::ServerMsg;

/// Whether an NPC of this kind takes feedback.
pub fn takes_feedback(kind: &str) -> bool {
    kind == FEEDBACK_NPC || kind == KAFRA
}

impl HubInstance {
    /// A checked form from `user_id`: [`HubEvent::FeedbackPlaced`] when a
    /// Scribe or Kafra is in reach, else a refusal (`FEEDBACK_RESULT`).
    pub fn feedback(&mut self, user_id: u32, draft: Box<Draft>, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let p = &self.players[i];
        let map = self.world.map(p.map);
        let near = map.manifest.npcs.iter().any(|n| takes_feedback(&n.kind) && n.in_reach(p.x, p.y));
        let event = if near {
            let level = p.fighter.as_ref().map_or(p.profile.level, |f| f.level);
            let place = Place { map: map.id().to_string(), x: p.x, y: p.y, level, class: p.profile.archetype.clone() };
            HubEvent::FeedbackPlaced { draft, place }
        } else {
            let e = FeedbackError::TooFar;
            HubEvent::Control(ServerMsg::FeedbackResult { id: 0, code: e.code().into(), message: e.to_string() })
        };
        out.push((user_id, event));
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::*;
    use super::super::tests::{profile, quiet};
    use super::super::{HubEvent, HubInstance};
    use crate::feedback::Draft;
    use crate::world::protocol::ServerMsg;
    use std::sync::Arc;
    use uuid::Uuid;

    fn draft() -> Box<Draft> {
        Box::new(Draft {
            kind: "bug".into(),
            priority: "normal".into(),
            title: "Title".into(),
            body: "Body long enough for the form.".into(),
            client_version: "0.1".into(),
            user_agent: "test".into(),
        })
    }

    #[test]
    fn feedback_wants_a_scribe_or_kafra_in_reach_and_records_the_place() {
        let mut hub = HubInstance::new(1, Arc::new(test_world()), quiet());
        let mut out = Vec::new();
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        let i = hub.find(1).unwrap();
        // Far from the Kafra at (5,1).
        (hub.players[i].x, hub.players[i].y) = (9.5, 4.5);
        out.clear();
        hub.feedback(1, draft(), &mut out);
        assert!(
            matches!(&out[..], [(1, HubEvent::Control(ServerMsg::FeedbackResult { id: 0, code, .. }))] if code == "npc_too_far"),
            "{out:?}"
        );
        (hub.players[i].x, hub.players[i].y) = (5.5, 2.25);
        out.clear();
        hub.feedback(1, draft(), &mut out);
        match &out[..] {
            [(1, HubEvent::FeedbackPlaced { draft: d, place })] => {
                assert_eq!(d.title, "Title");
                assert_eq!((place.map.as_str(), place.x, place.y, place.level, place.class.as_str()), ("town", 5.5, 2.25, 1, "novice"));
            }
            other => panic!("expected feedback_placed, got {other:?}"),
        }
        // Not in this instance: nothing.
        out.clear();
        hub.feedback(9, draft(), &mut out);
        assert!(out.is_empty());
    }
}
