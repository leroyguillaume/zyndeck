-- Games catalogued by Zyndeck.
--
-- `name` is localised: a JSON object mapping a language code to the game's name
-- in that language, e.g. {"fr": "Marvel Champions", "en": "Marvel Champions"}.
-- The shape (ISO 639-1 keys, string values) is validated in the application
-- layer, not by a database CHECK. `created_by` references the user who added it.
CREATE TABLE game (
    id uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    name jsonb NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    created_by uuid NOT NULL REFERENCES "user" (id)
);
