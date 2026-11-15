-- The Merchant line's homunculus (`hub::companion::HomSave`): its type,
-- name, level, experience into it, Bond (0..=100), evolution and stance,
-- `{"type", "name", "level", "exp", "bond", "evolved", "stance"}`. NULL:
-- never chosen (every other class, a Merchant before the first Call
-- Homunculus). Run HP is never stored: it stays with the stage instance.
ALTER TABLE characters ADD COLUMN homunculus JSONB;
