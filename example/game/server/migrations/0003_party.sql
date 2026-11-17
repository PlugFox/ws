-- Parties belong to accounts (Phase 6.35): a party outlives its missions,
-- the way back to town, character switches and sign-outs. The server keeps
-- them in memory (`world::lobby`) and writes every change through to these
-- tables (`db::party_repo`); boot loads them back.
--
-- `id` is the party's id on the wire (`LOBBY_STATE`), allocated by the
-- server. `modifiers` is the leader's choice, `{"tier": u8, "mask": u32}`
-- (`world::modifiers`), checked again at every start.
CREATE TABLE parties (
    id         BIGINT PRIMARY KEY,
    code       TEXT NOT NULL UNIQUE,
    mission    TEXT NOT NULL,
    public     BOOLEAN NOT NULL,
    modifiers  JSONB NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- One row per account in a party (an account is in at most one). `seat`
-- orders the members: seat 0 leads. `character` is what the party window
-- shows for the member: their current character, or their last one while
-- offline, `{"id", "name", "archetype", "level"}`. `last_seen` is when
-- they went offline; NULL while online (at boot everyone counts as having
-- left then). A member offline for too long is dropped
-- (`lobby::MEMBER_TTL`), an empty party with them.
CREATE TABLE party_members (
    user_id   BIGINT PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    party_id  BIGINT NOT NULL REFERENCES parties (id) ON DELETE CASCADE,
    seat      SMALLINT NOT NULL,
    character JSONB NOT NULL,
    joined_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen TIMESTAMPTZ
);
CREATE INDEX party_members_party_idx ON party_members (party_id, seat);
