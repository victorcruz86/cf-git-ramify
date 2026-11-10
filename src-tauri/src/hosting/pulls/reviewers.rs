// Who reviewed a pull request and how: GHReviewersUtils.getReviewsByReviewers (plugins/github at
// idea/241.14494.240, P§6.4). The list rows and the details' status lines read the same map.

use serde::{Deserialize, Serialize};

// `ReviewState` of collaboration-tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ReviewState {
    Accepted,
    WaitForUpdates,
    NeedReview,
}

// A reviewer and what the map says of them. `name` is the presentable name: the full name, or
// else the login (a team's combined slug).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Reviewer {
    pub name: String,
    pub state: ReviewState,
}

// The `actorInfo` fragment: a user, a bot, a mannequin or an organization.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawActor {
    #[serde(rename = "__typename")]
    pub typename: Option<String>,
    pub id: Option<String>,
    pub login: Option<String>,
    pub name: Option<String>,
}

// The `pullRequestReviewerInfo` fragment: a user or a team.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawReviewer {
    #[serde(rename = "__typename")]
    pub typename: Option<String>,
    pub id: Option<String>,
    pub login: Option<String>,
    pub name: Option<String>,
    pub combined_slug: Option<String>,
}

// What the map reads of a review (`pullRequestReview`).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RawReview {
    pub author: Option<RawActor>,
    pub state: Option<String>,
}

// A node of `reviewRequests`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RawReviewRequest {
    pub requested_reviewer: Option<RawReviewer>,
}

// A key of the map. Two are the same reviewer when their node ids are (`GHNode.equals`).
#[derive(Debug, Clone, PartialEq, Eq)]
struct Key {
    id: String,
    short_name: String,
    name: Option<String>,
}

impl Key {
    fn presentable(&self) -> String {
        self.name.clone().unwrap_or_else(|| self.short_name.clone())
    }
}

// Decision 5: connect does not ask for `findUser("ghost")`, so the ghost user is this one, with
// the login that request returns.
const GHOST: &str = "ghost";

fn ghost() -> Key {
    Key {
        id: GHOST.to_string(),
        short_name: GHOST.to_string(),
        name: None,
    }
}

// `author as? GHUser ?: ghostUser`: a bot, a mannequin, an organization or a deleted account is
// the ghost user.
fn user_or_ghost(actor: Option<&RawActor>) -> Key {
    actor
        .filter(|actor| actor.typename.as_deref() == Some("User"))
        .and_then(|actor| {
            Some(Key {
                id: actor.id.clone()?,
                short_name: actor.login.clone()?,
                name: actor.name.clone(),
            })
        })
        .unwrap_or_else(ghost)
}

// A requested reviewer: a user (`shortName` = login) or a team (`shortName` = combinedSlug).
// Anything else GitHub may send here is left out.
fn requested(reviewer: &RawReviewer) -> Option<Key> {
    let short_name = match reviewer.typename.as_deref() {
        Some("User") => reviewer.login.clone()?,
        Some("Team") => reviewer.combined_slug.clone()?,
        _ => return None,
    };
    Some(Key {
        id: reviewer.id.clone()?,
        short_name,
        name: reviewer.name.clone(),
    })
}

// A `LinkedHashMap` put: a key already there keeps its place and takes the new value.
fn put<V>(entries: &mut Vec<(Key, V)>, key: Key, value: V) {
    match entries.iter_mut().find(|(known, _)| known.id == key.id) {
        Some(entry) => entry.1 = value,
        None => entries.push((key, value)),
    }
}

