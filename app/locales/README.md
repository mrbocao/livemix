# Translating LiveMix

The interface uses English source strings. Each language catalog is a UTF-8 JSON object that maps an English source string to its translation. LiveMix displays English for entries the translator leaves out, so partial translations can be submitted and improved independently.

1. Copy `pt-BR.json` to a file named after the language's BCP 47 code, such as `es-ES.json`.
2. Keep the keys unchanged. Translate the values and omit any entries that still need review. Keep product names, codec names, keyboard chords and script commands recognizable.
3. Register the catalog in [`app/src/locale.rs`](../src/locale.rs) by adding a `Catalog` entry with its code, native display name and `include_str!` path.
4. From `app/`, run `cargo test --locked catalogs_parse_and_missing_translations_fall_back_to_english`. Build the app and choose your language in **Settings → Options → Interface language**.
5. Check that longer labels fit, that accented characters render, and that switching back to English works. Include screenshots of affected dialogs when practical.

To translate an interface label that is still in English, route it through `tr(ui, "English source")` or `locale::text(ctx, "English source")` and add the same key to the catalog. Keep widget IDs, preference keys, input names and preset contents independent of translations. For labels containing numbers or names, translate the static part before formatting.