// The last review per reviewer wins, in API order; a reviewer other than the author who last
// approved is ACCEPTED and one who last asked for changes WAIT_FOR_UPDATES, any other last state
// adds nothing. Then each requested reviewer is NEED_REVIEW, over what came before. The order is
// the map's insertion order.
pub fn map(
    author: Option<&RawActor>,
    reviews: &[RawReview],
    requests: &[RawReviewRequest],
) -> Vec<Reviewer> {
    let mut last: Vec<(Key, Option<&str>)> = Vec::new();
    for review in reviews {
        put(
            &mut last,
            user_or_ghost(review.author.as_ref()),
            review.state.as_deref(),
        );
    }
    let author = author.and_then(|author| author.id.as_deref());
    let mut result: Vec<(Key, ReviewState)> = Vec::new();
    for (reviewer, state) in last {
        if Some(reviewer.id.as_str()) == author {
            continue;
        }
        match state {
            Some("APPROVED") => put(&mut result, reviewer, ReviewState::Accepted),
            Some("CHANGES_REQUESTED") => put(&mut result, reviewer, ReviewState::WaitForUpdates),
            _ => {}
        }
    }
    for reviewer in requests
        .iter()
        .filter_map(|request| request.requested_reviewer.as_ref())
        .filter_map(requested)
    {
        put(&mut result, reviewer, ReviewState::NeedReview);
    }
    result
        .into_iter()
        .map(|(reviewer, state)| Reviewer {
            name: reviewer.presentable(),
            state,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn actor(typename: &str, id: &str, login: &str, name: Option<&str>) -> RawActor {
        RawActor {
            typename: Some(typename.to_string()),
            id: Some(id.to_string()),
            login: Some(login.to_string()),
            name: name.map(str::to_string),
        }
    }

    fn user(id: &str, login: &str) -> RawActor {
        actor("User", id, login, None)
    }

    fn review(author: Option<RawActor>, state: &str) -> RawReview {
        RawReview {
            author,
            state: Some(state.to_string()),
        }
    }

    fn request(typename: &str, id: &str, login: &str, name: Option<&str>) -> RawReviewRequest {
        let reviewer = RawReviewer {
            typename: Some(typename.to_string()),
            id: Some(id.to_string()),
            login: (typename == "User").then(|| login.to_string()),
            name: name.map(str::to_string),
            combined_slug: (typename == "Team").then(|| login.to_string()),
        };
        RawReviewRequest {
            requested_reviewer: Some(reviewer),
        }
    }

    fn entry(name: &str, state: ReviewState) -> Reviewer {
        Reviewer {
            name: name.to_string(),
            state,
        }
    }

    #[test]
    fn the_reviewer_map_follows_241() {
        let author = user("U_author", "author");
        let reviews = [
            // Approved, then only commented: the last review wins, so alice adds nothing.
            review(Some(user("U_alice", "alice")), "APPROVED"),
            review(Some(user("U_bob", "bob")), "CHANGES_REQUESTED"),
            review(Some(user("U_alice", "alice")), "COMMENTED"),
            // Asked for changes, then approved.
            review(
                Some(actor("User", "U_carol", "carol", Some("Carol C"))),
                "CHANGES_REQUESTED",
            ),
            review(
                Some(actor("User", "U_carol", "carol", Some("Carol C"))),
                "APPROVED",
            ),
            // The author's own review is left out.
            review(Some(author.clone()), "APPROVED"),
            // A bot, a mannequin and a deleted account are the ghost user, once.
            review(Some(actor("Bot", "B_1", "dependabot", None)), "APPROVED"),
            review(None, "APPROVED"),
            review(
                Some(actor("Mannequin", "M_1", "old", None)),
                "CHANGES_REQUESTED",
            ),
            review(Some(user("U_dave", "dave")), "DISMISSED"),
            review(Some(user("U_erin", "erin")), "APPROVED"),
        ];
        let requests = [
            // A request overrides an earlier state and keeps its place.
            request("User", "U_bob", "bob", None),
            request("User", "U_frank", "frank", Some("Frank F")),
            request("Team", "T_1", "acme/core", Some("Core")),
            request("Team", "T_2", "acme/docs", None),
            RawReviewRequest {
                requested_reviewer: None,
            },
        ];
        assert_eq!(
            map(Some(&author), &reviews, &requests),
            [
                entry("bob", ReviewState::NeedReview),
                entry("Carol C", ReviewState::Accepted),
                entry("ghost", ReviewState::WaitForUpdates),
                entry("erin", ReviewState::Accepted),
                entry("Frank F", ReviewState::NeedReview),
                entry("Core", ReviewState::NeedReview),
                entry("acme/docs", ReviewState::NeedReview),
            ]
        );

        // A deleted author: every reviewer counts.
        let mine = [review(Some(user("U_author", "author")), "APPROVED")];
        assert_eq!(
            map(None, &mine, &[]),
            [entry("author", ReviewState::Accepted)]
        );
        assert!(map(Some(&author), &[], &[]).is_empty());
    }
}
